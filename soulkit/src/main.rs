#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod cache_browser;
mod cache_preview;
mod config;
mod diagnostics;
mod library;
mod preflight;
mod runner;
mod runtime_libraries;
#[cfg(windows)]
mod taskbar_icon;
mod tasks;
mod worker;

use config::{BootInfo, ConfigStore};
use library::{FilePreview, LibraryEntry, LibraryStats};
use runner::{RunStatus, RunnerService};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tasks::{RunPlan, RunSettings, TaskId};
use tauri::{Manager, State};

struct AppState {
    config: Mutex<ConfigStore>,
}

fn settings(state: &AppState) -> Result<RunSettings, String> {
    state
        .config
        .lock()
        .map(|config| config.snapshot())
        .map_err(|_| "Settings are temporarily unavailable.".into())
}

fn allow_output(app: &tauri::AppHandle, output: &Path) -> Result<(), String> {
    if output.is_dir() {
        app.asset_protocol_scope()
            .allow_directory(output, true)
            .map_err(|e| format!("Cannot enable file previews for this output folder: {e}"))?;
    }
    Ok(())
}

fn output_root(app: &tauri::AppHandle, state: &AppState) -> Result<PathBuf, String> {
    let output = settings(state)?.output;
    allow_output(app, &output)?;
    Ok(output)
}

async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(work)
        .await
        .map_err(|e| format!("The background task could not finish: {e}"))?
}

#[tauri::command]
async fn get_settings(app: tauri::AppHandle) -> Result<BootInfo, String> {
    blocking(move || {
        let state = app.state::<AppState>();
        // Copy under a short lock; inspecting an offline destination must not
        // hold up other commands that only need a settings snapshot.
        let config = state
            .config
            .lock()
            .map_err(|_| "Settings are temporarily unavailable.")?
            .clone();
        let info = config.info();
        allow_output(&app, &info.settings.output)?;
        Ok(info)
    })
    .await
}

#[tauri::command]
fn inspect_runtime_libraries(
    state: State<AppState>,
    directory: Option<String>,
) -> Result<runtime_libraries::RuntimeLibraries, String> {
    let settings = settings(&state)?;
    runtime_libraries::inspect(&settings.workspace, directory.as_deref().map(Path::new))
}

#[tauri::command]
async fn inspect_run_environment(
    state: State<'_, AppState>,
    settings: Option<RunSettings>,
) -> Result<preflight::RunEnvironment, String> {
    let settings = match settings {
        Some(settings) => settings,
        None => self::settings(&state)?,
    };
    blocking(move || Ok(preflight::inspect(&settings))).await
}

#[tauri::command]
async fn save_settings(app: tauri::AppHandle, settings: RunSettings) -> Result<BootInfo, String> {
    blocking(move || {
        let runner = app.state::<RunnerService>();
        if matches!(
            runner.status().status.as_str(),
            "running" | "starting" | "cancelling" | "queued"
        ) {
            return Err("Wait for the current run to finish before changing folders.".into());
        }
        let state = app.state::<AppState>();
        let mut config = state
            .config
            .lock()
            .map_err(|_| "Settings are temporarily unavailable.")?
            .clone();
        let old_settings = config.snapshot();
        // Validation, persistence, volume queries and preview scope setup all
        // happen off the UI thread and outside the shared configuration lock.
        let info = config.save(settings)?;
        allow_output(&app, &info.settings.output)?;
        *state
            .config
            .lock()
            .map_err(|_| "Settings are temporarily unavailable.")? = config;
        if old_settings.cache_path != info.settings.cache_path
            || old_settings.workspace != info.settings.workspace
        {
            app.state::<cache_browser::CacheBrowserService>()
                .invalidate();
            let _ = app.state::<cache_preview::PreviewService>().cancel();
        }
        Ok(info)
    })
    .await
}

#[tauri::command]
async fn select_folder(initial: Option<String>) -> Result<Option<String>, String> {
    blocking(move || {
        let mut dialog = rfd::FileDialog::new().set_title("Choose a folder");
        if let Some(initial) = initial.filter(|value| !value.trim().is_empty()) {
            let path = PathBuf::from(initial);
            if path.is_dir() {
                dialog = dialog.set_directory(path);
            } else if let Some(parent) = path.parent().filter(|p| p.is_dir()) {
                dialog = dialog.set_directory(parent);
            }
        }
        Ok(dialog
            .pick_folder()
            .map(|path| path.to_string_lossy().into_owned()))
    })
    .await
}

#[tauri::command]
fn plan_run(state: State<AppState>, tasks: Vec<TaskId>) -> Result<RunPlan, String> {
    tasks::plan(&settings(&state)?, &tasks)
}

