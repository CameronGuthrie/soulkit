//! Bounded, read-only browsing of extracted files beneath the selected output.

use serde::Serialize;
use std::cmp::Ordering;
use std::fs::{self, File, Metadata};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::UNIX_EPOCH;

pub const MAX_PREVIEW_BYTES: usize = 256 * 1024;
const MAX_SCAN_ENTRIES: usize = 200_000;
const MAX_SEARCH_RESULTS: usize = 2_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FileKind {
    Folder,
    Image,
    Audio,
    Json,
    Text,
    Binary,
}

#[derive(Debug, Clone, Serialize)]
pub struct LibraryEntry {
    pub name: String,
    pub relative_path: String,
    pub absolute_path: String,
    pub is_dir: bool,
    pub kind: FileKind,
    pub size: u64,
    /// Modification time as Unix milliseconds, for the frontend's local date formatting.
    pub modified: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FilePreview {
    pub kind: FileKind,
    pub content: String,
    pub truncated: bool,
    pub size: u64,
    pub mime_type: String,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct LibraryStats {
    pub files: u64,
    pub folders: u64,
    pub bytes: u64,
    pub images: u64,
    pub audio: u64,
    pub json: u64,
    pub other: u64,
    pub truncated: bool,
}

/// List one directory without recursively traversing a large audio collection.
/// A new installation's missing output directory is a valid empty library.
pub fn list_directory(root: &Path, relative: &str) -> Result<Vec<LibraryEntry>, String> {
    let components = relative_components(relative)?;
    let Some(root) = canonical_root(root)? else {
        return Ok(Vec::new());
    };
    let directory = contained_path(&root, &components)?;
    if !directory.is_dir() {
        return Err("The selected library path is not a folder.".into());
    }
    let mut entries = Vec::new();
    for item in fs::read_dir(&directory).map_err(|e| format!("Cannot read folder: {e}"))? {
        let item = item.map_err(|e| format!("Cannot read folder entry: {e}"))?;
        if is_repository_metadata(&item.path()) {
            continue;
        }
        // A running extraction may replace a file during the listing. Skip
        // disappearing/inaccessible entries and all links rather than following them.
        if let Ok(Some(entry)) = describe_entry(&root, &item.path()) {
            entries.push(entry);
        }
    }
    entries.sort_by(entry_order);
    Ok(entries)
}

/// Search names and relative paths with a bounded, iterative directory walk.
/// Wrappers should run this work on a blocking worker rather than the GUI thread.
pub fn search(root: &Path, query: &str, limit: usize) -> Result<Vec<LibraryEntry>, String> {
    let query = query.trim().to_lowercase();
    if query.is_empty() || limit == 0 {
        return Ok(Vec::new());
    }
    let Some(root) = canonical_root(root)? else {
        return Ok(Vec::new());
    };
    let limit = limit.min(MAX_SEARCH_RESULTS);
    let mut found = Vec::new();
    let mut directories = vec![root.clone()];
    let mut visited = 0usize;
    while let Some(directory) = directories.pop() {
        let items = match fs::read_dir(&directory) {
            Ok(items) => items,
            Err(_) => continue,
        };
        let mut children = Vec::new();
        for item in items.flatten() {
            let path = item.path();
            if is_repository_metadata(&path) {
                continue;
            }
            visited += 1;
            if visited > MAX_SCAN_ENTRIES {
                found.sort_by(entry_order);
                return Ok(found);
            }
            let file_type = match item.file_type() {
                Ok(file_type) => file_type,
                Err(_) => continue,
            };
            if file_type.is_symlink() || (!file_type.is_file() && !file_type.is_dir()) {
                continue;
            }
            if !file_type.is_dir() {
                // Most entries in a large audio collection do not match.
                // DirEntry already supplies their type; avoid metadata and
                // canonicalization unless a regular file will be returned.
                let matches = path.strip_prefix(&root).is_ok_and(|relative| {
                    relative
                        .to_string_lossy()
                        .replace('\\', "/")
                        .to_lowercase()
                        .contains(&query)
                });
                if !matches {
                    continue;
                }
            }
            // Directories always receive the full reparse/containment checks
            // before recursion. Matching files receive the same checks before
            // they become visible results, including Windows junctions.
            if let Ok(Some(entry)) = describe_entry(&root, &path) {
                children.push((path, entry));
            }
        }
        children.sort_by(|a, b| entry_order(&a.1, &b.1));
        // Push folders in reverse so the stack scans them in display order.
        for (path, entry) in children.iter().rev() {
            if entry.is_dir {
                directories.push(path.clone());
            }
        }
        for (_, entry) in children {
            if entry.relative_path.to_lowercase().contains(&query) {
                found.push(entry);
                if found.len() == limit {
                    found.sort_by(entry_order);
                    return Ok(found);
                }
            }
        }
    }
    found.sort_by(entry_order);
    Ok(found)
}

/// Read at most 256 KiB of text/JSON, or the first 128 bytes as a hex preview.
/// Image and audio bodies are served by the separately scoped asset protocol.
pub fn preview(root: &Path, relative: &str) -> Result<FilePreview, String> {
    let components = relative_components(relative)?;
    let root = canonical_root(root)?
        .ok_or_else(|| "The output folder has not been created yet.".to_owned())?;
    let path = contained_path(&root, &components)?;
    let metadata = fs::metadata(&path).map_err(|e| format!("Cannot read file metadata: {e}"))?;
    if !metadata.is_file() {
        return Err("Choose a file to preview.".into());
    }
    let kind = file_kind(&path, false);
    let mut result = FilePreview {
        kind,
        content: String::new(),
        truncated: false,
        size: metadata.len(),
        mime_type: mime_type(&path, kind).into(),
    };
    if matches!(kind, FileKind::Image | FileKind::Audio) {
        return Ok(result);
    }
    let file = File::open(&path).map_err(|e| format!("Cannot open preview: {e}"))?;
    if kind == FileKind::Binary {
        let mut bytes = Vec::with_capacity(128);
        file.take(128)
            .read_to_end(&mut bytes)
            .map_err(|e| format!("Cannot read binary preview: {e}"))?;
        result.content = hex_preview(&bytes);
        result.truncated = metadata.len() > bytes.len() as u64;
        return Ok(result);
    }
    let mut bytes = Vec::with_capacity((metadata.len() as usize).min(MAX_PREVIEW_BYTES + 1));
    file.take((MAX_PREVIEW_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("Cannot read text preview: {e}"))?;
    result.truncated = bytes.len() > MAX_PREVIEW_BYTES || metadata.len() > MAX_PREVIEW_BYTES as u64;
    bytes.truncate(MAX_PREVIEW_BYTES);
    result.content = String::from_utf8_lossy(&bytes).into_owned();
    if kind == FileKind::Json && !result.truncated {
        if let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) {
            if let Ok(formatted) = serde_json::to_string_pretty(&value) {
                result.content = formatted;
            }
        }
    }
    // Pretty formatting can make compact JSON larger than its source. The
    // frontend receives a bounded string even for adversarial compact arrays.
    if result.content.len() > MAX_PREVIEW_BYTES {
        let mut end = MAX_PREVIEW_BYTES;
        while !result.content.is_char_boundary(end) {
            end -= 1;
        }
        result.content.truncate(end);
        result.truncated = true;
    }
    Ok(result)
}

/// Count extracted content without reading the contents of individual files.
pub fn stats(root: &Path) -> Result<LibraryStats, String> {
    let Some(root) = canonical_root(root)? else {
        return Ok(LibraryStats::default());
    };
    let mut stats = LibraryStats::default();
    let mut directories = vec![root.clone()];
    let mut visited = 0usize;
    while let Some(directory) = directories.pop() {
        let items = match fs::read_dir(directory) {
            Ok(items) => items,
            Err(_) => {
                stats.truncated = true;
                continue;
            }
        };
        for item in items.flatten() {
            if is_repository_metadata(&item.path()) {
                continue;
            }
            visited += 1;
            if visited > MAX_SCAN_ENTRIES {
                stats.truncated = true;
                return Ok(stats);
            }
            let entry = match describe_entry(&root, &item.path()) {
                Ok(Some(entry)) => entry,
                Ok(None) => continue,
                Err(_) => {
                    stats.truncated = true;
                    continue;
                }
            };
            if entry.is_dir {
                stats.folders = stats.folders.saturating_add(1);
                directories.push(item.path());
                continue;
            }
            stats.files = stats.files.saturating_add(1);
            stats.bytes = stats.bytes.saturating_add(entry.size);
            match entry.kind {
                FileKind::Image => stats.images += 1,
                FileKind::Audio => stats.audio += 1,
                FileKind::Json => stats.json += 1,
                _ => stats.other += 1,
            }
        }
    }
    Ok(stats)
}

/// Open a selected extracted file (or the output root) in its Windows app.
pub fn open_file(root: &Path, relative: &str) -> Result<(), String> {
    let components = relative_components(relative)?;
    let root = canonical_root(root)?
        .ok_or_else(|| "The output folder has not been created yet.".to_owned())?;
    let path = contained_path(&root, &components)?;
    launch_explorer(&path, false)
}

/// Reveal a selected extracted file in Explorer without executing its contents.
pub fn reveal_file(root: &Path, relative: &str) -> Result<(), String> {
    let components = relative_components(relative)?;
    let root = canonical_root(root)?
        .ok_or_else(|| "The output folder has not been created yet.".to_owned())?;
    let path = contained_path(&root, &components)?;
    launch_explorer(&path, !relative.is_empty())
}

fn launch_explorer(path: &Path, select: bool) -> Result<(), String> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let mut command = Command::new("explorer.exe");
        if select {
            // Explorer accepts /select,<path> as one argument. Command performs
            // Windows quoting; filenames never become shell command text.
            command.arg(format!("/select,{}", display_path(path)));
        } else {
            command.arg(display_path(path));
        }
        command
            .creation_flags(0x0800_0000)
            .spawn()
            .map_err(|e| format!("Cannot open Explorer: {e}"))?;
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let target = if select {
            path.parent().unwrap_or(path)
        } else {
            path
        };
        let program = if cfg!(target_os = "macos") {
            "open"
        } else {
            "xdg-open"
        };
        Command::new(program)
            .arg(target)
            .spawn()
            .map_err(|e| format!("Cannot open file: {e}"))?;
        Ok(())
    }
}

