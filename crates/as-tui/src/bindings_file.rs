//! The user's `keybindings.json`: change detection by polling (Hydra uses `fs.watchFile` with a
//! 500 ms interval) and what "Open keybindings file" has to do.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Remembers the last seen state of the file; [`poll`](Self::poll) reports a change once.
#[derive(Debug)]
pub struct Watcher {
    path: PathBuf,
    last: Option<(SystemTime, u64)>,
}

impl Watcher {
    /// Starts from the file's current state, so [`poll`](Self::poll) reports only later changes.
    pub fn new(path: PathBuf) -> Self {
        let last = Self::stat(&path);
        Self { path, last }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn stat(path: &Path) -> Option<(SystemTime, u64)> {
        let meta = std::fs::metadata(path).ok()?;
        Some((meta.modified().ok()?, meta.len()))
    }

    /// The current text of the file when it changed since the last call (`Some(None)`: the file
    /// was deleted or became unreadable), `None` when nothing changed.
    pub fn poll(&mut self) -> Option<Option<String>> {
        let now = Self::stat(&self.path);
        if now == self.last {
            return None;
        }
        self.last = now;
        Some(self.read())
    }

    /// The current text (`None` when the file does not exist).
    pub fn read(&self) -> Option<String> {
        std::fs::read_to_string(&self.path).ok()
    }
}

/// Create the file with the default bindings when it is missing (so there is something to edit).
pub fn ensure_file(path: &Path) -> anyhow::Result<()> {
    if path.exists() {
        return Ok(());
    }
    as_core::util::write_atomic(
        path,
        as_core::keybindings::default_file_contents().as_bytes(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_creation_edits_and_deletion_once() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("keybindings.json");
        let mut w = Watcher::new(path.clone());
        assert_eq!(w.poll(), None, "nothing yet");
        std::fs::write(&path, "[]").unwrap();
        assert_eq!(w.poll(), Some(Some("[]".to_string())));
        assert_eq!(w.poll(), None);
        // A different length is a change even when the clock is coarse.
        std::fs::write(&path, "[ ]").unwrap();
        assert_eq!(w.poll(), Some(Some("[ ]".to_string())));
        std::fs::remove_file(&path).unwrap();
        assert_eq!(w.poll(), Some(None));
        assert_eq!(w.poll(), None);
    }

    #[test]
    fn existing_file_is_not_reported_at_start() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("keybindings.json");
        std::fs::write(&path, "[]").unwrap();
        let mut w = Watcher::new(path);
        assert_eq!(w.poll(), None);
    }

    #[test]
    fn ensure_file_writes_the_defaults_once() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub").join("keybindings.json");
        ensure_file(&path).unwrap();
        let first = std::fs::read_to_string(&path).unwrap();
        assert!(first.contains("\"toggle-view\""));
        std::fs::write(&path, "[]").unwrap();
        ensure_file(&path).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "[]");
    }
}
