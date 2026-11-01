use crate::runtime_libraries::{self, RuntimeLibraries};
use crate::tasks::RunSettings;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Component, Path, PathBuf};
use tauri::Manager;

const MAX_SETTINGS_BYTES: u64 = 64 * 1024;

#[derive(Clone, Serialize)]
pub struct BootInfo {
    pub settings: RunSettings,
    pub version: &'static str,
    pub game_found: bool,
    pub cache_path: Option<PathBuf>,
    pub soulkit_found: bool,
    pub settings_warning: Option<String>,
    pub runtime_libraries: RuntimeLibraries,
    pub run_environment: crate::preflight::RunEnvironment,
}

#[derive(Serialize, Deserialize)]
struct SavedSettings {
    version: u32,
    settings: RunSettings,
}

#[derive(Clone)]
pub struct ConfigStore {
    path: Option<PathBuf>,
    settings: RunSettings,
    warning: Option<String>,
}

impl ConfigStore {
    pub fn load(app: &tauri::AppHandle) -> Self {
        let path = app
            .path()
            .app_config_dir()
            .ok()
            .map(|directory| directory.join("settings.json"));
        let mut store = Self {
            path,
            settings: default_settings(),
            warning: None,
        };
        if let Some(path) = &store.path {
            if path.exists() {
                let result = (|| -> Result<RunSettings, String> {
                    let length = fs::metadata(path).map_err(|e| e.to_string())?.len();
                    if length > MAX_SETTINGS_BYTES {
                        return Err("The saved settings file is too large.".into());
                    }
                    let saved: SavedSettings =
                        serde_json::from_slice(&fs::read(path).map_err(|e| e.to_string())?)
                            .map_err(|e| e.to_string())?;
                    if saved.version != 1 {
                        return Err("The saved settings version is not supported.".into());
                    }
                    validate_settings(saved.settings)
                })();
                match result {
                    Ok(settings) => store.settings = settings,
                    Err(error) => {
                        store.warning = Some(format!(
                            "Saved settings could not be loaded; using defaults. {error}"
                        ));
                    }
                }
            }
        } else {
            store.warning = Some(
                "The application settings folder is unavailable. Settings will only last for this session."
                    .into(),
            );
        }
        store
    }

    pub fn snapshot(&self) -> RunSettings {
        self.settings.clone()
    }

    pub fn info(&self) -> BootInfo {
        let run_environment = crate::preflight::inspect(&self.settings);
        let cache_path = run_environment.cache.path.clone();
        BootInfo {
            settings: self.settings.clone(),
            version: env!("CARGO_PKG_VERSION"),
            game_found: run_environment.cache.found,
            cache_path,
            soulkit_found: std::env::current_exe().is_ok_and(|path| path.is_file()),
            settings_warning: self.warning.clone(),
            runtime_libraries: runtime_libraries::for_settings(
                &self.settings.workspace,
                self.settings.dll_directory.as_deref(),
            ),
            run_environment,
        }
    }

    pub fn save(&mut self, settings: RunSettings) -> Result<BootInfo, String> {
        let settings = validate_settings(settings)?;
        if let Some(path) = &self.path {
            let parent = path
                .parent()
                .ok_or("The settings folder could not be resolved.")?;
            fs::create_dir_all(parent)
                .map_err(|e| format!("Cannot create the settings folder: {e}"))?;
            let bytes = serde_json::to_vec_pretty(&SavedSettings {
                version: 1,
                settings: settings.clone(),
            })
            .map_err(|e| e.to_string())?;
            let temporary = path.with_extension("json.tmp");
            fs::write(&temporary, bytes)
                .map_err(|e| format!("Cannot write the settings file: {e}"))?;
            fs::rename(&temporary, path)
                .map_err(|e| format!("Cannot replace the settings file: {e}"))?;
        }
        self.settings = settings;
        self.warning = self.path.is_none().then(|| {
            "Settings are available for this session, but no settings folder is available for persistence."
                .into()
        });
        Ok(self.info())
    }
}