fn canonical_root(root: &Path) -> Result<Option<PathBuf>, String> {
    let metadata = match fs::symlink_metadata(root) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("Cannot inspect output folder: {error}")),
    };
    if is_link_or_reparse(&metadata) {
        return Err("The output folder must be a regular folder, not a link or junction.".into());
    }
    if !metadata.is_dir() {
        return Err("The output path is not a folder.".into());
    }
    fs::canonicalize(root)
        .map(Some)
        .map_err(|e| format!("Cannot resolve output folder: {e}"))
}

fn relative_components(relative: &str) -> Result<Vec<String>, String> {
    if relative.is_empty() {
        return Ok(Vec::new());
    }
    let normalized = relative.replace('\\', "/");
    if normalized.len() > 32_768
        || normalized.starts_with('/')
        || normalized.contains(':')
        || normalized.chars().any(char::is_control)
    {
        return Err("Library paths must stay inside the output folder.".into());
    }
    let mut components = Vec::new();
    for component in normalized.split('/') {
        if component.is_empty() || component == "." || component == ".." {
            return Err("Library paths cannot contain parent or empty components.".into());
        }
        components.push(component.to_owned());
    }
    Ok(components)
}

fn contained_path(root: &Path, components: &[String]) -> Result<PathBuf, String> {
    let mut path = root.to_path_buf();
    for component in components {
        path.push(component);
        let metadata = fs::symlink_metadata(&path)
            .map_err(|e| format!("The selected file is no longer available: {e}"))?;
        if is_link_or_reparse(&metadata) {
            return Err("Library links and junctions are not followed.".into());
        }
        path = fs::canonicalize(&path)
            .map_err(|e| format!("Cannot resolve the selected file: {e}"))?;
        if !path.starts_with(root) {
            return Err("The selected file is outside the output folder.".into());
        }
    }
    Ok(path)
}

