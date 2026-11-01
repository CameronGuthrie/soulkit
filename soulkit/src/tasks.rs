//! Side-effect-free plans for the exact modules selected in the desktop app.

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunSettings {
    pub workspace: PathBuf,
    pub output: PathBuf,
    /// Retained for settings compatibility; always resolved to this application.
    #[serde(default)]
    pub soulkit: PathBuf,
    pub cache_path: Option<PathBuf>,
    #[serde(default)]
    pub dll_directory: Option<PathBuf>,
    pub locales: String,
    pub keep_intermediates: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum TaskId {
    Languages,
    Images,
    Audio,
    Models,
    Maps,
    Items,
    Calendars,
}

impl TaskId {
    pub const ALL: [Self; 7] = [
        Self::Languages,
        Self::Images,
        Self::Audio,
        Self::Models,
        Self::Maps,
        Self::Items,
        Self::Calendars,
    ];

    fn pipeline(self) -> bool {
        matches!(
            self,
            Self::Languages | Self::Images | Self::Audio | Self::Maps | Self::Items
        )
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Dependency {
    pub task: TaskId,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Job {
    pub id: String,
    pub label: String,
    pub args: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RunPlan {
    pub requested: Vec<TaskId>,
    pub modules: Vec<TaskId>,
    pub dependencies: Vec<Dependency>,
    pub jobs: Vec<Job>,
}

fn add_dependency(
    modules: &mut BTreeSet<TaskId>,
    dependencies: &mut Vec<Dependency>,
    task: TaskId,
    reason: &str,
) {
    if modules.insert(task) {
        dependencies.push(Dependency {
            task,
            reason: reason.to_owned(),
        });
    }
}

fn path(root: &Path, relative: &str) -> String {
    root.join(relative).to_string_lossy().into_owned()
}

fn option(args: &mut Vec<String>, name: &str, value: impl Into<String>) {
    args.extend([name.to_owned(), value.into()]);
}

pub fn resolved(settings: &RunSettings) -> Result<RunSettings, String> {
    let workspace = std::fs::canonicalize(&settings.workspace)
        .map_err(|e| format!("Choose an existing workspace folder: {e}"))?;
    if !workspace.is_dir() {
        return Err("The workspace must be a folder.".into());
    }
    let soulkit = std::env::current_exe()
        .map_err(|e| format!("Cannot locate the application executable: {e}"))?;
    let output = if settings.output.is_absolute() {
        settings.output.clone()
    } else {
        workspace.join(&settings.output)
    };
    if output.is_file() {
        return Err("The output location is a file. Choose a folder instead.".into());
    }
    let cache_path = settings.cache_path.as_ref().map(|cache| {
        if cache.is_absolute() {
            cache.clone()
        } else {
            workspace.join(cache)
        }
    });
    // Cache checks belong to the selected plan's preflight. Language downloads
    // and calendars with existing inputs can run without a cache.
    Ok(RunSettings {
        workspace,
        soulkit,
        output,
        cache_path,
        dll_directory: settings.dll_directory.clone(),
        locales: settings.locales.clone(),
        keep_intermediates: settings.keep_intermediates,
    })
}

pub fn plan(settings: &RunSettings, requested: &[TaskId]) -> Result<RunPlan, String> {
    let settings = resolved(settings)?;
    let requested: BTreeSet<_> = requested.iter().copied().collect();
    if requested.is_empty() {
        return Err("Select at least one module to run.".into());
    }
    let mut modules = requested.clone();
    let mut dependencies = Vec::new();
    if modules.contains(&TaskId::Calendars) {
        if !settings.output.join("formatted_packages.json").is_file() {
            add_dependency(
                &mut modules,
                &mut dependencies,
                TaskId::Items,
                "Calendars need item package definitions; they have not been extracted here yet.",
            );
        }
        if !settings
            .output
            .join("maps/map-json/ancestors.json")
            .is_file()
        {
            add_dependency(
                &mut modules,
                &mut dependencies,
                TaskId::Maps,
                "The Gods and Ghosts calendar needs the extracted ancestor map data.",
            );
        }
    }
    if modules.contains(&TaskId::Items)
        && !settings.output.join("Languages/Languages.json").is_file()
    {
        add_dependency(
            &mut modules,
            &mut dependencies,
            TaskId::Languages,
            "Items need language strings; Languages.json has not been extracted here yet.",
        );
    }
    if modules.contains(&TaskId::Languages) {
        let locales: Vec<_> = settings.locales.split(',').map(str::trim).collect();
        if locales.is_empty()
            || locales.iter().any(|locale| {
                locale.is_empty()
                    || !locale.as_bytes()[0].is_ascii_alphabetic()
                    || !locale
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
            })
        {
            return Err("Enter language codes separated by commas, for example en,fr,de.".into());
        }
    }
    let mut jobs = Vec::new();
    if modules.iter().any(|m| m.pipeline()) {
        let mut args = vec!["run-all".into(), "--skip-calendars".into()];
        option(&mut args, "--locales", settings.locales.clone());
        for (name, destination) in [
            (
                "--droptables-output",
                settings.output.to_string_lossy().into_owned(),
            ),
            (
                "--items-output",
                settings.output.to_string_lossy().into_owned(),
            ),
            (
                "--calendars-output",
                settings.output.to_string_lossy().into_owned(),
            ),
            ("--artifacts-output", path(&settings.output, "Artifacts")),
            ("--images-output", path(&settings.output, "images")),
            ("--audio-output", path(&settings.output, "audio")),
            ("--map-output", path(&settings.output, "maps")),
            (
                "--items-packages",
                path(&settings.output, "packages_bin_attrs.json"),
            ),
            (
                "--items-languages",
                path(&settings.output, "Languages/Languages.json"),
            ),
            (
                "--items-recipes",
                path(&settings.output, "crafting_recipes.json"),
            ),
        ] {
            option(&mut args, name, destination);
        }
        for (module, flag) in [
            (TaskId::Languages, "--skip-languages"),
            (TaskId::Images, "--skip-images"),
            (TaskId::Audio, "--skip-audio"),
            (TaskId::Maps, "--skip-map"),
            (TaskId::Items, "--skip-items"),
        ] {
            if !modules.contains(&module) {
                args.push(flag.into());
            }
        }
        if !modules.contains(&TaskId::Items) {
            args.extend(["--skip-droptables".into(), "--skip-resolved-loot".into()]);
        }
        if settings.keep_intermediates {
            args.push("--keep-intermediates".into());
        }
        jobs.push(Job {
            id: "extract".into(),
            label: "Extract selected modules".into(),
            args,
        });
    }
    if modules.contains(&TaskId::Calendars) {
        let first = (chrono::Utc::now().date_naive() - chrono::Days::new(90))
            .format("%Y-%m-%d")
            .to_string();
        let mut zenith = vec!["zenith-calendar".into()];
        option(
            &mut zenith,
            "--packages",
            path(&settings.output, "formatted_packages.json"),
        );
        if modules.contains(&TaskId::Items) || settings.output.join("market_items.json").is_file() {
            option(
                &mut zenith,
                "--market-items",
                path(&settings.output, "market_items.json"),
            );
        }
        option(&mut zenith, "--from", first.clone());
        option(&mut zenith, "--days", "181");
        option(
            &mut zenith,
            "--output",
            path(&settings.output, "zenith-calendar.json"),
        );
        jobs.push(Job {
            id: "zenith".into(),
            label: "Generate Zenith calendar".into(),
            args: zenith,
        });
        let mut ancestors = vec![
            "gods-and-ghosts-calendar".into(),
            "--all-unlock-variations".into(),
        ];
        option(
            &mut ancestors,
            "--ancestors",
            path(&settings.output, "maps/map-json/ancestors.json"),
        );
        option(&mut ancestors, "--from", first.clone());
        option(&mut ancestors, "--days", "181");
        option(
            &mut ancestors,
            "--output",
            path(&settings.output, "gods-and-ghosts-calendar.json"),
        );
        jobs.push(Job {
            id: "gods_and_ghosts".into(),
            label: "Generate ancestor calendars".into(),
            args: ancestors,
        });
        let mut weather = vec!["weather-calendar".into()];
        option(
            &mut weather,
            "--packages",
            path(&settings.output, "formatted_packages.json"),
        );
        option(&mut weather, "--from", first);
        option(&mut weather, "--days", "181");
        option(
            &mut weather,
            "--output",
            path(&settings.output, "weather/weather-calendar.json"),
        );
        jobs.push(Job {
            id: "weather".into(),
            label: "Generate weather calendar".into(),
            args: weather,
        });
    }
    if modules.contains(&TaskId::Models) {
        jobs.push(Job {
            id: "models".into(),
            label: "Export static meshes".into(),
            args: vec![
                "models".into(),
                "extract".into(),
                "--output".into(),
                path(&settings.output, "models"),
            ],
        });
    }
    Ok(RunPlan {
        requested: TaskId::ALL
            .into_iter()
            .filter(|task| requested.contains(task))
            .collect(),
        modules: TaskId::ALL
            .into_iter()
            .filter(|task| modules.contains(task))
            .collect(),
        dependencies,
        jobs,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct Fixture {
        root: PathBuf,
    }
    impl Fixture {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let root = std::env::temp_dir().join(format!(
                "soulkit-plan-{}-{}-{}",
                std::process::id(),
                chrono::Utc::now().timestamp_nanos_opt().unwrap(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&root).unwrap();
            let root = fs::canonicalize(root).unwrap();
            Self { root }
        }
        fn settings(&self) -> RunSettings {
            RunSettings {
                workspace: self.root.clone(),
                output: "output".into(),
                soulkit: "soulkit-cli.exe".into(),
                cache_path: None,
                dll_directory: None,
                locales: "en,fr".into(),
                keep_intermediates: false,
            }
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn model_selection_is_a_standalone_export_outside_the_pipeline() {
        let f = Fixture::new();
        let plan = plan(&f.settings(), &[TaskId::Models]).unwrap();
        assert_eq!(plan.requested, vec![TaskId::Models]);
        assert_eq!(plan.modules, vec![TaskId::Models]);
        assert!(plan.dependencies.is_empty());
        assert_eq!(plan.jobs.len(), 1);
        let args = &plan.jobs[0].args;
        assert_eq!(args[..3], ["models", "extract", "--output"]);
        assert!(args[3].ends_with("models"));
        assert!(!f.root.join("output").exists());
    }

    #[test]
    fn audio_selection_does_not_download_languages_or_run_data_stages() {
        let f = Fixture::new();
        let plan = plan(&f.settings(), &[TaskId::Audio]).unwrap();
        assert_eq!(plan.modules, vec![TaskId::Audio]);
        assert!(plan.dependencies.is_empty());
        assert_eq!(plan.jobs.len(), 1);
        let args = &plan.jobs[0].args;
        for flag in [
            "--skip-languages",
            "--skip-items",
            "--skip-map",
            "--skip-images",
            "--skip-calendars",
            "--skip-droptables",
            "--skip-resolved-loot",
        ] {
            assert!(args.iter().any(|arg| arg == flag), "{flag}");
        }
        assert!(!args.iter().any(|arg| arg == "--skip-audio"));
        assert!(args.contains(&f.root.join("output/audio").to_string_lossy().into_owned()));
        assert!(
            !f.root.join("output").exists(),
            "planning must not write output"
        );
    }

    #[test]
    fn calendars_report_missing_supporting_inputs_and_reuse_existing_ones() {
        let f = Fixture::new();
        let settings = f.settings();
        let first = plan(&settings, &[TaskId::Calendars]).unwrap();
        assert_eq!(first.dependencies.len(), 3);
        assert!(first.modules.contains(&TaskId::Items));
        assert!(first.modules.contains(&TaskId::Maps));
        assert!(first.modules.contains(&TaskId::Languages));
        fs::create_dir_all(f.root.join("output/maps/map-json")).unwrap();
        fs::write(f.root.join("output/formatted_packages.json"), b"{}").unwrap();
        fs::write(f.root.join("output/maps/map-json/ancestors.json"), b"{}").unwrap();
        let second = plan(&settings, &[TaskId::Calendars]).unwrap();
        assert_eq!(second.modules, vec![TaskId::Calendars]);
        assert!(second.dependencies.is_empty());
        assert_eq!(second.jobs.len(), 3);
        assert!(second.jobs.iter().all(|j| j.args[0] != "run-all"));
    }

    #[test]
    fn public_task_ids_reject_removed_features_and_all_plan_keeps_allowed_tools() {
        let f = Fixture::new();
        for removed in ["scripts", "version"] {
            assert!(serde_json::from_value::<TaskId>(serde_json::json!(removed)).is_err());
        }
        let p = plan(&f.settings(), &TaskId::ALL).unwrap();
        assert_eq!(p.requested.len(), 7);
        assert_eq!(p.modules.len(), 7);
        assert_eq!(p.jobs.len(), 5);
        assert!(p
            .jobs
            .iter()
            .all(|job| !matches!(job.args[0].as_str(), "scripts" | "version")));
        assert!(p
            .jobs
            .iter()
            .all(|job| !job.args.iter().any(|arg| arg == "--skip-version")));
        assert!(!f.root.join("output").exists());
    }

    #[test]
    fn image_paths_with_spaces_remain_single_arguments() {
        let f = Fixture::new();
        let mut settings = f.settings();
        settings.output = PathBuf::from("folder with spaces");
        let p = plan(&settings, &[TaskId::Images]).unwrap();
        let image_path = f
            .root
            .join("folder with spaces/images")
            .to_string_lossy()
            .into_owned();
        assert!(p.jobs[0]
            .args
            .windows(2)
            .any(|args| args == ["--images-output", &image_path]));
        assert!(!f.root.join("folder with spaces").exists());
    }

    #[test]
    fn empty_selection_and_unsafe_language_codes_are_rejected_without_writes() {
        let f = Fixture::new();
        assert!(plan(&f.settings(), &[]).is_err());
        let mut settings = f.settings();
        settings.locales = "en,../../escape".into();
        assert!(plan(&settings, &[TaskId::Languages]).is_err());
        assert!(!f.root.join("output").exists());
    }

    #[test]
    fn planning_does_not_validate_a_cache_that_selected_tools_may_not_need() {
        let f = Fixture::new();
        let mut settings = f.settings();
        settings.cache_path = Some(f.root.join("missing-cache-location"));
        assert!(plan(&settings, &[TaskId::Languages]).is_ok());
        assert!(plan(&settings, &[TaskId::Audio]).is_ok());
        assert!(!f.root.join("output").exists());
    }
}
