//! Decode one requested media asset in a disposable, bounded private cache.

use crate::cache_browser::{self, CacheEntry};
use crate::tasks::RunSettings;
use serde::Serialize;
use std::collections::VecDeque;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tauri::Manager;

const MAX_INPUT: u64 = 32 * 1024 * 1024;
const MAX_CACHE: u64 = 64 * 1024 * 1024;
const MAX_ENTRIES: usize = 8;
const OWNER: &str = "soulkit-media-preview-v1";

#[derive(Clone, Serialize)]
pub struct CachePreview {
    pub path: Option<String>,
    pub kind: String,
    pub notice: Option<String>,
}

struct Cached {
    key: String,
    directory: PathBuf,
    bytes: u64,
    preview: CachePreview,
}

#[derive(Default)]
struct Inner {
    session: Option<PathBuf>,
    // Windows keeps this marker exclusively open so other app instances can
    // distinguish a live session from one abandoned by a crashed process.
    owner_file: Option<fs::File>,
    child: Option<(u64, Child)>,
    entries: VecDeque<Cached>,
}

#[derive(Default)]
pub struct PreviewService {
    inner: Mutex<Inner>,
    sequence: AtomicU64,
    closing: AtomicBool,
}

impl PreviewService {
    pub fn cancel(&self) -> Result<(), String> {
        let token = self.sequence.fetch_add(1, Ordering::AcqRel) + 1;
        self.stop_child_through(token)
    }

    pub fn shutdown(&self) {
        self.closing.store(true, Ordering::Release);
        let _ = self.cancel();
        let session = self.inner.lock().ok().and_then(|mut state| {
            state.entries.clear();
            drop(state.owner_file.take());
            state.session.take()
        });
        if let Some(session) = session {
            remove_owned(&session);
        }
    }

    fn stop_child_through(&self, token: u64) -> Result<(), String> {
        let mut state = self
            .inner
            .lock()
            .map_err(|_| "Preview state is unavailable.")?;
        if state
            .child
            .as_ref()
            .is_some_and(|(active, _)| *active > token)
        {
            return Ok(());
        }
        if let Some((_, mut child)) = state.child.take() {
            let _ = child.kill();
            child
                .wait()
                .map_err(|error| format!("Cannot stop the media preview: {error}"))?;
        }
        Ok(())
    }

