//! Read-only checks for the game cache and the volumes used by an extraction.

use crate::config::{absolute_path, readable_path};
use crate::diagnostics::RunFailure;
use crate::tasks::{RunPlan, RunSettings, TaskId};
use serde::Serialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize)]
pub struct CacheReport {
    pub path: Option<PathBuf>,
    pub found: bool,
    pub custom: bool,
    pub error: Option<String>,
    pub error_kind: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct StorageReport {
    /// Configured destination, which need not exist yet.
    pub path: PathBuf,
    /// Existing ancestor used to query the volume, without creating any files.
    pub checked_path: Option<PathBuf>,
    pub available_bytes: Option<u64>,
    pub total_bytes: Option<u64>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RunEnvironment {
    pub cache: CacheReport,
    pub output_storage: StorageReport,
    pub scratch_storage: StorageReport,
}

fn default_cache_path() -> Option<PathBuf> {
    soulframe_cache::detect_soulframe_cache().or_else(|| {
        if let Some(root) = std::env::var_os("SOULFRAME_ROOT") {
            Some(PathBuf::from(root).join("Cache.Windows"))
        } else {
            std::env::var_os("LOCALAPPDATA")
                .map(|root| PathBuf::from(root).join("Soulframe/Downloaded/Public/Cache.Windows"))
        }
    })
}

fn cache_candidates(selection: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if selection
        .file_name()
        .is_some_and(|name| name.to_string_lossy().eq_ignore_ascii_case("Cache.Windows"))
    {
        paths.push(selection.to_path_buf());
    }
    paths.extend([
        selection.join("Cache.Windows"),
        selection.join("Public/Cache.Windows"),
        selection.join("Downloaded/Public/Cache.Windows"),
        selection.join("Soulframe/Downloaded/Public/Cache.Windows"),
    ]);
    paths
}

/// A named but empty cache folder is not a usable installation. Check for a
/// nonempty TOC and matching cache body; decoding belongs to the selected tool.
fn has_cache_pair(directory: &Path) -> std::io::Result<bool> {
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        if !path
            .extension()
            .is_some_and(|extension| extension.to_string_lossy().eq_ignore_ascii_case("toc"))
        {
            continue;
        }
        let toc = entry.metadata()?;
        if !toc.is_file() || toc.len() == 0 {
            continue;
        }
        match std::fs::metadata(path.with_extension("cache")) {
            Ok(cache) if cache.is_file() && cache.len() > 0 => return Ok(true),
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(false)
}

fn inspect_cache_at(selection: Option<PathBuf>, custom: bool) -> CacheReport {
    let Some(selection) = selection else {
        return CacheReport {
            path: None,
            found: false,
            custom,
            error: Some("No Soulframe installation was detected.".into()),
            error_kind: Some("cache_missing".into()),
        };
    };
    let candidates = cache_candidates(&selection);
    let mut expected = candidates.first().cloned().unwrap_or(selection);
    let mut problem = "The game cache folder does not exist.".to_string();
    let mut kind = "cache_missing";
    for directory in candidates {
        match has_cache_pair(&directory) {
            Ok(true) => {
                return CacheReport {
                    path: Some(readable_path(directory)),
                    found: true,
                    custom,
                    error: None,
                    error_kind: None,
                };
            }
            Ok(false) => {
                expected = directory;
                problem = "This folder has no nonempty matching .toc and .cache files. Check that Soulframe has finished downloading.".into();
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                expected = directory;
                kind = if error.kind() == std::io::ErrorKind::PermissionDenied {
                    "permission_denied"
                } else {
                    "cache_missing"
                };
                problem = format!("Cannot inspect the game cache: {error}");
            }
        }
    }
    CacheReport {
        path: Some(readable_path(expected)),
        found: false,
        custom,
        error: Some(problem),
        error_kind: Some(kind.into()),
    }
}

pub fn inspect_cache(settings: &RunSettings) -> CacheReport {
    let custom = settings.cache_path.is_some();
    let selected = settings
        .cache_path
        .as_ref()
        .map(|path| {
            if path.is_absolute() {
                path.clone()
            } else {
                settings.workspace.join(path)
            }
        })
        .or_else(default_cache_path);
    inspect_cache_at(selected, custom)
}

fn existing_directory(path: &Path) -> std::io::Result<PathBuf> {
    let mut candidate = path;
    loop {
        match std::fs::metadata(candidate) {
            Ok(metadata) if metadata.is_dir() => return Ok(candidate.to_path_buf()),
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        candidate = candidate.parent().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "No existing destination folder was found",
            )
        })?;
    }
}

#[cfg(windows)]
fn volume_space(directory: &Path) -> std::io::Result<(u64, u64)> {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "kernel32")]
    extern "system" {
        fn GetDiskFreeSpaceExW(
            directory: *const u16,
            available: *mut u64,
            total: *mut u64,
            free: *mut u64,
        ) -> i32;
    }
    let mut wide: Vec<u16> = directory.as_os_str().encode_wide().collect();
    if wide.last().copied() != Some(u16::from(b'\\')) {
        wide.push(u16::from(b'\\'));
    }
    wide.push(0);
    let mut available = 0;
    let mut total = 0;
    let mut free = 0;
    if unsafe { GetDiskFreeSpaceExW(wide.as_ptr(), &mut available, &mut total, &mut free) } == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok((available, total))
    }
}

