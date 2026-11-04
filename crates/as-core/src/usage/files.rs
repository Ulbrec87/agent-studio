//! What the usage code does to the file system (Hydra `electron/usage/usageFiles.ts`). Every call
//! is bounded (walk by depth and file count, reads by a byte cap) and no call follows a link at the
//! file's own name: a link, pipe, device or folder where a log should be is never read.

use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

/// An ordinary log file found by [`walk`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageFileInfo {
    pub path: PathBuf,
    pub size: u64,
    pub mtime_ms: i64,
    /// Inode on Unix, 0 elsewhere (the size check still catches a replaced file).
    pub ino: u64,
}

/// The most files one walk returns.
pub const MAX_WALK_FILES: usize = 50_000;

fn not_ordinary() -> std::io::Error {
    std::io::Error::other("not an ordinary file")
}

#[cfg(unix)]
fn inode(m: &std::fs::Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt;
    m.ino()
}
#[cfg(not(unix))]
fn inode(_: &std::fs::Metadata) -> u64 {
    0
}

fn mtime_ms(m: &std::fs::Metadata) -> i64 {
    m.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_millis() as i64)
}

fn walk_into(dir: &Path, depth: usize, max_depth: usize, out: &mut Vec<UsageFileInfo>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return; // unreadable or gone: skipped
    };
    for entry in entries.filter_map(Result::ok) {
        if out.len() >= MAX_WALK_FILES {
            return;
        }
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        if kind.is_dir() {
            if depth < max_depth {
                walk_into(&path, depth + 1, max_depth, out);
            }
        } else if kind.is_file()
            && path.extension().is_some_and(|e| e == "jsonl")
            && let Ok(m) = std::fs::symlink_metadata(&path)
            && m.is_file()
        {
            out.push(UsageFileInfo {
                path,
                size: m.len(),
                mtime_ms: mtime_ms(&m),
                ino: inode(&m),
            });
        }
    }
}

/// Ordinary `*.jsonl` files under `root` (never through links); a missing root gives `[]`.
pub fn walk(root: &Path, max_depth: usize) -> Vec<UsageFileInfo> {
    let mut out = Vec::new();
    if std::fs::symlink_metadata(root).is_ok_and(|m| m.is_dir()) {
        walk_into(root, 0, max_depth, &mut out);
    }
    out
}

/// Up to `max_bytes` of an ordinary file from `offset`. Errors for anything else.
pub fn read_chunk(path: &Path, offset: u64, max_bytes: usize) -> std::io::Result<Vec<u8>> {
    if !std::fs::symlink_metadata(path)?.is_file() {
        return Err(not_ordinary());
    }
    let mut file = std::fs::File::open(path)?;
    let meta = file.metadata()?;
    if !meta.is_file() {
        return Err(not_ordinary());
    }
    let want = (max_bytes as u64).min(meta.len().saturating_sub(offset)) as usize;
    let mut buf = vec![0u8; want];
    file.seek(SeekFrom::Start(offset))?;
    let mut filled = 0;
    while filled < want {
        let n = file.read(&mut buf[filled..])?;
        if n == 0 {
            break;
        }
        filled += n;
    }
    buf.truncate(filled);
    Ok(buf)
}

/// The text of an ordinary file of at most `max_bytes`; `Ok(None)` when it is not there.
pub fn read_text(path: &Path, max_bytes: u64) -> std::io::Result<Option<String>> {
    let meta = match std::fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    if !meta.is_file() {
        return Err(not_ordinary());
    }
    let file = std::fs::File::open(path)?;
    let opened = file.metadata()?;
    if !opened.is_file() {
        return Err(not_ordinary());
    }
    if opened.len() > max_bytes {
        return Err(std::io::Error::other("too large"));
    }
    let mut bytes = Vec::new();
    file.take(max_bytes).read_to_end(&mut bytes)?;
    Ok(Some(String::from_utf8_lossy(&bytes).into_owned()))
}