    pub fn preview(
        &self,
        app: &tauri::AppHandle,
        settings: &RunSettings,
        entry: &CacheEntry,
        cache: &Path,
        generation: u64,
    ) -> Result<CachePreview, String> {
        let token = self.sequence.fetch_add(1, Ordering::AcqRel) + 1;
        self.stop_child_through(token)?;
        if self.closing.load(Ordering::Acquire) {
            return Err("Soulkit is closing.".into());
        }
        let kind = match entry.kind {
            cache_browser::MediaKind::Images => "image",
            cache_browser::MediaKind::Audio => "audio",
            cache_browser::MediaKind::Models => {
                return Ok(CachePreview {
                    path: None,
                    kind: "model".into(),
                    notice: Some("Models have no in-app preview. Extract to get a .glb you can open in Blender or any glTF viewer.".into()),
                })
            }
            cache_browser::MediaKind::All => {
                return Err("Choose an image or audio asset to preview.".into())
            }
        };
        let extension = Path::new(&entry.source_path)
            .extension()
            .unwrap_or_default()
            .to_string_lossy()
            .to_ascii_lowercase();
        if kind == "image" && matches!(extension.as_str(), "hdr" | "exr") {
            return Ok(CachePreview {
                path: None,
                kind: kind.into(),
                notice: Some("This native lighting texture cannot be previewed or converted yet. Extraction preserves its original cache parts in images/native.".into()),
            });
        }
        if kind == "audio" && matches!(extension.as_str(), "mid" | "midi") {
            return Ok(CachePreview {
                path: None,
                kind: kind.into(),
                notice: Some("This player-instrument score uses the game's native note format and cannot be played here. Extraction preserves its original cache parts in audio/native.".into()),
            });
        }
        if entry.compressed_bytes > MAX_INPUT || entry.decompressed_bytes > MAX_INPUT {
            return Ok(CachePreview {
                path: None,
                kind: kind.into(),
                notice: Some("This asset is too large for an in-app cache preview. Extract it to view or play the full file.".into()),
            });
        }
        let key = format!("{generation}:{}", entry.id);
        let directory = {
            let mut state = self
                .inner
                .lock()
                .map_err(|_| "Preview state is unavailable.")?;
            if self.closing.load(Ordering::Acquire)
                || self.sequence.load(Ordering::Acquire) != token
            {
                return Err("Preview cancelled.".into());
            }
            if let Some(position) = state.entries.iter().position(|item| item.key == key) {
                let item = state.entries.remove(position).unwrap();
                if item
                    .preview
                    .path
                    .as_ref()
                    .is_some_and(|path| Path::new(path).is_file())
                {
                    let result = item.preview.clone();
                    state.entries.push_back(item);
                    return Ok(result);
                }
                remove_owned(&item.directory);
            }
            if state.session.is_none() {
                let base = app
                    .path()
                    .app_cache_dir()
                    .map_err(|error| error.to_string())?
                    .join("media-previews");
                let (session, owner) = create_session(&base)?;
                state.owner_file = Some(owner);
                state.session = Some(session);
            }
            let directory = state
                .session
                .as_ref()
                .unwrap()
                .join(format!("asset-{token}"));
            fs::create_dir(&directory)
                .map_err(|error| format!("Cannot create a media preview: {error}"))?;
            fs::write(directory.join(".owner"), OWNER).map_err(|error| error.to_string())?;
            directory
        };
        let result = self.decode(app, settings, entry, cache, kind, token, &directory);
        match result {
            Ok((preview, bytes)) if preview.path.is_some() => {
                let mut state = self
                    .inner
                    .lock()
                    .map_err(|_| "Preview state is unavailable.")?;
                if self.sequence.load(Ordering::Acquire) != token {
                    remove_owned(&directory);
                    return Err("Preview cancelled.".into());
                }
                while state.entries.len() >= MAX_ENTRIES
                    || state
                        .entries
                        .iter()
                        .map(|entry| entry.bytes)
                        .sum::<u64>()
                        .saturating_add(bytes)
                        > MAX_CACHE
                {
                    if let Some(old) = state.entries.pop_front() {
                        remove_owned(&old.directory);
                    } else {
                        break;
                    }
                }
                state.entries.push_back(Cached {
                    key,
                    directory,
                    bytes,
                    preview: preview.clone(),
                });
                Ok(preview)
            }
            Ok((preview, _)) => {
                remove_owned(&directory);
                Ok(preview)
            }
            Err(error) => {
                let _ = self.stop_child_through(token);
                remove_owned(&directory);
                Err(error)
            }
        }
    }

