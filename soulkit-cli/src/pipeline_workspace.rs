//! Scratch files owned by one pipeline invocation.
//!
//! Cleanup is explicit: failed runs and requested debug snapshots survive.

use anyhow::{ensure, Context, Result};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

const MARKER: &str = ".soulkit-owned";
static NEXT_RUN: AtomicU64 = AtomicU64::new(0);

pub struct PipelineWorkspace {
    base: PathBuf,
    parent: PathBuf,
    root: PathBuf,
    identity: String,
    created_parent: bool,
}

impl PipelineWorkspace {
    pub fn new(base: &Path) -> Result<Self> {
        let base = fs::canonicalize(base)
            .with_context(|| format!("Cannot resolve pipeline directory {}", base.display()))?;
        ensure!(base.is_dir(), "Pipeline directory is not a directory");
        reject_link_ancestors(&base)?;

        let parent = base.join(".soulkit-work");
        let created_parent = match fs::create_dir(&parent) {
            Ok(()) => true,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => false,
            Err(error) => return Err(error).context("Cannot create pipeline scratch directory"),
        };
        reject_link_ancestors(&parent)?;
        ensure!(
            parent.is_dir(),
            "Pipeline scratch parent is not a directory"
        );
        ensure!(
            fs::canonicalize(&parent)? == parent,
            "Pipeline scratch parent resolves outside its expected location"
        );

        let mut root = None;
        let mut identity = String::new();
        for _ in 0..100 {
            identity = unique_identity();
            let candidate = parent.join(format!("run-{identity}"));
            match fs::create_dir(&candidate) {
                Ok(()) => {
                    root = Some(candidate);
                    break;
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error).context("Cannot create pipeline run directory"),
            }
        }
        let root = root.context("Cannot allocate a unique pipeline run directory")?;
        reject_link_ancestors(&root)?;
        ensure!(
            fs::canonicalize(&root)? == root,
            "Pipeline run directory resolves outside its expected location"
        );
        let mut marker = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(root.join(MARKER))
            .context("Cannot mark ownership of pipeline scratch files")?;
        marker.write_all(identity.as_bytes())?;

        Ok(Self {
            base,
            parent,
            root,
            identity,
            created_parent,
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Validate outputs and caller-owned inputs before running any stages.
    pub fn ensure_retained_paths(&self, paths: &[&Path]) -> Result<()> {
        for path in paths {
            let resolved = resolve_existing_ancestors(&self.base, path)
                .with_context(|| format!("Cannot resolve retained path {}", path.display()))?;
            ensure!(
                !resolved.starts_with(&self.root) && !self.root.starts_with(&resolved),
                "Retained path {} overlaps pipeline scratch directory {}",
                path.display(),
                self.root.display()
            );
        }
        Ok(())
    }

    pub fn finish(&self, success: bool, keep: bool) -> Result<()> {
        if !success || keep {
            println!("Pipeline intermediates kept at {}", self.root.display());
            return Ok(());
        }
        // A repeated finish call is harmless once this owned directory is gone.
        match fs::symlink_metadata(&self.root) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error).context("Cannot inspect pipeline scratch directory"),
            Ok(_) => {}
        }

        let result = (|| -> Result<()> {
            self.check_ownership()?;
            self.check_entries()?;
            // Re-check the root and marker after walking the tree, before the
            // only recursive removal in this module.
            self.check_ownership()?;
            fs::remove_dir_all(&self.root)?;
            if self.created_parent {
                // Nonrecursive: another invocation or retained snapshot makes
                // this fail harmlessly rather than removing their files.
                let _ = fs::remove_dir(&self.parent);
            }
            Ok(())
        })();
        result.with_context(|| {
            format!(
                "Cannot safely clean pipeline intermediates at {}; files were retained",
                self.root.display()
            )
        })?;
        println!(
            "Pipeline intermediates removed from {}",
            self.root.display()
        );
        Ok(())
    }

    fn check_ownership(&self) -> Result<()> {
        ensure!(
            self.root.parent() == Some(self.parent.as_path())
                && self.parent.parent() == Some(self.base.as_path())
                && self.root != self.parent
                && self.root != self.base,
            "Unexpected pipeline scratch directory ancestry"
        );
        reject_link_ancestors(&self.root)?;
        ensure!(
            fs::canonicalize(&self.base)? == self.base
                && fs::canonicalize(&self.parent)? == self.parent
                && fs::canonicalize(&self.root)? == self.root,
            "Pipeline scratch directory was replaced or redirected"
        );
        ensure!(
            self.root.is_dir(),
            "Pipeline scratch directory is not a directory"
        );
        let marker = self.root.join(MARKER);
        let metadata =
            fs::symlink_metadata(&marker).context("Pipeline ownership marker is missing")?;
        ensure!(
            !is_link_or_reparse(&metadata)
                && metadata.is_file()
                && metadata.len() == self.identity.len() as u64,
            "Pipeline ownership marker was replaced"
        );
        ensure!(
            fs::read_to_string(marker)? == self.identity,
            "Pipeline ownership marker no longer matches this invocation"
        );
        Ok(())
    }

    fn check_entries(&self) -> Result<()> {
        let mut directories = vec![self.root.clone()];
        while let Some(directory) = directories.pop() {
            for entry in fs::read_dir(directory)? {
                let path = entry?.path();
                let metadata = fs::symlink_metadata(&path)?;
                ensure!(
                    !is_link_or_reparse(&metadata),
                    "Refusing cleanup through a link or reparse point: {}",
                    path.display()
                );
                let resolved = fs::canonicalize(&path)?;
                ensure!(
                    resolved.starts_with(&self.root) && resolved != self.root,
                    "Pipeline scratch entry resolves outside its owned directory"
                );
                if metadata.is_dir() {
                    directories.push(path);
                }
            }
        }
        Ok(())
    }
}

fn unique_identity() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!(
        "{}-{nanos}-{}",
        std::process::id(),
        NEXT_RUN.fetch_add(1, Ordering::Relaxed)
    )
}