fn is_workspace(path: &Path) -> bool {
    path.join("Cargo.toml").is_file() && path.join("soulkit-cli/Cargo.toml").is_file()
}

fn detect_workspace() -> PathBuf {
    let cwd = std::env::current_dir().ok();
    let executable = std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(Path::to_path_buf));
    for seed in cwd.iter().chain(executable.iter()) {
        if let Some(root) = seed.ancestors().find(|path| is_workspace(path)) {
            return root.to_path_buf();
        }
    }
    cwd.filter(|path| path.join("output").is_dir())
        .or(executable)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn default_settings() -> RunSettings {
    let workspace = detect_workspace();
    // The GUI contains the same extraction library as the standalone CLI.
    let soulkit = std::env::current_exe().unwrap_or_default();
    RunSettings {
        output: workspace.join("output"),
        workspace,
        soulkit,
        cache_path: None,
        dll_directory: None,
        locales: "en,fr,de,es,it,pt,ru,pl,tr,ja,ko,zh".into(),
        keep_intermediates: false,
    }
}

/// Resolve existing prefixes, then retain nonexistent output components.
pub(crate) fn absolute_path(base: &Path, path: &Path) -> Result<PathBuf, String> {
    if path.as_os_str().is_empty() {
        return Err("A folder or executable path is empty.".into());
    }
    if !path.is_absolute()
        && (path.has_root() || path.components().any(|c| matches!(c, Component::Prefix(_))))
    {
        return Err("Use a fully qualified path or a path relative to the workspace.".into());
    }
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    };
    let mut resolved = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::Prefix(prefix) => resolved.push(prefix.as_os_str()),
            Component::RootDir => resolved.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                resolved.pop();
            }
            Component::Normal(name) => {
                resolved.push(name);
                if resolved.exists() {
                    resolved = fs::canonicalize(&resolved)
                        .map_err(|e| format!("Cannot resolve {}: {e}", resolved.display()))?;
                }
            }
        }
    }
    Ok(resolved)
}

pub fn validate_settings(mut settings: RunSettings) -> Result<RunSettings, String> {
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    settings.workspace = absolute_path(&cwd, &settings.workspace)?;
    if !settings.workspace.is_dir() {
        return Err("The workspace folder does not exist.".into());
    }
    settings.output = absolute_path(&settings.workspace, &settings.output)?;
    if settings.output.exists() && !settings.output.is_dir() {
        return Err("The output path points to a file. Choose a folder.".into());
    }
    // Ignore a previously saved external CLI path. Runs always use this app.
    settings.soulkit = std::env::current_exe()
        .map_err(|e| format!("Cannot locate the application executable: {e}"))?;
    if let Some(cache) = settings.cache_path.filter(|p| !p.as_os_str().is_empty()) {
        let cache = absolute_path(&settings.workspace, &cache)?;
        if cache.exists() && !cache.is_dir() {
            return Err("The game cache location is a file. Choose a folder.".into());
        }
        settings.cache_path = Some(cache);
    } else {
        settings.cache_path = None;
    }
    settings.dll_directory = match settings
        .dll_directory
        .filter(|path| !path.as_os_str().is_empty())
    {
        Some(directory) => {
            let directory = absolute_path(&settings.workspace, &directory)?;
            if directory.exists() && !directory.is_dir() {
                return Err("The DLL location is a file. Choose a folder instead.".into());
            }
            Some(readable_path(directory))
        }
        None => None,
    };
    let locales: Vec<_> = settings
        .locales
        .split(',')
        .map(str::trim)
        .filter(|locale| !locale.is_empty())
        .collect();
    if locales.is_empty()
        || locales.iter().any(|locale| {
            locale.len() > 24
                || !locale
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
        })
    {
        return Err("Enter one or more comma-separated language codes, such as en,fr,de.".into());
    }
    settings.locales = locales.join(",");
    settings.workspace = readable_path(settings.workspace);
    settings.output = readable_path(settings.output);
    settings.soulkit = readable_path(settings.soulkit);
    settings.cache_path = settings.cache_path.map(readable_path);
    Ok(settings)
}