    fn decode(
        &self,
        app: &tauri::AppHandle,
        settings: &RunSettings,
        entry: &CacheEntry,
        cache: &Path,
        kind: &str,
        token: u64,
        directory: &Path,
    ) -> Result<(CachePreview, u64), String> {
        let output = directory.join("output");
        let manifest = cache_browser::manifest_for_entries(
            cache,
            &output,
            std::slice::from_ref(entry),
            "png",
        )?;
        let manifest_path = directory.join("selection.json");
        cache_browser::write_manifest_at(&manifest_path, &manifest)?;
        let mut command = Command::new(std::env::current_exe().map_err(|error| error.to_string())?);
        command
            .arg(crate::worker::WORKER_FLAG)
            .arg(cache_browser::WORKER_COMMAND)
            .arg(&manifest_path)
            .current_dir(&settings.workspace)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        crate::runtime_libraries::configure_worker(
            &mut command,
            &settings.workspace,
            settings.dll_directory.as_deref(),
        )?;
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000);
        }
        let (stdout, stderr) = {
            let mut state = self
                .inner
                .lock()
                .map_err(|_| "Preview state is unavailable.")?;
            if self.sequence.load(Ordering::Acquire) != token {
                return Err("Preview cancelled.".into());
            }
            let mut child = command
                .spawn()
                .map_err(|error| format!("Cannot start the media preview: {error}"))?;
            let pipes = child.stdout.take().zip(child.stderr.take());
            let Some(pipes) = pipes else {
                let _ = child.kill();
                let _ = child.wait();
                return Err("Cannot capture preview output.".into());
            };
            state.child = Some((token, child));
            pipes
        };
        let stdout = drain(stdout);
        let stderr = drain(stderr);
        let started = Instant::now();
        let status = loop {
            if self.sequence.load(Ordering::Acquire) != token {
                return Err("Preview cancelled.".into());
            }
            let mut state = self
                .inner
                .lock()
                .map_err(|_| "Preview state is unavailable.")?;
            let Some((active, child)) = state.child.as_mut() else {
                return Err("Preview cancelled.".into());
            };
            if *active != token {
                return Err("Preview cancelled.".into());
            }
            if let Some(status) = child.try_wait().map_err(|error| error.to_string())? {
                if let Some((_, mut child)) = state.child.take() {
                    let _ = child.wait();
                }
                break status;
            }
            if started.elapsed() > Duration::from_secs(45) {
                if let Some((_, mut child)) = state.child.take() {
                    let _ = child.kill();
                    let _ = child.wait();
                }
                return Err("This preview took too long. Extract the asset to view or play it outside Soulkit.".into());
            }
            drop(state);
            std::thread::sleep(Duration::from_millis(25));
        };
        let out = stdout.join().unwrap_or_default();
        let err = stderr.join().unwrap_or_default();
        if !status.success() {
            let text = if err.trim().is_empty() { out } else { err };
            return Err(format!("Preview failed: {}", text.trim()));
        }
        let mut files = Vec::new();
        collect(&output, &mut files)?;
        let bytes = files.iter().map(|(_, size)| *size).sum::<u64>();
        if bytes > MAX_CACHE {
            return Ok((CachePreview { path: None, kind: kind.into(), notice: Some("The decoded file is too large for a cache preview. Extract it to view the full file.".into()) }, 0));
        }
        let file = files.into_iter().find(|(path, _)| {
            let extension = path
                .extension()
                .unwrap_or_default()
                .to_string_lossy()
                .to_ascii_lowercase();
            if kind == "image" {
                matches!(extension.as_str(), "png" | "jpg" | "jpeg" | "webp")
            } else {
                matches!(extension.as_str(), "ogg" | "wav" | "mp3" | "flac")
            }
        });
        let Some((path, _)) = file else {
            return Ok((CachePreview { path: None, kind: kind.into(), notice: Some("This format does not have a playable in-app preview. Extract it to keep the original media data.".into()) }, 0));
        };
        app.asset_protocol_scope()
            .allow_directory(directory, true)
            .map_err(|error| error.to_string())?;
        Ok((
            CachePreview {
                path: Some(readable(&path)),
                kind: kind.into(),
                notice: None,
            },
            bytes,
        ))
    }
}

fn create_session(base: &Path) -> Result<(PathBuf, fs::File), String> {
    fs::create_dir_all(base)
        .map_err(|error| format!("Cannot create the preview cache: {error}"))?;
    reap_stale_sessions(base);
    let session = base.join(format!(
        "session-{}-{}",
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
    ));
    fs::create_dir(&session)
        .map_err(|error| format!("Cannot create the preview session: {error}"))?;
    match create_owner_file(&session) {
        Ok(owner) => Ok((session, owner)),
        Err(error) => {
            // Only remove the just-created directory if it is still empty.
            let _ = fs::remove_dir(&session);
            Err(format!("Cannot mark the preview session: {error}"))
        }
    }
}

fn create_owner_file(directory: &Path) -> std::io::Result<fs::File> {
    let path = directory.join(".owner");
    let mut options = fs::OpenOptions::new();
    options.read(true).write(true).create_new(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(0);
    }
    let mut file = options.open(&path)?;
    if let Err(error) = file.write_all(OWNER.as_bytes()) {
        drop(file);
        let _ = fs::remove_file(&path);
        return Err(error);
    }
    Ok(file)
}

