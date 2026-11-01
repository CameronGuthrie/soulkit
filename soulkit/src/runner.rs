//! Sequential built-in extraction jobs with bounded logs and process cancellation.

use crate::diagnostics::{JobDiagnostics, RunFailure};
use crate::tasks::{self, Job, RunPlan, RunSettings, TaskId};
use serde::Serialize;
use std::io::Read;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::Emitter;

#[derive(Debug, Clone, Serialize)]
pub struct RunStatus {
    pub status: String,
    pub run_id: u64,
    pub job_index: usize,
    pub total_jobs: usize,
    pub label: String,
    /// Fraction of the entire plan, from zero to one.
    pub progress: f32,
    pub error: Option<String>,
    pub failure: Option<RunFailure>,
}

impl Default for RunStatus {
    fn default() -> Self {
        Self {
            status: "idle".into(),
            run_id: 0,
            job_index: 0,
            total_jobs: 0,
            label: "Ready to extract".into(),
            progress: 0.0,
            error: None,
            failure: None,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct RunEvent {
    pub kind: String,
    pub status: RunStatus,
    pub line: Option<String>,
    pub stream: Option<String>,
}

type Emit = Arc<dyn Fn(RunEvent) + Send + Sync>;

struct Inner {
    status: Mutex<RunStatus>,
    stage: Mutex<(usize, usize)>,
    child: Mutex<Option<Child>>,
    cancel: AtomicBool,
    sequence: AtomicU64,
    diagnostics: Mutex<JobDiagnostics>,
}

#[derive(Clone)]
pub struct RunnerService {
    inner: Arc<Inner>,
}

impl Default for RunnerService {
    fn default() -> Self {
        Self::new()
    }
}

impl RunnerService {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Inner {
                status: Mutex::new(RunStatus::default()),
                stage: Mutex::new((1, 1)),
                child: Mutex::new(None),
                cancel: AtomicBool::new(false),
                sequence: AtomicU64::new(0),
                diagnostics: Mutex::new(JobDiagnostics::default()),
            }),
        }
    }