#[tauri::command]
async fn start_run(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    runner: State<'_, RunnerService>,
    tasks: Vec<TaskId>,
) -> Result<RunPlan, diagnostics::RunFailure> {
    let settings = settings(&state)
        .map_err(|error| diagnostics::RunFailure::from_error(error, "Prepare extraction"))?;
    let selected = tasks.clone();
    let settings = tauri::async_runtime::spawn_blocking(move || {
        let settings = tasks::resolved(&settings)
            .map_err(|error| diagnostics::RunFailure::from_error(error, "Prepare extraction"))?;
        tasks::plan(&settings, &selected)
            .map_err(|error| diagnostics::RunFailure::from_error(error, "Prepare extraction"))?;
        Ok::<_, diagnostics::RunFailure>(settings)
    })
    .await
    .map_err(|error| {
        diagnostics::RunFailure::from_error(error.to_string(), "Prepare extraction")
    })??;
    runner
        .start(app, settings, tasks)
        .map_err(|error| diagnostics::RunFailure::from_error(error, "Start extraction"))
}

#[tauri::command]
fn cancel_run(app: tauri::AppHandle, runner: State<RunnerService>) -> Result<(), String> {
    use tauri::Emitter;
    runner.cancel()?;
    // A cache/storage check may still be waiting on the filesystem. Report
    // Cancelling immediately, while the managed preparation observes the flag
    // before it is allowed to start any extraction process.
    let _ = app.emit(
        "run-event",
        runner::RunEvent {
            kind: "status".into(),
            status: runner.status(),
            line: None,
            stream: None,
        },
    );
    Ok(())
}

#[tauri::command]
fn run_status(runner: State<RunnerService>) -> RunStatus {
    runner.status()
}

#[tauri::command]
async fn cache_browse(
    app: tauri::AppHandle,
    request: cache_browser::BrowseRequest,
) -> Result<cache_browser::BrowseResult, String> {
    blocking(move || {
        let settings = settings(&app.state::<AppState>())?;
        app.state::<cache_browser::CacheBrowserService>()
            .browse(&settings, request)
    })
    .await
}

#[tauri::command]
fn cache_cancel_browse(browser: State<cache_browser::CacheBrowserService>) {
    browser.cancel_scan();
}

#[tauri::command]
async fn cache_select(
    app: tauri::AppHandle,
    request: cache_browser::SelectionRequest,
) -> Result<cache_browser::SelectionSummary, String> {
    blocking(move || {
        let settings = settings(&app.state::<AppState>())?;
        app.state::<cache_browser::CacheBrowserService>()
            .select(&settings, request)
    })
    .await
}

#[tauri::command]
async fn cache_extract(
    app: tauri::AppHandle,
    generation: u64,
    image_format: String,
    all: Option<bool>,
) -> Result<RunPlan, diagnostics::RunFailure> {
    let error = |e| diagnostics::RunFailure::from_error(e, "Prepare cache extraction");
    if !matches!(image_format.as_str(), "png" | "dds") {
        return Err(error("Choose PNG or DDS image output.".into()));
    }
    let (settings, selection) = blocking({
        let app = app.clone();
        move || {
            let settings = settings(&app.state::<AppState>())?;
            app.state::<cache_browser::CacheBrowserService>()
                .prepare_extraction(&settings, generation, all.unwrap_or(false))
        }
    })
    .await
    .map_err(error)?;
    let owned = cache_browser::OwnedManifest::planned(&settings.workspace);
    let plan = cache_browser::extraction_plan(
        &owned.path,
        selection.summary.images > 0,
        selection.summary.audio > 0,
        selection.summary.models > 0,
        selection.summary.count,
    );
    let prepare_owned = owned.clone();
    app.state::<RunnerService>()
        .start_prepared(
            app.clone(),
            settings,
            plan,
            move |settings, plan| {
                let settings = preflight::prepare(settings, plan)?;
                let manifest = selection
                    .manifest(&settings.output, &image_format)
                    .map_err(|e| {
                        diagnostics::RunFailure::from_error(e, "Prepare media selection")
                    })?;
                prepare_owned
                    .create(&settings.workspace, &settings.output)
                    .map_err(|e| {
                        diagnostics::RunFailure::from_error(e, "Prepare cache extraction")
                    })?;
                cache_browser::write_manifest_at(&prepare_owned.path, &manifest)
                    .map_err(|e| diagnostics::RunFailure::from_error(e, "Write media selection"))?;
                Ok(settings)
            },
            move |outcome| {
                if outcome == "failed" {
                    owned.exists().then(|| {
                        format!(
                            "Selection manifest retained for diagnostics: {}",
                            owned.path.display()
                        )
                    })
                } else {
                    owned.cleanup().err()
                }
            },
        )
        .map_err(error)
}