#[cfg(windows)]
fn session_name(name: &std::ffi::OsStr) -> bool {
    let Some(name) = name.to_str().and_then(|name| name.strip_prefix("session-")) else {
        return false;
    };
    let Some((pid, timestamp)) = name.split_once('-') else {
        return false;
    };
    pid.bytes().all(|byte| byte.is_ascii_digit())
        && timestamp.bytes().all(|byte| byte.is_ascii_digit())
        && pid.parse::<u32>().is_ok_and(|pid| pid > 0)
        && timestamp
            .parse::<u64>()
            .is_ok_and(|timestamp| timestamp > 0)
}

#[cfg(windows)]
fn reap_stale_sessions(base: &Path) {
    use std::os::windows::fs::OpenOptionsExt;

    let Ok(entries) = fs::read_dir(base) else {
        return;
    };
    for entry in entries.flatten() {
        if !session_name(&entry.file_name()) {
            continue;
        }
        let path = entry.path();
        if !fs::symlink_metadata(&path)
            .is_ok_and(|metadata| metadata.is_dir() && !is_link(&metadata))
        {
            continue;
        }
        let marker = path.join(".owner");
        if !fs::symlink_metadata(&marker)
            .is_ok_and(|metadata| metadata.is_file() && !is_link(&metadata))
        {
            continue;
        }
        // A live app holds its marker with no sharing. Only abandoned markers
        // can be opened exclusively, including when a previous process crashed.
        let Ok(mut owner) = fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&marker)
        else {
            continue;
        };
        let mut value = String::new();
        let owned = (&mut owner)
            .take((OWNER.len() + 1) as u64)
            .read_to_string(&mut value)
            .is_ok()
            && value == OWNER;
        drop(owner);
        if owned {
            remove_owned(&path);
        }
    }
}

// Without a portable exclusive file lock, leave other sessions untouched.
#[cfg(not(windows))]
fn reap_stale_sessions(_base: &Path) {}

fn drain(mut pipe: impl Read + Send + 'static) -> std::thread::JoinHandle<String> {
    std::thread::spawn(move || {
        let mut captured = Vec::new();
        let _ = std::io::copy(&mut pipe.by_ref().take(16_384), &mut captured);
        let _ = std::io::copy(&mut pipe, &mut std::io::sink());
        String::from_utf8_lossy(&captured).into_owned()
    })
}

fn collect(directory: &Path, files: &mut Vec<(PathBuf, u64)>) -> Result<(), String> {
    for entry in fs::read_dir(directory).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let metadata = fs::symlink_metadata(entry.path()).map_err(|error| error.to_string())?;
        if is_link(&metadata) {
            return Err("A preview output unexpectedly contains a filesystem link.".into());
        }
        if metadata.is_dir() {
            collect(&entry.path(), files)?;
        } else if metadata.is_file() {
            files.push((entry.path(), metadata.len()));
        }
        if files.len() > 128 {
            return Err("The preview produced too many files.".into());
        }
    }
    Ok(())
}

fn readable(path: &Path) -> String {
    let path = path.to_string_lossy();
    if let Some(unc) = path.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{unc}")
    } else {
        path.strip_prefix(r"\\?\").unwrap_or(&path).to_string()
    }
}

fn remove_owned(path: &Path) {
    if fs::symlink_metadata(path).is_ok_and(|metadata| metadata.is_dir() && !is_link(&metadata))
        && fs::read_to_string(path.join(".owner")).ok().as_deref() == Some(OWNER)
        && no_links(path)
    {
        let _ = fs::remove_dir_all(path);
    }
}

fn is_link(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return true;
        }
    }
    metadata.file_type().is_symlink()
}

fn no_links(path: &Path) -> bool {
    let Ok(entries) = fs::read_dir(path) else {
        return false;
    };
    for entry in entries {
        let Ok(entry) = entry else {
            return false;
        };
        let Ok(metadata) = fs::symlink_metadata(entry.path()) else {
            return false;
        };
        if is_link(&metadata) || (metadata.is_dir() && !no_links(&entry.path())) {
            return false;
        }
    }
    true
}