    pub fn status(&self) -> RunStatus {
        self.inner
            .status
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn start(
        &self,
        app: tauri::AppHandle,
        settings: RunSettings,
        selected: Vec<TaskId>,
    ) -> Result<RunPlan, String> {
        let settings = tasks::resolved(&settings)?;
        let plan = tasks::plan(&settings, &selected)?;
        let emit: Emit = Arc::new(move |event| {
            let _ = app.emit("run-event", event);
        });
        self.start_with_emit(settings, plan, emit)
    }

    fn start_with_emit(
        &self,
        settings: RunSettings,
        plan: RunPlan,
        emit: Emit,
    ) -> Result<RunPlan, String> {
        self.start_with_preparation(settings, plan, emit, crate::preflight::prepare)
    }

    fn start_with_preparation<F>(
        &self,
        settings: RunSettings,
        plan: RunPlan,
        emit: Emit,
        prepare: F,
    ) -> Result<RunPlan, String>
    where
        F: FnOnce(RunSettings, &RunPlan) -> Result<RunSettings, RunFailure> + Send + 'static,
    {
        self.start_with_resources(settings, plan, emit, prepare, |_| None)
    }

    /// A custom plan shares process ownership, cancellation, diagnostics and
    /// preflight lifecycle with ordinary module extraction.
    pub fn start_prepared<F, C>(
        &self,
        app: tauri::AppHandle,
        settings: RunSettings,
        plan: RunPlan,
        prepare: F,
        cleanup: C,
    ) -> Result<RunPlan, String>
    where
        F: FnOnce(RunSettings, &RunPlan) -> Result<RunSettings, RunFailure> + Send + 'static,
        C: FnOnce(&str) -> Option<String> + Send + 'static,
    {
        let emit: Emit = Arc::new(move |event| {
            let _ = app.emit("run-event", event);
        });
        self.start_with_resources(settings, plan, emit, prepare, cleanup)
    }

    fn start_with_resources<F, C>(
        &self,
        settings: RunSettings,
        plan: RunPlan,
        emit: Emit,
        prepare: F,
        cleanup: C,
    ) -> Result<RunPlan, String>
    where
        F: FnOnce(RunSettings, &RunPlan) -> Result<RunSettings, RunFailure> + Send + 'static,
        C: FnOnce(&str) -> Option<String> + Send + 'static,
    {
        let run_id = {
            let mut status = self
                .inner
                .status
                .lock()
                .map_err(|_| "Runner state is unavailable.")?;
            if matches!(status.status.as_str(), "running" | "cancelling") {
                return Err(
                    "An extraction is already running. Wait for it or cancel it first.".into(),
                );
            }
            self.inner.cancel.store(false, Ordering::Release);
            *status = RunStatus {
                status: "running".into(),
                run_id: self.inner.sequence.fetch_add(1, Ordering::Relaxed) + 1,
                job_index: 0,
                total_jobs: plan.jobs.len(),
                label: "Checking game cache and storage".into(),
                progress: 0.0,
                error: None,
                failure: None,
            };
            status.run_id
        };
        self.reset_diagnostics();
        self.emit_status(&emit);
        let service = self.clone();
        let worker_plan = plan.clone();
        std::thread::spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                // Preparation can involve a slow external/UNC filesystem.
                // It belongs to the managed run so Stop/CloseRequested cannot
                // miss it and allow a worker to start after cancellation.
                if service.inner.cancel.load(Ordering::Acquire) {
                    service.finish("cancelled", None, &emit);
                    return "cancelled";
                }
                let prepared = prepare(settings, &worker_plan);
                if service.inner.cancel.load(Ordering::Acquire) {
                    service.finish("cancelled", None, &emit);
                    return "cancelled";
                }
                match prepared {
                    Ok(settings) => service.run_sequence(&settings, &worker_plan, &emit),
                    Err(failure) => service.finish_failure(failure, &emit),
                }
            }));
            let outcome = match result {
                Ok(outcome) => outcome,
                Err(payload) => {
                    let _ = service.stop_child();
                    let reason = payload
                        .downcast_ref::<String>()
                        .map(String::as_str)
                        .or_else(|| payload.downcast_ref::<&str>().copied())
                        .unwrap_or("No panic details were provided");
                    service.finish(
                        "failed",
                        Some(format!(
                            "The extraction worker stopped unexpectedly: {reason}"
                        )),
                        &emit,
                    )
                }
            };
            if let Some(line) = cleanup(outcome) {
                let status = service.status();
                if status.run_id == run_id {
                    emit(RunEvent {
                        kind: "log".into(),
                        status,
                        line: Some(line),
                        stream: Some("runner".into()),
                    });
                }
            }
        });
        Ok(plan)
    }

    pub fn cancel(&self) -> Result<(), String> {
        {
            let mut status = self
                .inner
                .status
                .lock()
                .map_err(|_| "Runner state is unavailable.")?;
            if !matches!(status.status.as_str(), "running" | "cancelling") {
                return Ok(());
            }
            self.inner.cancel.store(true, Ordering::Release);
            status.status = "cancelling".into();
            status.label = "Stopping extraction…".into();
        }
        // CloseRequested also calls this method. Finish terminating the child
        // before returning so closing the GUI cannot strand a CLI/game process.
        self.stop_child()
    }

    fn emit_status(&self, emit: &Emit) {
        emit(RunEvent {
            kind: "status".into(),
            status: self.status(),
            line: None,
            stream: None,
        });
    }

    fn finish(&self, state: &str, error: Option<String>, emit: &Emit) -> &'static str {
        if state == "failed" {
            let label = self.status().label;
            let mut diagnostics = self
                .inner
                .diagnostics
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if let Some(error) = &error {
                diagnostics.record(error, "runner");
            }
            let failure = diagnostics.failure(
                &label,
                None,
                error
                    .as_deref()
                    .unwrap_or("The extraction worker stopped without reporting a cause."),
            );
            drop(diagnostics);
            return self.finish_failure(failure, emit);
        }
        {
            let mut status = self.inner.status.lock().unwrap_or_else(|e| e.into_inner());
            status.status = state.into();
            status.error = error;
            status.failure = None;
            status.label = match state {
                "completed" => "Extraction complete",
                "cancelled" => "Extraction cancelled; partial outputs were kept",
                _ => "Extraction stopped with an error",
            }
            .into();
            if state == "completed" {
                status.progress = 1.0;
            }
        }
        self.emit_status(emit);
        if state == "cancelled" {
            "cancelled"
        } else {
            "completed"
        }
    }

    fn reset_diagnostics(&self) {
        *self
            .inner
            .diagnostics
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = JobDiagnostics::default();
    }

    fn finish_failure(&self, failure: RunFailure, emit: &Emit) -> &'static str {
        if self.inner.cancel.load(Ordering::Acquire) {
            return self.finish("cancelled", None, emit);
        }
        {
            let mut status = self.inner.status.lock().unwrap_or_else(|e| e.into_inner());
            if self.inner.cancel.load(Ordering::Acquire) {
                drop(status);
                return self.finish("cancelled", None, emit);
            }
            status.status = "failed".into();
            status.label = failure.title.clone();
            status.error = Some(failure.error_text());
            status.failure = Some(failure);
        }
        self.emit_status(emit);
        "failed"
    }

    fn run_sequence(&self, settings: &RunSettings, plan: &RunPlan, emit: &Emit) -> &'static str {
        for (index, job) in plan.jobs.iter().enumerate() {
            if self.inner.cancel.load(Ordering::Acquire) {
                self.finish("cancelled", None, emit);
                return "cancelled";
            }
            self.reset_diagnostics();
            {
                *self.inner.stage.lock().unwrap_or_else(|e| e.into_inner()) = (1, 1);
                let mut status = self.inner.status.lock().unwrap_or_else(|e| e.into_inner());
                status.job_index = index + 1;
                status.label = job.label.clone();
                status.progress = index as f32 / plan.jobs.len() as f32;
            }
            self.emit_status(emit);
            match self.run_job(settings, job, index, plan.jobs.len(), emit) {
                Ok(Some(status)) if status.success() => {}
                Ok(None) => {
                    self.finish("cancelled", None, emit);
                    return "cancelled";
                }
                Ok(Some(status)) => {
                    if self.inner.cancel.load(Ordering::Acquire) {
                        self.finish("cancelled", None, emit);
                        return "cancelled";
                    }
                    let fallback = format!(
                        "{} exited with {} without reporting a more specific cause.",
                        job.label,
                        status
                            .code()
                            .map_or_else(|| "a termination signal".into(), |c| format!("code {c}"))
                    );
                    let failure = self
                        .inner
                        .diagnostics
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .failure(&job.label, status.code(), &fallback);
                    return self.finish_failure(failure, emit);
                }
                Err(error) => {
                    if self.inner.cancel.load(Ordering::Acquire) {
                        self.finish("cancelled", None, emit);
                        return "cancelled";
                    } else {
                        return self.finish("failed", Some(error), emit);
                    }
                }
            }
        }
        if self.inner.cancel.load(Ordering::Acquire) {
            self.finish("cancelled", None, emit);
            "cancelled"
        } else {
            self.finish("completed", None, emit);
            "completed"
        }
    }

    fn run_job(
        &self,
        settings: &RunSettings,
        job: &Job,
        index: usize,
        total: usize,
        emit: &Emit,
    ) -> Result<Option<ExitStatus>, String> {
        let command = worker_command(settings, job)?;
        self.run_command(command, index, total, emit)
    }

    fn run_command(
        &self,
        mut command: Command,
        index: usize,
        total: usize,
        emit: &Emit,
    ) -> Result<Option<ExitStatus>, String> {
        self.reset_diagnostics();
        // Share process creation with cancellation so closing the GUI cannot
        // miss a process between its spawn and registration in the child slot.
        let (stdout, stderr) = {
            let mut slot = self
                .inner
                .child
                .lock()
                .map_err(|_| "Runner child state is unavailable.")?;
            if self.inner.cancel.load(Ordering::Acquire) {
                return Ok(None);
            }
            let mut child = command
                .spawn()
                .map_err(|e| format!("Could not start the extraction worker: {e}"))?;
            let pipes = child.stdout.take().zip(child.stderr.take());
            let Some(pipes) = pipes else {
                let _ = child.kill();
                let _ = child.wait();
                return Err("Could not capture extraction output.".into());
            };
            *slot = Some(child);
            pipes
        };
        let (sender, receiver) = mpsc::sync_channel(256);
        let readers = [
            read_pipe(stdout, "stdout", sender.clone()),
            read_pipe(stderr, "stderr", sender),
        ];
        let result = loop {
            for _ in 0..256 {
                match receiver.try_recv() {
                    Ok(line) => self.emit_line(line, index, total, emit),
                    Err(_) => break,
                }
            }
            if self.inner.cancel.load(Ordering::Acquire) {
                self.emit_status(emit);
                self.stop_child()?;
                break None;
            }
            let status = {
                let mut child = self
                    .inner
                    .child
                    .lock()
                    .map_err(|_| "Runner child state is unavailable.")?;
                child
                    .as_mut()
                    .ok_or("The extraction process is missing.")?
                    .try_wait()
                    .map_err(|e| format!("Could not read extraction status: {e}"))?
            };
            if let Some(status) = status {
                let mut child = self
                    .inner
                    .child
                    .lock()
                    .map_err(|_| "Runner child state is unavailable.")?;
                if let Some(mut child) = child.take() {
                    let _ = child.wait();
                }
                break Some(status);
            }
            std::thread::sleep(Duration::from_millis(25));
        };
        // Drain normal pipe endings without hanging on a descendant that kept
        // an inherited handle. Cancellation has already killed the entire tree.
        let deadline = Instant::now() + Duration::from_millis(500);
        while Instant::now() < deadline {
            match receiver.recv_timeout(Duration::from_millis(15)) {
                Ok(line) => self.emit_line(line, index, total, emit),
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    if readers.iter().all(std::thread::JoinHandle::is_finished) {
                        break;
                    }
                }
            }
        }
        drop(receiver);
        for reader in readers {
            if reader.is_finished() {
                let _ = reader.join();
            }
        }
        Ok(result)
    }

    fn stop_child(&self) -> Result<(), String> {
        let mut slot = self
            .inner
            .child
            .lock()
            .map_err(|_| "Runner child state is unavailable.")?;
        if let Some(mut child) = slot.take() {
            if child.try_wait().map_err(|e| e.to_string())?.is_none() {
                #[cfg(windows)]
                {
                    let mut kill = Command::new("taskkill.exe");
                    kill.args(["/PID", &child.id().to_string(), "/T", "/F"])
                        .stdout(Stdio::null())
                        .stderr(Stdio::null());
                    hide_console(&mut kill);
                    let _ = kill.status();
                }
                let _ = child.kill();
            }
            child
                .wait()
                .map_err(|e| format!("Could not wait for the stopped extraction: {e}"))?;
        }
        Ok(())
    }

    fn emit_line(&self, line: LogLine, index: usize, total: usize, emit: &Emit) {
        self.inner
            .diagnostics
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .record(&line.text, line.stream);
        if let Some((stage, stages, label)) = stage_progress(&line.text) {
            *self.inner.stage.lock().unwrap_or_else(|e| e.into_inner()) = (stage, stages);
            let mut status = self.inner.status.lock().unwrap_or_else(|e| e.into_inner());
            if status.status == "running" {
                status.label = label;
                // A stage header announces its start, not its completion.
                let fraction = stage.saturating_sub(1) as f32 / stages as f32;
                status.progress = (index as f32 + fraction) / total as f32;
            }
        } else if let Some((processed, files)) = audio_progress(&line.text) {
            let (stage, stages) = *self.inner.stage.lock().unwrap_or_else(|e| e.into_inner());
            let mut status = self.inner.status.lock().unwrap_or_else(|e| e.into_inner());
            if status.status == "running" {
                status.label = format!("Audio · {processed} / {files} assets");
                let fraction = (stage.saturating_sub(1) as f32 + processed as f32 / files as f32)
                    / stages as f32;
                status.progress = (index as f32 + fraction) / total as f32;
            }
        }
        emit(RunEvent {
            kind: "log".into(),
            status: self.status(),
            line: Some(line.text),
            stream: Some(line.stream.into()),
        });
    }
}