#[tauri::command]
async fn cache_preview(
    app: tauri::AppHandle,
    generation: u64,
    id: String,
) -> Result<cache_preview::CachePreview, String> {
    blocking(move || {
        let settings = settings(&app.state::<AppState>())?;
        let lookup = app
            .state::<cache_browser::CacheBrowserService>()
            .lookup(&settings, generation, &id)?;
        app.state::<cache_preview::PreviewService>().preview(
            &app,
            &settings,
            &lookup.entry,
            &lookup.cache_path,
            generation,
        )
    })
    .await
}

#[tauri::command]
fn cache_cancel_preview(previews: State<cache_preview::PreviewService>) -> Result<(), String> {
    previews.cancel()
}

#[tauri::command]
async fn list_directory(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    relative: String,
) -> Result<Vec<LibraryEntry>, String> {
    let root = output_root(&app, &state)?;
    blocking(move || library::list_directory(&root, &relative)).await
}

#[tauri::command]
async fn search_files(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    query: String,
    limit: Option<usize>,
) -> Result<Vec<LibraryEntry>, String> {
    let root = output_root(&app, &state)?;
    blocking(move || library::search(&root, &query, limit.unwrap_or(500))).await
}

#[tauri::command]
async fn preview_file(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    relative: String,
) -> Result<FilePreview, String> {
    let root = output_root(&app, &state)?;
    blocking(move || library::preview(&root, &relative)).await
}

#[tauri::command]
async fn library_stats(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<LibraryStats, String> {
    let root = output_root(&app, &state)?;
    blocking(move || library::stats(&root)).await
}

#[tauri::command]
async fn open_file(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    relative: String,
) -> Result<(), String> {
    let root = output_root(&app, &state)?;
    blocking(move || library::open_file(&root, &relative)).await
}

#[tauri::command]
async fn reveal_file(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    relative: String,
) -> Result<(), String> {
    let root = output_root(&app, &state)?;
    blocking(move || library::reveal_file(&root, &relative)).await
}

#[tauri::command]
async fn open_output(app: tauri::AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    let root = output_root(&app, &state)?;
    blocking(move || library::open_file(&root, "")).await
}

fn main() {
    if let Some(code) = worker::run_if_requested(std::env::args_os()) {
        std::process::exit(code);
    }

    let result = tauri::Builder::default()
        .manage(RunnerService::new())
        .manage(cache_browser::CacheBrowserService::default())
        .manage(cache_preview::PreviewService::default())
        .setup(|app| {
            #[cfg(windows)]
            if let Some(window) = app.get_webview_window("main") {
                let icon = taskbar_icon::install(&window).map_err(std::io::Error::other)?;
                app.manage(icon);
            }
            let config = ConfigStore::load(app.handle());
            allow_output(app.handle(), &config.snapshot().output).map_err(std::io::Error::other)?;
            app.manage(AppState {
                config: Mutex::new(config),
            });
            Ok(())
        })
        .on_window_event(|window, event| {
            if matches!(event, tauri::WindowEvent::CloseRequested { .. }) {
                if let Some(runner) = window.try_state::<RunnerService>() {
                    let _ = runner.cancel();
                }
                if let Some(browser) = window.try_state::<cache_browser::CacheBrowserService>() {
                    browser.cancel_scan();
                }
                if let Some(previews) = window.try_state::<cache_preview::PreviewService>() {
                    let _ = previews.cancel();
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            get_settings,
            inspect_runtime_libraries,
            inspect_run_environment,
            save_settings,
            select_folder,
            plan_run,
            start_run,
            cancel_run,
            run_status,
            cache_browse,
            cache_cancel_browse,
            cache_select,
            cache_extract,
            cache_preview,
            cache_cancel_preview,
            list_directory,
            search_files,
            preview_file,
            library_stats,
            open_file,
            reveal_file,
            open_output,
        ])
        .build(tauri::generate_context!())
        .map(|app| {
            // Tauri exits the process directly after this event, so managed
            // state Drop implementations alone cannot clean preview files.
            app.run(|app, event| {
                if matches!(event, tauri::RunEvent::Exit) {
                    let _ = app.state::<RunnerService>().cancel();
                    app.state::<cache_browser::CacheBrowserService>()
                        .cancel_scan();
                    app.state::<cache_preview::PreviewService>().shutdown();
                }
            });
        });
    if let Err(error) = result {
        eprintln!("Soulkit could not start: {error}");
        rfd::MessageDialog::new()
            .set_title("Soulkit could not start")
            .set_description(error.to_string())
            .set_level(rfd::MessageLevel::Error)
            .set_buttons(rfd::MessageButtons::Ok)
            .show();
        std::process::exit(1);
    }
}