pub(crate) fn readable_path(path: PathBuf) -> PathBuf {
    #[cfg(windows)]
    {
        let text = path.to_string_lossy();
        if let Some(unc) = text.strip_prefix(r"\\?\UNC\") {
            return PathBuf::from(format!(r"\\{unc}"));
        }
        if let Some(local) = text.strip_prefix(r"\\?\") {
            return PathBuf::from(local);
        }
    }
    path
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_resolve_relative_outputs_without_creating_them() {
        let base = fs::canonicalize(std::env::temp_dir()).unwrap();
        let path = absolute_path(&base, Path::new("soulkit-preview/../future-output")).unwrap();
        assert_eq!(path, base.join("future-output"));
    }

    #[test]
    fn invalid_settings_do_not_create_output_directories() {
        let mut settings = default_settings();
        settings.workspace = std::env::temp_dir();
        settings.soulkit = PathBuf::from("missing-soulkit-cli.exe");
        settings.output = PathBuf::from("not-created-by-validation");
        settings.locales = "../escape".into();
        assert!(validate_settings(settings).is_err());
    }

    #[test]
    fn old_cli_settings_migrate_to_the_embedded_application() {
        let mut settings = default_settings();
        settings.soulkit = PathBuf::from("removed-install/soulkit-cli.exe");
        let validated = validate_settings(settings).unwrap();
        assert_eq!(
            fs::canonicalize(validated.soulkit).unwrap(),
            fs::canonicalize(std::env::current_exe().unwrap()).unwrap()
        );
    }

    #[test]
    fn existing_settings_without_a_dll_folder_keep_the_app_default() {
        let mut value = serde_json::to_value(default_settings()).unwrap();
        value.as_object_mut().unwrap().remove("dll_directory");
        let settings: RunSettings = serde_json::from_value(value).unwrap();
        assert!(settings.dll_directory.is_none());
    }

    #[test]
    fn custom_dll_folder_can_be_saved_while_files_are_missing() {
        let mut settings = default_settings();
        settings.dll_directory = Some(settings.workspace.clone());
        let settings = validate_settings(settings).unwrap();
        assert!(settings.dll_directory.is_some());
        let bytes = serde_json::to_vec(&settings).unwrap();
        let restored: RunSettings = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(settings.dll_directory, restored.dll_directory);
    }

    #[test]
    fn missing_dll_folder_is_retained_without_creating_it_or_resetting_settings() {
        let mut settings = default_settings();
        let folder = settings.workspace.join(format!(
            "target/missing-dll-folder-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap()
        ));
        settings.dll_directory = Some(folder.clone());
        let output = settings.output.clone();
        let validated = validate_settings(settings).unwrap();
        assert_eq!(validated.dll_directory, Some(folder.clone()));
        assert_eq!(validated.output, output);
        assert!(!folder.exists());
        let report = runtime_libraries::for_settings(&validated.workspace, Some(&folder));
        assert!(report.custom);
        assert_eq!(report.directory, folder);
        assert!(report.files.iter().all(|file| !file.found));
    }

    #[test]
    fn missing_cache_folder_is_retained_without_resetting_the_output() {
        let mut settings = default_settings();
        let folder = settings.workspace.join(format!(
            "target/missing-cache-folder-{}-{}/Cache.Windows",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap()
        ));
        settings.cache_path = Some(folder.clone());
        let output = settings.output.clone();
        let validated = validate_settings(settings).unwrap();
        assert_eq!(validated.cache_path, Some(folder.clone()));
        assert_eq!(validated.output, output);
        assert!(!folder.exists());
        let report = crate::preflight::inspect_cache(&validated);
        assert!(!report.found);
        assert_eq!(report.path, Some(folder));
    }
}