fn worker_command(settings: &RunSettings, job: &Job) -> Result<Command, String> {
    let executable = std::env::current_exe()
        .map_err(|e| format!("Cannot locate the application executable: {e}"))?;
    let mut command = Command::new(executable);
    command
        .arg(crate::worker::WORKER_FLAG)
        .args(&job.args)
        .current_dir(&settings.workspace)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    crate::runtime_libraries::configure_worker(
        &mut command,
        &settings.workspace,
        settings.dll_directory.as_deref(),
    )?;
    if let Some(cache) = &settings.cache_path {
        let game = if cache
            .file_name()
            .is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case("Cache.Windows"))
        {
            cache.parent().unwrap_or(cache)
        } else {
            cache.as_path()
        };
        command.env("SOULFRAME_ROOT", game);
    }
    hide_console(&mut command);
    Ok(command)
}

fn hide_console(command: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    #[cfg(not(windows))]
    let _ = command;
}

struct LogLine {
    text: String,
    stream: &'static str,
}

fn read_pipe(
    mut pipe: impl Read + Send + 'static,
    stream: &'static str,
    sender: mpsc::SyncSender<LogLine>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let mut buffer = [0u8; 8192];
        let mut line = Vec::new();
        loop {
            match pipe.read(&mut buffer) {
                Ok(0) => break,
                Ok(length) => {
                    for &byte in &buffer[..length] {
                        if byte == b'\n' || byte == b'\r' {
                            if !line.is_empty() {
                                let text = String::from_utf8_lossy(&line).into_owned();
                                if sender.send(LogLine { text, stream }).is_err() {
                                    return;
                                }
                                line.clear();
                            }
                        } else if line.len() < 65_536 {
                            line.push(byte);
                        }
                    }
                }
                Err(_) => break,
            }
        }
        if !line.is_empty() {
            let _ = sender.send(LogLine {
                text: String::from_utf8_lossy(&line).into_owned(),
                stream,
            });
        }
    })
}