fn describe_entry(root: &Path, path: &Path) -> Result<Option<LibraryEntry>, String> {
    let metadata = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if is_link_or_reparse(&metadata) || (!metadata.is_file() && !metadata.is_dir()) {
        return Ok(None);
    }
    let canonical = fs::canonicalize(path).map_err(|e| e.to_string())?;
    let relative = canonical
        .strip_prefix(root)
        .map_err(|_| "A library entry resolved outside the output folder.".to_owned())?;
    let relative_path = relative.to_string_lossy().replace('\\', "/");
    if relative_path.is_empty() {
        return Ok(None);
    }
    let is_dir = metadata.is_dir();
    Ok(Some(LibraryEntry {
        name: path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned(),
        relative_path,
        absolute_path: display_path(&canonical),
        is_dir,
        kind: file_kind(path, is_dir),
        size: if is_dir { 0 } else { metadata.len() },
        modified: metadata.modified().ok().and_then(|time| {
            time.duration_since(UNIX_EPOCH)
                .ok()
                .map(|duration| duration.as_millis().min(u64::MAX as u128) as u64)
        }),
    }))
}

fn display_path(path: &Path) -> String {
    let value = path.to_string_lossy().into_owned();
    #[cfg(windows)]
    {
        if let Some(unc) = value.strip_prefix(r"\\?\UNC\") {
            return format!(r"\\{unc}");
        }
        if let Some(ordinary) = value.strip_prefix(r"\\?\") {
            return ordinary.to_owned();
        }
    }
    value
}