impl Drop for PreviewService {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    static FIXTURE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    fn fixture() -> PathBuf {
        let directory = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../target/cache-preview-tests")
            .join(format!(
                "{}-{}-{}",
                std::process::id(),
                chrono::Utc::now().timestamp_nanos_opt().unwrap(),
                FIXTURE_SEQUENCE.fetch_add(1, Ordering::Relaxed)
            ));
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join(".owner"), OWNER).unwrap();
        directory
    }

    #[test]
    fn cleanup_requires_owned_directory_and_preserves_neighbor() {
        let root = fixture();
        let owned = root.join("owned");
        let neighbor = root.join("neighbor");
        fs::create_dir(&owned).unwrap();
        fs::write(owned.join(".owner"), OWNER).unwrap();
        fs::write(owned.join("preview.png"), b"image").unwrap();
        fs::create_dir(&neighbor).unwrap();
        fs::write(neighbor.join("keep.txt"), b"keep").unwrap();
        remove_owned(&neighbor);
        assert_eq!(fs::read(neighbor.join("keep.txt")).unwrap(), b"keep");
        remove_owned(&owned);
        assert!(!owned.exists());
        assert!(neighbor.exists());
        remove_owned(&root);
    }

    #[test]
    fn preview_collection_counts_nested_outputs_and_rejects_excess_files() {
        let root = fixture();
        let output = root.join("output");
        fs::create_dir_all(output.join("nested")).unwrap();
        fs::write(output.join("nested/asset.png"), b"12345").unwrap();
        let mut files = Vec::new();
        collect(&output, &mut files).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].1, 5);
        for index in 0..128 {
            fs::write(output.join(format!("{index}.txt")), b"x").unwrap();
        }
        assert!(collect(&output, &mut Vec::new())
            .unwrap_err()
            .contains("too many files"));
        remove_owned(&root);
    }

    #[test]
    fn readable_path_handles_windows_extended_paths() {
        assert_eq!(
            readable(Path::new(r"\\?\C:\exports\voice.ogg")),
            r"C:\exports\voice.ogg"
        );
        assert_eq!(
            readable(Path::new(r"\\?\UNC\server\exports\image.png")),
            r"\\server\exports\image.png"
        );
    }

    #[test]
    fn shutdown_removes_session_without_relying_on_process_drop() {
        let root = fixture();
        let (session, owner) = create_session(&root).unwrap();
        let service = PreviewService::default();
        {
            let mut state = service.inner.lock().unwrap();
            state.session = Some(session.clone());
            state.owner_file = Some(owner);
        }
        service.shutdown();
        assert!(service.closing.load(Ordering::Acquire));
        assert!(!session.exists());
        assert!(root.exists());
        assert!(service.inner.lock().unwrap().session.is_none());
        assert!(service.inner.lock().unwrap().owner_file.is_none());
        remove_owned(&root);
    }

    #[cfg(windows)]
    #[test]
    fn stale_session_cleanup_preserves_live_locks_and_unowned_neighbors() {
        let root = fixture();
        let (live, owner) = create_session(&root).unwrap();
        let stale = root.join("session-13-130");
        let unowned = root.join("session-14-140");
        let foreign = root.join("session-15-150");
        let neighbor = root.join("keep-this-folder");
        let malformed = root.join("session-16-160-extra");
        for path in [&stale, &unowned, &foreign, &neighbor, &malformed] {
            fs::create_dir(path).unwrap();
            fs::write(path.join("keep.bin"), b"data").unwrap();
        }
        fs::write(stale.join(".owner"), OWNER).unwrap();
        fs::write(foreign.join(".owner"), "another app").unwrap();
        fs::write(neighbor.join(".owner"), OWNER).unwrap();
        fs::write(malformed.join(".owner"), OWNER).unwrap();
        assert!(
            fs::File::open(live.join(".owner")).is_err(),
            "Live session marker must be exclusive"
        );

        reap_stale_sessions(&root);
        assert!(
            live.exists(),
            "A live preview session must survive another app's cleanup"
        );
        assert!(
            !stale.exists(),
            "An abandoned owned session should be reclaimed"
        );
        for path in [&unowned, &foreign, &neighbor, &malformed] {
            assert_eq!(fs::read(path.join("keep.bin")).unwrap(), b"data");
        }

        drop(owner);
        reap_stale_sessions(&root);
        assert!(
            !live.exists(),
            "Releasing the owner handle makes the session reclaimable"
        );
        remove_owned(&root);
    }
}