/// Makes the folder (and parents), private to the user where the OS allows.
pub fn make_folder(dir: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)
    }
    #[cfg(not(unix))]
    {
        std::fs::create_dir_all(dir)
    }
}

/// Writes `<path>.tmp-<pid>` with mode 600, then renames it over `path`.
pub fn write_atomic_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        make_folder(parent)?;
    }
    if let Ok(m) = std::fs::symlink_metadata(path)
        && !m.is_file()
    {
        return Err(not_ordinary());
    }
    let mut name = path
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_default();
    name.push(format!(".tmp-{}", std::process::id()));
    let tmp = path.with_file_name(name);
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let result = (|| {
        let mut f = options.open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// Everything the store does to the file system, so tests can use files in memory.
pub trait UsageFs: Send + Sync {
    fn walk(&self, root: &Path, max_depth: usize) -> Vec<UsageFileInfo>;
    fn read_chunk(&self, path: &Path, offset: u64, max_bytes: usize) -> std::io::Result<Vec<u8>>;
    fn read_text(&self, path: &Path, max_bytes: u64) -> std::io::Result<Option<String>>;
    fn write_atomic(&self, path: &Path, bytes: &[u8]) -> std::io::Result<()>;
}

/// The real file system.
pub struct StdUsageFs;

impl UsageFs for StdUsageFs {
    fn walk(&self, root: &Path, max_depth: usize) -> Vec<UsageFileInfo> {
        walk(root, max_depth)
    }
    fn read_chunk(&self, path: &Path, offset: u64, max_bytes: usize) -> std::io::Result<Vec<u8>> {
        read_chunk(path, offset, max_bytes)
    }
    fn read_text(&self, path: &Path, max_bytes: u64) -> std::io::Result<Option<String>> {
        read_text(path, max_bytes)
    }
    fn write_atomic(&self, path: &Path, bytes: &[u8]) -> std::io::Result<()> {
        write_atomic_private(path, bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn walks_jsonl_files_by_depth_and_skips_links() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("a/b/c")).unwrap();
        std::fs::write(root.join("top.jsonl"), "1").unwrap();
        std::fs::write(root.join("a/b/x.jsonl"), "22").unwrap();
        std::fs::write(root.join("a/b/c/deep.jsonl"), "333").unwrap();
        std::fs::write(root.join("a/readme.txt"), "no").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(root.join("top.jsonl"), root.join("a/link.jsonl")).unwrap();
        let names = |depth| {
            let mut v: Vec<_> = walk(root, depth)
                .iter()
                .map(|f| f.path.file_name().unwrap().to_string_lossy().into_owned())
                .collect();
            v.sort();
            v
        };
        assert_eq!(names(2), ["top.jsonl", "x.jsonl"]);
        assert_eq!(names(3), ["deep.jsonl", "top.jsonl", "x.jsonl"]);
        assert!(walk(&root.join("missing"), 3).is_empty());
    }

    #[test]
    fn reads_chunks_from_an_offset() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("f.jsonl");
        std::fs::write(&f, "hello world").unwrap();
        assert_eq!(read_chunk(&f, 6, 100).unwrap(), b"world");
        assert_eq!(read_chunk(&f, 0, 5).unwrap(), b"hello");
        assert!(read_chunk(&f, 50, 5).unwrap().is_empty());
        assert!(read_chunk(dir.path(), 0, 5).is_err());
    }

    #[test]
    fn writes_atomically_with_mode_600() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("sub/cache.json");
        write_atomic_private(&f, b"one").unwrap();
        write_atomic_private(&f, b"two").unwrap();
        assert_eq!(read_text(&f, 100).unwrap().as_deref(), Some("two"));
        assert_eq!(std::fs::read_dir(f.parent().unwrap()).unwrap().count(), 1);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&f).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        assert_eq!(read_text(&dir.path().join("nope"), 10).unwrap(), None);
        assert!(read_text(&f, 1).is_err());
    }
}