fn is_link_or_reparse(metadata: &Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        false
    }
}

fn is_repository_metadata(path: &Path) -> bool {
    path.file_name().is_some_and(|name| name == ".git")
}

fn entry_order(a: &LibraryEntry, b: &LibraryEntry) -> Ordering {
    b.is_dir
        .cmp(&a.is_dir)
        .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        .then_with(|| a.relative_path.cmp(&b.relative_path))
}

fn file_kind(path: &Path, directory: bool) -> FileKind {
    if directory {
        return FileKind::Folder;
    }
    let extension = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match extension.as_str() {
        "png" | "jpg" | "jpeg" | "webp" | "gif" | "bmp" | "ico" | "avif" | "svg" => FileKind::Image,
        "ogg" | "opus" | "wav" | "mp3" | "m4a" | "aac" | "flac" | "aif" | "aiff" => FileKind::Audio,
        "json" => FileKind::Json,
        "txt" | "md" | "csv" | "tsv" | "jsonl" | "log" | "toml" | "yaml" | "yml" | "ini"
        | "xml" | "html" | "css" | "js" | "rs" | "lua" | "luau" | "cfg" | "vert" | "frag"
        | "shader" => FileKind::Text,
        _ => FileKind::Binary,
    }
}

fn mime_type(path: &Path, kind: FileKind) -> &'static str {
    let extension = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match extension.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "bmp" => "image/bmp",
        "ico" => "image/x-icon",
        "avif" => "image/avif",
        "svg" => "image/svg+xml",
        "ogg" | "opus" => "audio/ogg",
        "wav" => "audio/wav",
        "mp3" => "audio/mpeg",
        "flac" => "audio/flac",
        "m4a" => "audio/mp4",
        "aac" => "audio/aac",
        "json" => "application/json",
        _ if kind == FileKind::Text => "text/plain",
        _ => "application/octet-stream",
    }
}