fn stage_progress(line: &str) -> Option<(usize, usize, String)> {
    let (_, rest) = line.split_once("stage ")?;
    let (numbers, label) = rest.split_once(':')?;
    let (stage, stages) = numbers.trim().split_once('/')?;
    let stage = stage.parse().ok()?;
    let stages = stages.parse().ok()?;
    if stages == 0 || stage == 0 || stage > stages {
        return None;
    }
    Some((stage, stages, label.trim().into()))
}

fn audio_progress(line: &str) -> Option<(usize, usize)> {
    let rest = line.trim().strip_prefix("audio ")?;
    let (numbers, _) = rest.split_once(':')?;
    let (processed, files) = numbers.split_once('/')?;
    let processed: usize = processed.parse().ok()?;
    let files: usize = files.parse().ok()?;
    (files > 0 && processed <= files).then_some((processed, files))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn preparation_fixture() -> (RunSettings, RunPlan) {
        let settings = RunSettings {
            workspace: std::env::temp_dir(),
            output: std::env::temp_dir(),
            soulkit: std::env::temp_dir().join("not-a-separate-worker.exe"),
            cache_path: None,
            dll_directory: None,
            locales: "en".into(),
            keep_intermediates: false,
        };
        let plan = RunPlan {
            requested: vec![TaskId::Calendars],
            modules: vec![TaskId::Calendars],
            dependencies: vec![],
            jobs: vec![Job {
                id: "must-not-start".into(),
                label: "Must never start".into(),
                args: vec!["this-command-must-never-run".into()],
            }],
        };
        (settings, plan)
    }

    #[test]
    fn stop_during_slow_preparation_prevents_workers_and_wins_over_check_failure() {
        for fail_check in [false, true] {
            let service = RunnerService::new();
            let (settings, plan) = preparation_fixture();
            let (entered_tx, entered_rx) = mpsc::channel();
            let (release_tx, release_rx) = mpsc::channel();
            let (event_tx, event_rx) = mpsc::channel();
            let emit: Emit = Arc::new(move |event| {
                let _ = event_tx.send(event);
            });
            service
                .start_with_preparation(settings, plan, emit, move |settings, _| {
                    entered_tx.send(()).unwrap();
                    release_rx.recv_timeout(Duration::from_secs(10)).unwrap();
                    if fail_check {
                        Err(RunFailure::from_error(
                            "Soulframe cache not found",
                            "Check cache",
                        ))
                    } else {
                        Ok(settings)
                    }
                })
                .unwrap();
            entered_rx.recv_timeout(Duration::from_secs(10)).unwrap();
            let checking = service.status();
            assert_eq!(checking.status, "running");
            assert_eq!(checking.job_index, 0);
            assert!(checking.label.contains("Checking game cache"));
            assert!(service.inner.child.lock().unwrap().is_none());
            service.cancel().unwrap();
            assert_eq!(service.status().status, "cancelling");
            release_tx.send(()).unwrap();
            loop {
                let event = event_rx.recv_timeout(Duration::from_secs(10)).unwrap();
                assert_eq!(event.status.job_index, 0, "No extraction may start");
                assert_ne!(event.status.status, "failed", "Cancellation must win");
                if event.status.status == "cancelled" {
                    assert!(event.status.failure.is_none());
                    break;
                }
            }
            assert!(service.inner.child.lock().unwrap().is_none());
        }
    }

    #[test]
    fn preparation_failure_is_reported_before_any_job_or_process_starts() {
        let service = RunnerService::new();
        let (settings, plan) = preparation_fixture();
        let (sender, receiver) = mpsc::channel();
        let emit: Emit = Arc::new(move |event| {
            let _ = sender.send(event);
        });
        service
            .start_with_preparation(settings, plan, emit, |_, _| {
                Err(RunFailure::from_error(
                    "Soulframe cache not found",
                    "Check cache",
                ))
            })
            .unwrap();
        loop {
            let event = receiver.recv_timeout(Duration::from_secs(10)).unwrap();
            assert_eq!(event.status.job_index, 0);
            if event.status.status == "failed" {
                assert_eq!(event.status.failure.unwrap().kind, "cache_missing");
                assert_eq!(event.status.progress, 0.0);
                break;
            }
        }
        assert!(service.inner.child.lock().unwrap().is_none());
    }

    #[test]
    fn reads_bounded_utf8_lines_and_real_stage_progress() {
        let (sender, receiver) = mpsc::sync_channel(8);
        let reader = read_pipe(
            std::io::Cursor::new("first\r\nsecond — voice\n".as_bytes().to_vec()),
            "stderr",
            sender,
        );
        reader.join().unwrap();
        let lines: Vec<_> = receiver.iter().collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[1].text, "second — voice");
        assert_eq!(lines[0].stream, "stderr");
        assert_eq!(
            stage_progress("# soulkit-cli run-all — stage 3/8: audio extract"),
            Some((3, 8, "audio extract".into()))
        );
        assert!(stage_progress("stage 5/0: wrong").is_none());
        assert_eq!(
            audio_progress("  audio 250/16157: 250 exported"),
            Some((250, 16157))
        );
        assert!(audio_progress("audio 251/0: invalid").is_none());
    }

    #[test]
    #[ignore = "helper subprocess for the cancellation test"]
    fn child_fixture() {
        use std::io::Write;
        println!("runner-fixture-ready");
        std::io::stdout().flush().unwrap();
        loop {
            std::thread::sleep(Duration::from_secs(1));
        }
    }

    #[test]
    #[ignore = "helper subprocess for the diagnostics pipe test"]
    fn failed_child_fixture() {
        use std::io::Write;
        println!("[!] images extract failed for SF/Interface/: Soulframe cache not found at default location. Pass --cache-path to override.");
        for _ in 0..100 {
            println!("continuing pipeline output");
        }
        eprintln!("Extraction failed: Pipeline stages failed: images, audio. Intermediates have been retained.");
        std::io::stdout().flush().unwrap();
        std::io::stderr().flush().unwrap();
        std::process::exit(1);
    }

    #[test]
    fn a_real_failed_child_reports_its_stdout_cause_instead_of_just_code_one() {
        let service = RunnerService::new();
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--ignored",
                "--exact",
                "runner::tests::failed_child_fixture",
                "--nocapture",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        hide_console(&mut command);
        service.inner.status.lock().unwrap().status = "running".into();
        let emit: Emit = Arc::new(|_| {});
        let exit = service.run_command(command, 0, 1, &emit).unwrap().unwrap();
        assert_eq!(exit.code(), Some(1));
        let failure = service.inner.diagnostics.lock().unwrap().failure(
            "Extract selected modules",
            exit.code(),
            "Exited with code 1",
        );
        service.finish_failure(failure, &emit);
        let status = service.status();
        assert_eq!(status.status, "failed");
        assert_eq!(status.label, "Soulframe cache not found");
        let failure = status.failure.unwrap();
        assert_eq!(failure.kind, "cache_missing");
        assert_eq!(failure.exit_code, Some(1));
        assert!(failure
            .details
            .iter()
            .any(|line| line.contains("SF/Interface/")));
        assert!(!status.error.unwrap().contains("code 1"));
    }

    #[test]
    fn completed_cancelled_and_reset_jobs_do_not_reuse_previous_failure_details() {
        let service = RunnerService::new();
        let emit: Emit = Arc::new(|_| {});
        service.emit_line(
            LogLine {
                text: "Soulframe cache not found".into(),
                stream: "stdout",
            },
            0,
            1,
            &emit,
        );
        service.finish("failed", Some("Extraction stopped".into()), &emit);
        assert_eq!(service.status().failure.unwrap().kind, "cache_missing");
        service.reset_diagnostics();
        service.finish("failed", Some("A separate decoder failure".into()), &emit);
        assert_eq!(service.status().failure.unwrap().kind, "other");
        service.finish("completed", None, &emit);
        assert!(service.status().failure.is_none());
        assert!(service.status().error.is_none());
        service.inner.cancel.store(true, Ordering::Release);
        service.finish_failure(
            RunFailure::from_error("No space left on device", "Images"),
            &emit,
        );
        assert_eq!(service.status().status, "cancelled");
        assert!(service.status().failure.is_none());
        assert!(service.status().error.is_none());
    }

    #[test]
    fn cancellation_before_spawn_never_attempts_to_start_the_program() {
        let service = RunnerService::new();
        service.inner.cancel.store(true, Ordering::Release);
        let settings = RunSettings {
            workspace: std::env::temp_dir(),
            output: std::env::temp_dir(),
            soulkit: std::env::temp_dir().join("does-not-exist-soulkit-runner.exe"),
            cache_path: None,
            dll_directory: None,
            locales: "en".into(),
            keep_intermediates: false,
        };
        let job = Job {
            id: "never".into(),
            label: "Never started".into(),
            args: vec![],
        };
        let emit: Emit = Arc::new(|_| {});
        assert!(service
            .run_job(&settings, &job, 0, 1, &emit)
            .unwrap()
            .is_none());
        assert!(service.inner.child.lock().unwrap().is_none());
    }

    #[test]
    fn worker_uses_this_executable_and_preserves_argument_boundaries() {
        let settings = RunSettings {
            workspace: std::env::temp_dir(),
            output: std::env::temp_dir(),
            soulkit: std::env::temp_dir().join("unavailable-old-cli.exe"),
            cache_path: None,
            dll_directory: None,
            locales: "en".into(),
            keep_intermediates: false,
        };
        let job = Job {
            id: "calendar".into(),
            label: "Calendar".into(),
            args: vec![
                "zenith-calendar".into(),
                "--output".into(),
                "folder with spaces/calendar.json".into(),
            ],
        };
        let command = worker_command(&settings, &job).unwrap();
        assert_eq!(command.get_program(), std::env::current_exe().unwrap());
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            [
                crate::worker::WORKER_FLAG,
                "zenith-calendar",
                "--output",
                "folder with spaces/calendar.json",
            ]
            .map(std::ffi::OsStr::new)
        );
    }

    #[test]
    fn cancellation_stops_and_reaps_a_real_child_without_running_extractors() {
        let service = RunnerService::new();
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--ignored",
                "--exact",
                "runner::tests::child_fixture",
                "--nocapture",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        hide_console(&mut command);
        service.inner.status.lock().unwrap().status = "running".into();
        let (sender, receiver) = mpsc::channel();
        let emit: Emit = Arc::new(move |event| {
            let _ = sender.send(event);
        });
        let worker = service.clone();
        let thread = std::thread::spawn(move || {
            let result = worker.run_command(command, 0, 1, &emit);
            assert!(result.is_ok());
            worker.finish("cancelled", None, &emit);
        });
        let start = Instant::now();
        loop {
            let event = receiver.recv_timeout(Duration::from_secs(10)).unwrap();
            if event
                .line
                .as_deref()
                .is_some_and(|line| line.contains("runner-fixture-ready"))
            {
                break;
            }
            assert!(start.elapsed() < Duration::from_secs(10));
        }
        service.cancel().unwrap();
        loop {
            let event = receiver.recv_timeout(Duration::from_secs(10)).unwrap();
            if event.status.status == "cancelled" {
                break;
            }
        }
        assert!(service.inner.child.lock().unwrap().is_none());
        assert_eq!(service.status().status, "cancelled");
        thread.join().unwrap();
    }
}
