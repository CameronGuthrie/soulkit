//! Bounded worker diagnostics with actionable causes instead of bare exit codes.

use serde::Serialize;
use std::collections::VecDeque;

const RECENT_LINES: usize = 40;
const FAILURE_LINES: usize = 10;
const LINE_CHARACTERS: usize = 1_536;
const DETAIL_LINES: usize = 12;

#[derive(Debug, Clone, Serialize)]
pub struct RunFailure {
    pub kind: String,
    pub title: String,
    pub message: String,
    pub details: Vec<String>,
    pub suggestion: String,
    pub action: Option<String>,
    pub exit_code: Option<i32>,
}

impl RunFailure {
    /// Classify an infrastructure/start error without losing its original text.
    pub fn from_error(error: impl Into<String>, job_label: impl Into<String>) -> Self {
        let mut diagnostics = JobDiagnostics::default();
        let error = error.into();
        diagnostics.record(&error, "stderr");
        diagnostics.failure(&job_label.into(), None, &error)
    }

    pub fn error_text(&self) -> String {
        format!("{} {}", self.message, self.suggestion)
    }
}

#[derive(Debug, Clone)]
struct CapturedLine {
    text: String,
    stream: &'static str,
}

#[derive(Debug, Default)]
pub(crate) struct JobDiagnostics {
    recent: VecDeque<CapturedLine>,
    failures: Vec<CapturedLine>,
    specific: Option<(FailureKind, CapturedLine)>,
}

impl JobDiagnostics {
    pub fn record(&mut self, text: &str, stream: &'static str) {
        let text = bounded_clean_line(text);
        if text.is_empty() || decorative(&text) {
            return;
        }
        let line = CapturedLine { text, stream };
        if !warning(&line.text) {
            if self.specific.is_none() {
                if let Some(kind) = classify(&line.text) {
                    self.specific = Some((kind, line.clone()));
                }
            }
            if failure_context(&line.text)
                && self.failures.len() < FAILURE_LINES
                && !self.failures.iter().any(|item| item.text == line.text)
            {
                self.failures.push(line.clone());
            }
        }
        if self.recent.len() == RECENT_LINES {
            self.recent.pop_front();
        }
        self.recent.push_back(line);
    }

