// Adapted from agent-lookout (MIT, © 2026 Idris Olubisi): src/collector/history/historyFiles.ts
//! Everything the history does to the file system, in its own folder and nowhere else.
//! Synchronous: writes are small, and `stop()` must be able to flush before the process exits.
//! Tests replace it with a folder in memory.
//!
//! No method follows a link at the file's own name: a link, a pipe, a device or a folder where a
//! history file should be is never read, written or deleted.

use std::io::{Read, Write};
use std::path::Path;

/// What is at a path, without following a link. `File` is an ordinary file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathKind {
    File,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PathInfo {
    pub kind: PathKind,
    pub size: u64,
}

pub trait HistoryFs: Send {
    /// Makes the folder (and parents), private to the user where the OS allows.
    fn make_folder(&self, dir: &Path) -> std::io::Result<()>;
    /// The names in a folder. `Err` when it cannot list it.
    fn list(&self, dir: &Path) -> std::io::Result<Vec<String>>;
    /// What is at a path without following a link, `None` when nothing is there.
    fn stat(&self, file: &Path) -> std::io::Result<Option<PathInfo>>;
    /// The text of an ordinary file of at most `max_bytes`. `Err` for anything else.
    fn read(&self, file: &Path, max_bytes: u64) -> std::io::Result<String>;
    /// Adds to the end of an ordinary file, making it (mode 600) when it is not there.
    fn append(&self, file: &Path, text: &str) -> std::io::Result<()>;
    /// Deletes an ordinary file. One that is not there is no error.
    fn remove(&self, file: &Path) -> std::io::Result<()>;
}

pub struct StdHistoryFs;

fn not_ordinary() -> std::io::Error {
    std::io::Error::other("not an ordinary file")
}

impl HistoryFs for StdHistoryFs {
    fn make_folder(&self, dir: &Path) -> std::io::Result<()> {
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

    fn list(&self, dir: &Path) -> std::io::Result<Vec<String>> {
        Ok(std::fs::read_dir(dir)?
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect())
    }

    fn stat(&self, file: &Path) -> std::io::Result<Option<PathInfo>> {
        match std::fs::symlink_metadata(file) {
            Ok(m) => Ok(Some(PathInfo {
                kind: if m.is_file() {
                    PathKind::File
                } else {
                    PathKind::Other
                },
                size: m.len(),
            })),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }

    fn read(&self, file: &Path, max_bytes: u64) -> std::io::Result<String> {
        if !std::fs::symlink_metadata(file)?.is_file() {
            return Err(not_ordinary());
        }
        let handle = std::fs::File::open(file)?;
        let meta = handle.metadata()?;
        if !meta.is_file() {
            return Err(not_ordinary());
        }
        if meta.len() > max_bytes {
            return Err(std::io::Error::other("too large to be a history file"));
        }
        let mut bytes = Vec::with_capacity(usize::try_from(meta.len()).unwrap_or(0));
        handle.take(max_bytes).read_to_end(&mut bytes)?;
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }

    fn append(&self, file: &Path, text: &str) -> std::io::Result<()> {
        if let Ok(meta) = std::fs::symlink_metadata(file)
            && !meta.is_file()
        {
            return Err(not_ordinary());
        }
        let mut options = std::fs::OpenOptions::new();
        options.append(true).create(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        options.open(file)?.write_all(text.as_bytes())
    }

    fn remove(&self, file: &Path) -> std::io::Result<()> {
        match std::fs::symlink_metadata(file) {
            Ok(m) if m.is_dir() => Ok(()),
            Ok(_) => std::fs::remove_file(file),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        }
    }
}
