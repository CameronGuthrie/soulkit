//! Presence checks and per-worker configuration for the selected native DLLs.

use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::Command;

pub const DIRECTORY_ENV: &str = "SOULKIT_DLL_DIRECTORY";
const DLL_NAMES: [&str; 3] = ["oo2core_9.dll", "libzstd.dll", "vcruntime140.dll"];

#[derive(Debug, Clone, Serialize)]
pub struct RuntimeLibrary {
    pub name: &'static str,
    pub path: PathBuf,
    pub found: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct RuntimeLibraries {
    pub directory: PathBuf,
    pub custom: bool,
    pub files: Vec<RuntimeLibrary>,
}

pub fn resolve_directory(workspace: &Path, directory: Option<&Path>) -> Result<PathBuf, String> {
    let app_directory = std::env::current_exe()
        .map_err(|e| format!("Cannot locate the application executable: {e}"))?
        .parent()
        .ok_or("Cannot locate the application's DLL folder.")?
        .to_path_buf();
    resolve_with_default(workspace, directory, &app_directory)
}

fn resolve_with_default(
    workspace: &Path,
    directory: Option<&Path>,
    default_directory: &Path,
) -> Result<PathBuf, String> {
    let directory = directory
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(default_directory);
    let resolved = crate::config::absolute_path(workspace, directory)?;
    if resolved.exists() && !resolved.is_dir() {
        return Err("The DLL location is a file. Choose a folder instead.".into());
    }
    Ok(crate::config::readable_path(resolved))
}

fn report(directory: PathBuf, custom: bool) -> RuntimeLibraries {
    let files = DLL_NAMES
        .into_iter()
        .map(|name| {
            let path = directory.join(name);
            RuntimeLibrary {
                name,
                found: path.is_file(),
                path,
            }
        })
        .collect();
    RuntimeLibraries {
        directory,
        custom,
        files,
    }
}

/// Inspects file presence only. Native code is never loaded into the GUI.
pub fn inspect(workspace: &Path, directory: Option<&Path>) -> Result<RuntimeLibraries, String> {
    Ok(report(
        resolve_directory(workspace, directory)?,
        directory.is_some_and(|path| !path.as_os_str().is_empty()),
    ))
}

/// A saved folder can disappear or become unreadable after validation. Keep
/// reporting that configured location rather than silently showing a fallback.
pub fn for_settings(workspace: &Path, directory: Option<&Path>) -> RuntimeLibraries {
    let custom = directory.is_some_and(|path| !path.as_os_str().is_empty());
    let resolved = resolve_directory(workspace, directory).unwrap_or_else(|_| {
        let path = directory.filter(|path| !path.as_os_str().is_empty());
        match path {
            Some(path) if path.is_absolute() => path.to_path_buf(),
            Some(path) => workspace.join(path),
            None => std::env::current_exe()
                .ok()
                .and_then(|path| path.parent().map(Path::to_path_buf))
                .unwrap_or_else(|| workspace.to_path_buf()),
        }
    });
    report(resolved, custom)
}

pub fn configure_worker(
    command: &mut Command,
    workspace: &Path,
    directory: Option<&Path>,
) -> Result<(), String> {
    let directory = resolve_directory(workspace, directory)?;
    // These overrides belong to the child only. Pin the exact folder checked
    // in Settings rather than inheriting unrelated per-library overrides.
    command
        .env(DIRECTORY_ENV, &directory)
        .env("SOULFRAME_OODLE_LIB", directory.join("oo2core_9.dll"))
        .env("SOULFRAME_ZSTD_LIB", directory.join("libzstd.dll"));
    Ok(())
}

/// Called before tool code, only inside the isolated extraction worker.
pub fn initialize_worker() -> Result<(), String> {
    let Some(directory) = std::env::var_os(DIRECTORY_ENV).map(PathBuf::from) else {
        return Ok(());
    };
    if !directory.is_absolute() {
        return Err(format!(
            "The configured DLL folder must be an absolute path: {}.",
            directory.display()
        ));
    }
    // Calendar tasks need no native compression libraries. Missing
    // folders are reported in Settings; exact per-library overrides fail only
    // if a selected tool actually needs to load one of those libraries.
    if !directory.is_dir() {
        return Ok(());
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        #[link(name = "kernel32")]
        extern "system" {
            fn SetDllDirectoryW(path: *const u16) -> i32;
        }
        let wide: Vec<u16> = directory.as_os_str().encode_wide().chain(Some(0)).collect();
        // The chosen folder also contains dependencies such as the VC runtime.
        // This changes only this worker; the Tauri process is unaffected.
        if unsafe { SetDllDirectoryW(wide.as_ptr()) } == 0 {
            return Err(format!(
                "Cannot enable DLL dependency lookup in {}: {}",
                directory.display(),
                std::io::Error::last_os_error()
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> PathBuf {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("target")
            .join("runtime-library-tests")
            .join(format!(
                "{}-{}",
                std::process::id(),
                chrono::Utc::now().timestamp_nanos_opt().unwrap()
            ));
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn reports_presence_and_missing_names_without_loading_native_code() {
        let root = fixture();
        std::fs::write(root.join("oo2core_9.dll"), b"presence fixture, not a DLL").unwrap();
        std::fs::create_dir(root.join("libzstd.dll")).unwrap();
        let result = inspect(&root, Some(&root)).unwrap();
        assert!(result.custom);
        assert!(result.files[0].found);
        assert!(!result.files[1].found, "a directory is not a DLL file");
        assert!(!result.files[2].found);
        assert_eq!(result.files[2].path, root.join("vcruntime140.dll"));
    }

    #[test]
    fn defaults_to_app_folder_and_resolves_custom_relative_folders() {
        let root = fixture();
        let default = root.join("app");
        assert_eq!(
            resolve_with_default(&root, None, &default).unwrap(),
            default
        );
        let custom = resolve_with_default(&root, Some(Path::new("native libs")), &default).unwrap();
        assert_eq!(custom, root.join("native libs"));
        let missing = report(custom.clone(), true);
        assert!(missing.files.iter().all(|file| !file.found));
        assert!(!custom.exists(), "inspection must not create folders");
    }

    #[test]
    fn configures_child_library_paths_without_changing_parent_environment() {
        let root = fixture();
        let original = std::env::var_os(DIRECTORY_ENV);
        let mut command = Command::new("unused.exe");
        configure_worker(&mut command, &root, Some(&root)).unwrap();
        let environment = command
            .get_envs()
            .collect::<std::collections::BTreeMap<_, _>>();
        assert_eq!(
            environment[std::ffi::OsStr::new(DIRECTORY_ENV)],
            Some(root.as_os_str())
        );
        assert_eq!(
            environment[std::ffi::OsStr::new("SOULFRAME_OODLE_LIB")],
            Some(root.join("oo2core_9.dll").as_os_str())
        );
        assert_eq!(std::env::var_os(DIRECTORY_ENV), original);
    }
}