    pub fn failure(&self, job: &str, exit_code: Option<i32>, fallback: &str) -> RunFailure {
        let primary = self
            .specific
            .as_ref()
            .map(|(_, line)| line)
            .or_else(|| {
                self.failures
                    .iter()
                    .find(|line| !pipeline_summary(&line.text))
            })
            .or_else(|| self.failures.first())
            .or_else(|| {
                self.recent
                    .iter()
                    .rev()
                    .find(|line| line.stream == "stderr" && !warning(&line.text))
            });
        let cause = primary.map_or(fallback, |line| line.text.as_str());
        let kind = self
            .specific
            .as_ref()
            .map(|(kind, _)| *kind)
            .or_else(|| classify(cause))
            .unwrap_or(FailureKind::Other);
        let (name, title, message, suggestion, action) = match kind {
            FailureKind::CacheMissing => (
                "cache_missing",
                "Soulframe cache not found",
                "Soulkit could not find the installed game cache for this run.".to_string(),
                "In Settings, choose the game's Cache.Windows folder, save changes, and try again.",
                Some("open_cache_settings"),
            ),
            FailureKind::DiskFull => (
                "disk_full",
                "Not enough disk space",
                "The drive used for an export or temporary file ran out of available space.".into(),
                "Free space on the affected drive, or choose output and workspace folders on a drive with more space in Settings, then try again.",
                Some("open_output_settings"),
            ),
            FailureKind::PermissionDenied => (
                "permission_denied",
                "File or folder access was denied",
                "The extraction could not access a file or folder needed for this run.".into(),
                "Check the path in the diagnostic details and its permissions. Choose writable output and workspace folders in Settings, and close any program locking the file.",
                Some("open_output_settings"),
            ),
            FailureKind::DllMissing => (
                "dll_missing",
                "Required DLL unavailable",
                "A native library needed by the selected tool could not be found or loaded.".into(),
                "Check Runtime DLLs in Settings and choose the folder containing the required DLLs. Use DLL versions for this app's architecture, then try again.",
                Some("open_dll_settings"),
            ),
            FailureKind::Other => (
                "other",
                "Extraction failed",
                clean_cause(cause),
                "Review the diagnostic details and run output below. Copy the report when asking for help.",
                None,
            ),
        };
        let mut details = Vec::new();
        if !job.is_empty() {
            details.push(format!("Task: {job}"));
        }
        let mut append = |line: &CapturedLine| {
            let text = format!("[{}] {}", line.stream, line.text);
            if details.len() < DETAIL_LINES && !details.contains(&text) {
                details.push(text);
            }
        };
        if let Some(line) = primary {
            append(line);
        }
        for line in &self.failures {
            append(line);
        }
        // Keep a short tail as context, while retaining the earliest specific
        // cause separately even when later progress output is very large.
        for line in self.recent.iter().skip(self.recent.len().saturating_sub(6)) {
            append(line);
        }
        if details.len() == usize::from(!job.is_empty()) && !fallback.is_empty() {
            details.push(bounded_clean_line(fallback));
        }
        RunFailure {
            kind: name.into(),
            title: title.into(),
            message,
            details,
            suggestion: suggestion.into(),
            action: action.map(str::to_string),
            exit_code,
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum FailureKind {
    CacheMissing,
    DiskFull,
    PermissionDenied,
    DllMissing,
    Other,
}

fn classify(text: &str) -> Option<FailureKind> {
    let text = text.to_ascii_lowercase();
    if !failure_context(&text) {
        return None;
    }
    if [
        "no space left on device",
        "not enough space on the disk",
        "not enough disk space",
        "disk is full",
        "disk full",
        "disk quota exceeded",
        "storage quota exceeded",
        "enospc",
        "edquot",
    ]
    .iter()
    .any(|phrase| text.contains(phrase))
        || disk_full_code(&text)
    {
        return Some(FailureKind::DiskFull);
    }
    if [
        "permission denied",
        "access is denied",
        "access denied",
        "read-only file system",
        "readonly file system",
    ]
    .iter()
    .any(|phrase| text.contains(phrase))
        || (cfg!(windows) && os_error(&text, 5))
        || (cfg!(unix) && os_error(&text, 13))
    {
        return Some(FailureKind::PermissionDenied);
    }
    if (text.contains("soulframe cache")
        || text.contains("game cache")
        || text.contains("cache.windows"))
        && [
            "not found",
            "does not exist",
            "could not find",
            "unavailable",
        ]
        .iter()
        .any(|phrase| text.contains(phrase))
    {
        return Some(FailureKind::CacheMissing);
    }
    if (text.contains(".dll") || text.contains("oodle library") || text.contains("zstd library"))
        && [
            "not found",
            "could not locate",
            "failed to load",
            "cannot load",
            "could not load",
            "unavailable",
            "missing",
        ]
        .iter()
        .any(|phrase| text.contains(phrase))
    {
        return Some(FailureKind::DllMissing);
    }
    None
}

fn disk_full_code(text: &str) -> bool {
    // Raw OS codes have different meanings on Windows and Unix: for example,
    // Windows 122 means an insufficient buffer, not Linux EDQUOT.
    (cfg!(windows) && [112, 39, 1295].iter().any(|code| os_error(text, *code)))
        || (cfg!(unix) && [28, 122, 69].iter().any(|code| os_error(text, *code)))
}

fn os_error(text: &str, code: u32) -> bool {
    text.contains(&format!("os error {code})")) || text.contains(&format!("os error {code}:"))
}

fn failure_context(text: &str) -> bool {
    let text = text.trim().to_ascii_lowercase();
    if text.starts_with("0 errors") || text.starts_with("0 failed") || warning(&text) {
        return false;
    }
    [
        "error",
        "failed",
        "failure",
        "cannot ",
        "could not ",
        "not found",
        "does not exist",
        "unavailable",
        "no space left",
        "not enough space",
        "not enough disk space",
        "disk is full",
        "disk full",
        "quota exceeded",
        "permission denied",
        "access denied",
        "access is denied",
        "read-only file system",
        "readonly file system",
        "enospc",
        "edquot",
        "missing",
    ]
    .iter()
    .any(|phrase| text.contains(phrase))
}

fn warning(text: &str) -> bool {
    let text = text.trim().to_ascii_lowercase();
    text.starts_with("warning:")
        || text.starts_with("[warn]")
        || text.starts_with("note:")
        || audio_counter(&text)
        // This exact message announces a supported fallback, even when its
        // nested header-discovery error mentions a missing DLL. Actual native
        // file write failures are different messages and remain candidates.
        || (text.starts_with("[audio] vorbis headers unavailable")
            && text.contains("native data")
            && text.contains("preserv"))
}

fn audio_counter(text: &str) -> bool {
    let Some((prefix, counters)) = text.split_once(':') else {
        return false;
    };
    let progress_prefix = prefix
        .strip_prefix("audio ")
        .and_then(|numbers| numbers.split_once('/'))
        .is_some_and(|(processed, total)| {
            processed.parse::<usize>().is_ok() && total.parse::<usize>().is_ok()
        });
    (prefix == "audio complete" || progress_prefix)
        && counters.contains("exported")
        && counters.contains("unchanged")
        && counters.contains("failed")
}

fn pipeline_summary(text: &str) -> bool {
    text.to_ascii_lowercase()
        .contains("pipeline stages failed:")
}

fn decorative(text: &str) -> bool {
    text.chars()
        .all(|character| matches!(character, '=' | '#' | '-' | '_' | ' '))
}

fn clean_cause(text: &str) -> String {
    let text = text.trim();
    let text = text.strip_prefix("[!]").unwrap_or(text).trim();
    let text = text
        .strip_prefix("Extraction failed:")
        .unwrap_or(text)
        .trim();
    if text.is_empty() {
        "The extraction worker stopped without reporting a cause.".into()
    } else {
        bounded_clean_line(text)
    }
}

fn bounded_clean_line(text: &str) -> String {
    let mut clean = String::new();
    let mut escape = false;
    let mut csi = false;
    for character in text.chars() {
        if escape {
            escape = false;
            csi = character == '[';
            continue;
        }
        if csi {
            if ('@'..='~').contains(&character) {
                csi = false;
            }
            continue;
        }
        if character == '\u{1b}' {
            escape = true;
            continue;
        }
        if !character.is_control() || character == '\t' {
            clean.push(character);
        }
    }
    let clean = clean.trim();
    if clean.chars().count() <= LINE_CHARACTERS {
        return clean.into();
    }
    // Preserve the end of an unusually long path/error chain, where the OS
    // error and disk-full cause often appear, as well as the initial context.
    let head: String = clean.chars().take(LINE_CHARACTERS * 2 / 3).collect();
    let tail: Vec<char> = clean.chars().rev().take(LINE_CHARACTERS / 3).collect();
    format!("{head}…{}", tail.into_iter().rev().collect::<String>())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn screenshot_cache_failure_beats_the_exit_code_and_pipeline_summary() {
        let mut diagnostics = JobDiagnostics::default();
        for line in [
            "Soulframe Image Extractor",
            "[!] images extract failed for SF/Interface/: Soulframe cache not found at default location. Pass --cache-path to override.",
            "[!] audio extract failed: Soulframe cache not found; use --cache-path or set SOULFRAME_ROOT to Downloaded/Public",
            "Extraction failed: Pipeline stages failed: images, images, Interface SWFs, audio. Intermediates have been retained.",
        ] {
            diagnostics.record(line, "stdout");
        }
        let failure = diagnostics.failure(
            "Extract selected modules",
            Some(1),
            "Process exited with code 1",
        );
        assert_eq!(failure.kind, "cache_missing");
        assert_eq!(failure.title, "Soulframe cache not found");
        assert_eq!(failure.action.as_deref(), Some("open_cache_settings"));
        assert_eq!(failure.exit_code, Some(1));
        assert!(failure.suggestion.contains("Settings"));
        assert!(failure.details[1].contains("SF/Interface/"));
        assert!(!failure.message.contains("code 1"));
    }

    #[test]
    fn specific_error_is_retained_after_a_large_log_tail_and_ansi_is_removed() {
        let mut diagnostics = JobDiagnostics::default();
        diagnostics.record("\u{1b}[31mFailed writing C:/exports/audio/example.wav: No space left on device (os error 28)\u{1b}[0m", "stderr");
        for _ in 0..500 {
            diagnostics.record(&"progress".repeat(2_000), "stdout");
        }
        diagnostics.record("Extraction failed: Pipeline stages failed: audio", "stderr");
        let failure = diagnostics.failure("Audio", Some(1), "Exited");
        assert_eq!(failure.kind, "disk_full");
        assert!(failure
            .details
            .iter()
            .any(|line| line.contains("example.wav")));
        assert!(failure.details.iter().all(|line| !line.contains('\u{1b}')));
        assert_eq!(diagnostics.recent.len(), RECENT_LINES);
        assert!(diagnostics
            .recent
            .iter()
            .all(|line| line.text.chars().count() <= LINE_CHARACTERS + 1));
    }

    #[test]
    fn permissions_and_unknown_failures_retain_the_original_cause() {
        let permissions = RunFailure::from_error(
            "Cannot create D:/output/audio: Access is denied. (os error 5)",
            "Audio",
        );
        assert_eq!(permissions.kind, "permission_denied");
        assert!(permissions
            .details
            .iter()
            .any(|line| line.contains("D:/output/audio")));
        let unknown =
            RunFailure::from_error("Extraction failed: A decoder rejected frame 17", "Audio");
        assert_eq!(unknown.kind, "other");
        assert!(unknown.message.contains("decoder rejected frame 17"));
        assert_eq!(unknown.action, None);
    }

    #[test]
    fn success_counters_and_warnings_do_not_become_the_failure_cause() {
        let mut diagnostics = JobDiagnostics::default();
        diagnostics.record("0 errors", "stdout");
        diagnostics.record("warning: optional game cache metadata not found", "stderr");
        diagnostics.record("[!] Decode failed: Unsupported codec 99", "stderr");
        let failure = diagnostics.failure("Audio", Some(1), "Exited");
        assert_eq!(failure.kind, "other");
        assert!(failure.message.contains("Unsupported codec 99"));
    }

    #[test]
    fn repeated_audio_statistics_do_not_fill_the_failure_slots_before_an_unknown_error() {
        let mut diagnostics = JobDiagnostics::default();
        for count in 1..30 {
            diagnostics.record(
                &format!("audio {count}/16157: {count} exported (0 native), 0 unchanged, 0 failed"),
                "stdout",
            );
        }
        diagnostics.record(
            "Audio complete: 29 exported (0 native), 0 unchanged, 0 failed; output/audio",
            "stdout",
        );
        diagnostics.record("[!] Decode failed: Unsupported packet length 17", "stderr");
        let failure = diagnostics.failure("Audio", Some(1), "Exited with code 1");
        assert_eq!(diagnostics.failures.len(), 1);
        assert!(diagnostics.specific.is_none());
        assert_eq!(failure.kind, "other");
        assert!(failure.message.contains("Unsupported packet length 17"));
        assert!(!failure.message.contains("0 failed"));
        assert!(failure
            .details
            .iter()
            .any(|line| line.contains("Audio complete:")));
    }

    #[test]
    fn native_preservation_fallback_is_context_instead_of_a_missing_dll_failure() {
        let mut diagnostics = JobDiagnostics::default();
        diagnostics.record(
            "[audio] Vorbis headers unavailable (Failed to load Oodle library from C:/Soulkit/oo2core_9.dll); affected assets will be preserved as native data. Use --game-exe with the matching client.",
            "stderr",
        );
        diagnostics.record("[!] Decode failed: Unsupported packet length 17", "stderr");
        let failure = diagnostics.failure("Audio", Some(1), "Exited with code 1");
        assert_eq!(diagnostics.failures.len(), 1);
        assert!(diagnostics.specific.is_none());
        assert_eq!(failure.kind, "other");
        assert!(failure.message.contains("Unsupported packet length 17"));
        assert!(failure
            .details
            .iter()
            .any(|line| line.contains("preserved as native data")));
    }

    #[test]
    fn actual_native_file_write_failures_remain_fatal_candidates() {
        let failure = RunFailure::from_error(
            "Cannot write native audio file D:/output/native/example.dsp: No space left on device",
            "Audio",
        );
        assert_eq!(failure.kind, "disk_full");
        assert!(failure
            .details
            .iter()
            .any(|line| line.contains("example.dsp")));
    }

    #[test]
    fn native_library_errors_have_a_dll_settings_action() {
        let failure = RunFailure::from_error("Failed to load Oodle library from C:/Soulkit/oo2core_9.dll: The specified module could not be found. (os error 126)", "Images");
        assert_eq!(failure.kind, "dll_missing");
        assert_eq!(failure.action.as_deref(), Some("open_dll_settings"));
    }

    #[cfg(windows)]
    #[test]
    fn windows_disk_full_codes_are_distinguished_from_insufficient_buffer() {
        for code in [112, 39, 1295] {
            let failure =
                RunFailure::from_error(format!("Write failed (os error {code})"), "Images");
            assert_eq!(failure.kind, "disk_full");
        }
        assert_eq!(
            RunFailure::from_error("Write failed (os error 122)", "Images").kind,
            "other"
        );
    }

    #[cfg(unix)]
    #[test]
    fn unix_disk_full_and_quota_codes_are_recognized() {
        for code in [28, 122, 69] {
            let failure =
                RunFailure::from_error(format!("Write failed (os error {code})"), "Images");
            assert_eq!(failure.kind, "disk_full");
        }
    }
}