#[cfg(not(windows))]
fn volume_space(_: &Path) -> std::io::Result<(u64, u64)> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "Free-space reporting is unavailable on this platform",
    ))
}

pub fn inspect_storage(path: PathBuf) -> StorageReport {
    let mut report = StorageReport {
        path,
        checked_path: None,
        available_bytes: None,
        total_bytes: None,
        error: None,
    };
    let result = (|| {
        let existing = existing_directory(&report.path)?;
        report.checked_path = Some(readable_path(existing.clone()));
        volume_space(&existing)
    })();
    match result {
        Ok((available, total)) => {
            report.available_bytes = Some(available);
            report.total_bytes = Some(total);
        }
        Err(error) => report.error = Some(error.to_string()),
    }
    report
}

/// Draft settings may contain relative destinations. Resolve them read-only,
/// without validating unrelated fields or creating a destination folder.
pub fn inspect(settings: &RunSettings) -> RunEnvironment {
    let base = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let workspace = absolute_path(&base, &settings.workspace)
        .unwrap_or_else(|_| base.join(&settings.workspace));
    let output = absolute_path(&workspace, &settings.output)
        .unwrap_or_else(|_| workspace.join(&settings.output));
    let mut normalized = settings.clone();
    normalized.workspace = workspace.clone();
    RunEnvironment {
        cache: inspect_cache(&normalized),
        output_storage: inspect_storage(output),
        scratch_storage: inspect_storage(workspace.join(".soulkit-work")),
    }
}

fn needs_local_cache(plan: &RunPlan) -> bool {
    plan.modules.iter().any(|task| {
        matches!(
            task,
            TaskId::Images | TaskId::Audio | TaskId::Models | TaskId::Maps | TaskId::Items
        )
    })
}

fn storage_failure(report: &StorageReport, scratch: bool) -> Option<RunFailure> {
    // We cannot predict an extraction's final size. Block only the definite
    // zero-space case, rather than inventing a minimum requirement.
    (report.available_bytes == Some(0)).then(|| RunFailure {
        kind: "disk_full".into(),
        title: "No space available".into(),
        message: if scratch {
            "The workspace drive has no space available for temporary extraction files."
        } else {
            "The output drive has no space available for extracted files."
        }
        .into(),
        details: vec![format!("Folder: {}", report.path.display())],
        suggestion: if scratch {
            "Free space on the workspace drive or choose a workspace folder on another drive in Settings."
        } else {
            "Free space on the output drive or choose an output folder on another drive in Settings."
        }
        .into(),
        action: Some("open_output_settings".into()),
        exit_code: None,
    })
}