fn reject_link_ancestors(path: &Path) -> Result<()> {
    for ancestor in path.ancestors() {
        let metadata = fs::symlink_metadata(ancestor)?;
        ensure!(
            !is_link_or_reparse(&metadata),
            "Refusing pipeline scratch path through a link or reparse point: {}",
            ancestor.display()
        );
    }
    Ok(())
}

fn is_link_or_reparse(metadata: &fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }
    #[cfg(not(windows))]
    {
        false
    }
}

/// Resolve every existing prefix so a junction plus a nonexistent output leaf
/// cannot hide an overlap. Apply parent components after resolving links.
fn resolve_existing_ancestors(base: &Path, path: &Path) -> Result<PathBuf> {
    if !path.is_absolute() {
        ensure!(
            !path.has_root() && !path.components().any(|c| matches!(c, Component::Prefix(_))),
            "Retained paths must be relative or fully qualified"
        );
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
            Component::RootDir => {
                resolved.push(component.as_os_str());
                resolved = fs::canonicalize(&resolved)?;
            }
            Component::CurDir => {}
            Component::ParentDir => {
                resolved.pop();
            }
            Component::Normal(name) => {
                resolved.push(name);
                match fs::symlink_metadata(&resolved) {
                    Ok(_) => resolved = fs::canonicalize(&resolved)?,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error.into()),
                }
            }
        }
    }
    ensure!(resolved.is_absolute(), "Cannot resolve retained path");
    Ok(resolved)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestDirectory {
        path: PathBuf,
        parent: PathBuf,
    }

    impl TestDirectory {
        fn new() -> Self {
            let parent = fs::canonicalize(std::env::temp_dir()).unwrap();
            let path = parent.join(format!("soulkit-workspace-test-{}", unique_identity()));
            fs::create_dir(&path).unwrap();
            Self { path, parent }
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            if self.path.parent() == Some(self.parent.as_path())
                && fs::canonicalize(&self.path).is_ok_and(|path| path == self.path)
            {
                let _ = fs::remove_dir_all(&self.path);
            }
        }
    }

    #[test]
    fn success_removes_only_this_runs_scratch() {
        let base = TestDirectory::new();
        let output = base.path.join("output");
        fs::create_dir(&output).unwrap();
        fs::write(output.join("keep.json"), b"final").unwrap();
        let workspace = PipelineWorkspace::new(&base.path).unwrap();
        let root = workspace.root().to_path_buf();
        fs::create_dir(root.join("downloads")).unwrap();
        fs::write(root.join("downloads/input.bin"), b"scratch").unwrap();
        workspace.ensure_retained_paths(&[&output]).unwrap();
        workspace.finish(true, false).unwrap();
        workspace.finish(true, false).unwrap();
        assert!(!root.exists());
        assert_eq!(fs::read(output.join("keep.json")).unwrap(), b"final");
        assert!(base.path.is_dir());
    }

    #[test]
    fn failure_and_keep_option_preserve_intermediates() {
        let base = TestDirectory::new();
        for (success, keep) in [(false, false), (true, true)] {
            let workspace = PipelineWorkspace::new(&base.path).unwrap();
            fs::write(workspace.root().join("input.bin"), b"debug snapshot").unwrap();
            workspace.finish(success, keep).unwrap();
            assert_eq!(
                fs::read(workspace.root().join("input.bin")).unwrap(),
                b"debug snapshot"
            );
        }
    }

    #[test]
    fn retained_outputs_and_custom_inputs_cannot_overlap_scratch() {
        let base = TestDirectory::new();
        let workspace = PipelineWorkspace::new(&base.path).unwrap();
        for retained in [
            base.path.clone(),
            workspace.parent.clone(),
            workspace.root.clone(),
            workspace.root.join("future/final.json"),
        ] {
            assert!(workspace.ensure_retained_paths(&[&retained]).is_err());
        }
        workspace
            .ensure_retained_paths(&[
                Path::new("output/final.json"),
                Path::new("custom-keys"),
                Path::new("nonexistent/../output/other.json"),
            ])
            .unwrap();
    }

    #[test]
    fn changed_marker_preserves_the_directory() {
        let base = TestDirectory::new();
        let workspace = PipelineWorkspace::new(&base.path).unwrap();
        fs::write(workspace.root().join(MARKER), b"belongs to someone else").unwrap();
        fs::write(workspace.root().join("keep.bin"), b"data").unwrap();
        assert!(workspace.finish(true, false).is_err());
        assert!(workspace.root().join("keep.bin").exists());
    }

    #[test]
    fn replaced_root_is_not_removed() {
        let base = TestDirectory::new();
        let workspace = PipelineWorkspace::new(&base.path).unwrap();
        let original = workspace.root().with_extension("original");
        fs::rename(workspace.root(), &original).unwrap();
        fs::create_dir(workspace.root()).unwrap();
        fs::write(workspace.root().join("unowned.bin"), b"keep").unwrap();
        assert!(workspace.finish(true, false).is_err());
        assert!(workspace.root().join("unowned.bin").exists());
        assert!(original.join(MARKER).exists());
    }

    #[cfg(any(unix, windows))]
    fn directory_link(link: &Path, target: &Path) -> bool {
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(target, link).unwrap();
            true
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            // Junctions do not need Windows developer mode or symlink privileges.
            // Test paths are generated beneath temp; avoid shell metacharacters.
            if [link, target]
                .iter()
                .any(|p| p.to_string_lossy().chars().any(|c| "&|<>^%!\"".contains(c)))
            {
                return false;
            }
            std::process::Command::new("cmd.exe")
                .args(["/D", "/C", "mklink", "/J"])
                .arg(link)
                .arg(target)
                .creation_flags(0x0800_0000)
                .output()
                .is_ok_and(|output| output.status.success())
        }
    }

    #[test]
    #[cfg(any(unix, windows))]
    fn links_and_windows_junctions_cannot_redirect_cleanup_or_hide_outputs() {
        let base = TestDirectory::new();
        let outside = base.path.join("retained");
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("keep.json"), b"final").unwrap();
        let workspace = PipelineWorkspace::new(&base.path).unwrap();
        if !directory_link(&workspace.root().join("escape"), &outside) {
            return;
        }
        assert!(workspace.finish(true, false).is_err());
        assert_eq!(fs::read(outside.join("keep.json")).unwrap(), b"final");
        assert!(workspace.root().join(MARKER).exists());

        let alias = base.path.join("looks-external");
        assert!(directory_link(&alias, workspace.root()));
        assert!(workspace
            .ensure_retained_paths(&[&alias.join("not-created/final.json")])
            .is_err());
    }

    #[test]
    #[cfg(any(unix, windows))]
    fn root_replaced_by_link_or_junction_is_preserved() {
        let base = TestDirectory::new();
        let outside = base.path.join("retained");
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("keep.json"), b"final").unwrap();
        let workspace = PipelineWorkspace::new(&base.path).unwrap();
        let original = workspace.root().with_extension("original");
        fs::rename(workspace.root(), &original).unwrap();
        if !directory_link(workspace.root(), &outside) {
            return;
        }
        assert!(workspace.finish(true, false).is_err());
        assert_eq!(fs::read(outside.join("keep.json")).unwrap(), b"final");
        assert!(original.join(MARKER).exists());
    }
}