fn hex_preview(bytes: &[u8]) -> String {
    let mut result = String::new();
    for (row, chunk) in bytes.chunks(16).enumerate() {
        result.push_str(&format!("{:08x}  ", row * 16));
        for index in 0..16 {
            if let Some(byte) = chunk.get(index) {
                result.push_str(&format!("{byte:02x} "));
            } else {
                result.push_str("   ");
            }
            if index == 7 {
                result.push(' ');
            }
        }
        result.push_str(" |");
        for byte in chunk {
            result.push(if byte.is_ascii_graphic() || *byte == b' ' {
                *byte as char
            } else {
                '.'
            });
        }
        result.push_str("|\n");
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "soulkit-library-{}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos(),
                NEXT.fetch_add(1, AtomicOrdering::Relaxed),
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn missing_output_is_an_empty_library_but_malformed_paths_still_fail() {
        let base = TestDirectory::new();
        let missing = base.0.join("not-extracted");
        assert!(list_directory(&missing, "").unwrap().is_empty());
        assert_eq!(stats(&missing).unwrap().files, 0);
        assert!(list_directory(&missing, "../elsewhere").is_err());
        assert!(preview(&missing, "test.json").is_err());
    }

    #[test]
    fn listings_preserve_unicode_and_sort_folders_before_files() {
        let base = TestDirectory::new();
        fs::create_dir(base.0.join("z-folder")).unwrap();
        fs::create_dir(base.0.join("A-folder")).unwrap();
        fs::write(base.0.join("Audio.OGG"), [1, 2, 3]).unwrap();
        fs::write(base.0.join("zürich.txt"), "hello").unwrap();
        fs::write(base.0.join("rules.JSON"), "{}").unwrap();
        let entries = list_directory(&base.0, "").unwrap();
        assert_eq!(
            entries.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(),
            [
                "A-folder",
                "z-folder",
                "Audio.OGG",
                "rules.JSON",
                "zürich.txt"
            ]
        );
        assert_eq!(entries[2].kind, FileKind::Audio);
        assert_eq!(entries[2].size, 3);
        assert_eq!(entries[3].kind, FileKind::Json);
        assert_eq!(entries[4].relative_path, "zürich.txt");
        assert!(!entries[4].absolute_path.starts_with(r"\\?\"));
    }

    #[test]
    fn traversal_absolute_paths_and_alternate_streams_are_rejected() {
        let base = TestDirectory::new();
        fs::write(base.0.join("inside.txt"), "safe").unwrap();
        for path in [
            "../outside.txt",
            "nested/../../outside.txt",
            "/outside.txt",
            r"C:\outside.txt",
            r"\\server\share\file",
            "inside.txt:secret",
            "./inside.txt",
            "a//b",
        ] {
            assert!(list_directory(&base.0, path).is_err(), "{path}");
            assert!(preview(&base.0, path).is_err(), "{path}");
        }
        assert_eq!(preview(&base.0, "inside.txt").unwrap().content, "safe");
    }

    #[test]
    fn json_is_prettified_and_large_utf8_previews_are_bounded() {
        let base = TestDirectory::new();
        fs::write(
            base.0.join("small.json"),
            r#"{"name":"Löhn","items":[1,2]}"#,
        )
        .unwrap();
        let small = preview(&base.0, "small.json").unwrap();
        assert!(small.content.contains("\n"));
        assert!(small.content.contains("Löhn"));
        assert!(!small.truncated);

        let large = "界".repeat(MAX_PREVIEW_BYTES / 3 + 100);
        fs::write(base.0.join("large.txt"), large.as_bytes()).unwrap();
        let large = preview(&base.0, "large.txt").unwrap();
        assert!(large.truncated);
        assert!(large.content.len() <= MAX_PREVIEW_BYTES);
        assert!(std::str::from_utf8(large.content.as_bytes()).is_ok());

        let expanded = format!("[{}]", vec!["0"; 100_000].join(","));
        assert!(expanded.len() < MAX_PREVIEW_BYTES);
        fs::write(base.0.join("expanded.json"), expanded).unwrap();
        let expanded = preview(&base.0, "expanded.json").unwrap();
        assert!(expanded.truncated);
        assert_eq!(expanded.content.len(), MAX_PREVIEW_BYTES);
    }

    #[test]
    fn binary_preview_is_hex_and_does_not_load_an_entire_asset() {
        let base = TestDirectory::new();
        fs::write(base.0.join("script.luac"), vec![0x41; 4096]).unwrap();
        let preview = preview(&base.0, "script.luac").unwrap();
        assert_eq!(preview.kind, FileKind::Binary);
        assert!(preview.truncated);
        assert_eq!(preview.content.lines().count(), 8);
        assert!(preview.content.contains("00000070"));
        assert!(!preview.content.contains("00000080"));
    }

    #[test]
    fn search_is_recursive_and_limited_while_stats_count_each_file_once() {
        let base = TestDirectory::new();
        fs::create_dir_all(base.0.join("audio/dialog")).unwrap();
        fs::create_dir(base.0.join("images")).unwrap();
        fs::create_dir(base.0.join(".git")).unwrap();
        fs::write(base.0.join(".git/private-metadata"), [0; 100]).unwrap();
        fs::write(base.0.join("audio/dialog/Voice.ogg"), [0; 3]).unwrap();
        fs::write(base.0.join("audio/dialog/Voice2.wav"), [0; 4]).unwrap();
        fs::write(base.0.join("images/Test.PNG"), [0; 5]).unwrap();
        fs::write(base.0.join("index.json"), "{}").unwrap();
        fs::write(base.0.join("readme.txt"), "hi").unwrap();
        let matches = search(&base.0, "VOICE", 1).unwrap();
        assert_eq!(matches.len(), 1);
        assert!(matches[0].relative_path.starts_with("audio/dialog/"));
        assert!(search(&base.0, "", 200).unwrap().is_empty());
        let stats = stats(&base.0).unwrap();
        assert_eq!(stats.files, 5);
        assert_eq!(stats.folders, 3);
        assert_eq!(stats.audio, 2);
        assert_eq!(stats.images, 1);
        assert_eq!(stats.json, 1);
        assert_eq!(stats.other, 1);
        assert_eq!(stats.bytes, 16);
        assert!(!stats.truncated);
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
            if [link, target]
                .iter()
                .any(|p| p.to_string_lossy().chars().any(|c| "&|<>^%!\"".contains(c)))
            {
                return false;
            }
            Command::new("cmd.exe")
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
    fn links_and_windows_junctions_cannot_escape_the_library() {
        let base = TestDirectory::new();
        let output = base.0.join("output");
        let outside = base.0.join("private");
        fs::create_dir(&output).unwrap();
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("secret.txt"), "private").unwrap();
        let link = output.join("escape");
        if !directory_link(&link, &outside) {
            return;
        }
        assert!(list_directory(&output, "").unwrap().is_empty());
        assert!(preview(&output, "escape/secret.txt").is_err());
        assert!(list_directory(&output, "escape").is_err());
        assert!(search(&output, "secret", 20).unwrap().is_empty());
        assert_eq!(stats(&output).unwrap().files, 0);
        assert!(list_directory(&link, "").is_err());
        // Remove the link itself before fixture cleanup, never its target.
        fs::remove_dir(link).unwrap();
        assert_eq!(
            fs::read_to_string(outside.join("secret.txt")).unwrap(),
            "private"
        );
    }
}