pub fn prepare(mut settings: RunSettings, plan: &RunPlan) -> Result<RunSettings, RunFailure> {
    let environment = inspect(&settings);
    if needs_local_cache(plan) {
        let cache = environment.cache;
        if !cache.found {
            let denied = cache.error_kind.as_deref() == Some("permission_denied");
            let mut details = Vec::new();
            if let Some(path) = &cache.path {
                details.push(format!("Checked: {}", path.display()));
            }
            if let Some(error) = cache.error {
                details.push(error);
            }
            return Err(RunFailure {
                kind: if denied { "permission_denied" } else { "cache_missing" }.into(),
                title: if denied { "Game cache cannot be read" } else { "Game cache not found" }.into(),
                message: "The selected tools need the installed Soulframe cache before extraction can start.".into(),
                details,
                suggestion: "Open Settings > Game cache folder and choose Cache.Windows, its parent Public folder, or the Soulframe installation folder. Make sure the game has finished downloading.".into(),
                action: Some("open_cache_settings".into()),
                exit_code: None,
            });
        }
        // Use exactly the cache validated above rather than detecting a different
        // installation later in the worker's environment.
        settings.cache_path = cache.path;
    }
    if let Some(failure) = storage_failure(&environment.output_storage, false) {
        return Err(failure);
    }
    if plan.jobs.iter().any(|job| {
        job.args
            .first()
            .is_some_and(|arg| matches!(arg.as_str(), "run-all" | "cache-extract"))
    }) {
        if let Some(failure) = storage_failure(&environment.scratch_storage, true) {
            return Err(failure);
        }
    }
    Ok(settings)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tasks::Job;

    fn fixture() -> PathBuf {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("target/preflight-tests")
            .join(format!(
                "{}-{}",
                std::process::id(),
                chrono::Utc::now().timestamp_nanos_opt().unwrap()
            ));
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    fn settings(root: &Path) -> RunSettings {
        RunSettings {
            workspace: root.to_path_buf(),
            output: root.join("output"),
            soulkit: std::env::current_exe().unwrap(),
            cache_path: Some(root.join("missing/Cache.Windows")),
            dll_directory: None,
            locales: "en".into(),
            keep_intermediates: false,
        }
    }

    fn plan(modules: Vec<TaskId>) -> RunPlan {
        RunPlan {
            requested: modules.clone(),
            modules,
            dependencies: vec![],
            jobs: vec![Job {
                id: "fixture".into(),
                label: "Fixture".into(),
                args: vec!["zenith-calendar".into()],
            }],
        }
    }

    #[test]
    fn rejects_an_empty_cache_and_reports_the_selected_location_before_writing() {
        let root = fixture();
        let cache = root.join("Cache.Windows");
        std::fs::create_dir(&cache).unwrap();
        let report = inspect_cache_at(Some(cache.clone()), true);
        assert!(!report.found);
        assert_eq!(report.path, Some(cache.clone()));
        let mut settings = settings(&root);
        settings.cache_path = Some(cache);
        let failure = prepare(settings, &plan(vec![TaskId::Audio])).unwrap_err();
        assert_eq!(failure.kind, "cache_missing");
        assert_eq!(failure.action.as_deref(), Some("open_cache_settings"));
        assert!(!root.join("output").exists());
        assert!(!root.join(".soulkit-work").exists());
    }

    #[test]
    fn model_extraction_requires_the_selected_cache_before_writing() {
        let root = fixture();
        let failure = prepare(settings(&root), &plan(vec![TaskId::Models])).unwrap_err();
        assert_eq!(failure.kind, "cache_missing");
        assert_eq!(failure.action.as_deref(), Some("open_cache_settings"));
        assert!(!root.join("output").exists());
    }

    #[test]
    fn accepts_game_root_and_pins_a_nonempty_matching_cache_pair() {
        let root = fixture();
        let cache = root.join("Downloaded/Public/Cache.Windows");
        std::fs::create_dir_all(&cache).unwrap();
        std::fs::write(cache.join("H.Misc.toc"), b"toc").unwrap();
        let incomplete = inspect_cache_at(Some(root.clone()), true);
        assert!(!incomplete.found);
        std::fs::write(cache.join("H.Misc.cache"), b"cache").unwrap();
        let mut settings = settings(&root);
        settings.cache_path = Some(root.clone());
        let settings = prepare(settings, &plan(vec![TaskId::Images])).unwrap();
        assert_eq!(settings.cache_path, Some(cache));
    }

    #[test]
    fn cache_free_tools_and_calendars_with_existing_inputs_remain_available() {
        let root = fixture();
        for modules in [vec![TaskId::Languages], vec![TaskId::Calendars]] {
            assert!(prepare(settings(&root), &plan(modules)).is_ok());
        }
        let dependency_plan = plan(vec![TaskId::Calendars, TaskId::Maps]);
        assert_eq!(
            prepare(settings(&root), &dependency_plan).unwrap_err().kind,
            "cache_missing"
        );
    }

    #[test]
    fn storage_lookup_uses_existing_ancestor_without_creating_output() {
        let root = fixture();
        let destination = root.join("future/nested/output");
        let report = inspect_storage(destination.clone());
        assert_eq!(report.checked_path, Some(root));
        assert!(!destination.exists());
        #[cfg(windows)]
        {
            assert!(report.available_bytes.is_some(), "{:?}", report.error);
            assert!(report.total_bytes.unwrap() >= report.available_bytes.unwrap());
        }
    }

    #[test]
    fn only_definite_zero_space_blocks_and_unknown_space_is_not_a_fake_estimate() {
        let mut report = StorageReport {
            path: "output".into(),
            checked_path: None,
            available_bytes: None,
            total_bytes: None,
            error: Some("unavailable".into()),
        };
        assert!(storage_failure(&report, false).is_none());
        report.available_bytes = Some(1);
        assert!(storage_failure(&report, false).is_none());
        report.available_bytes = Some(0);
        assert_eq!(storage_failure(&report, false).unwrap().kind, "disk_full");
        assert!(storage_failure(&report, true)
            .unwrap()
            .message
            .contains("temporary"));
    }
}
