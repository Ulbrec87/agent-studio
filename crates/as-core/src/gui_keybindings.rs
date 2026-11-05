//! `gui-keybindings.json`: the desktop app's key bindings (Hydra's `mod+x` accelerators).
//!
//! Not the TUI's `keybindings.json` (prefix-key catalog, see [`crate::keybindings`]): the two files have different formats
//! and never mix. The daemon only *reads and creates* the file; merging with the defaults and validating command ids
//! stays in the front end (`gui/shared/keybindings.ts`), which owns the defaults. Reading is tolerant: a missing or corrupt
//! file is reported, never an error that breaks the app.

use std::io::Write;
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// What a template may contain at most (a rules array of ~30 short entries is under 2 KiB).
pub const MAX_TEMPLATE_BYTES: usize = 64 * 1024;
/// Bigger files are treated as corrupt instead of being read and shipped to every client each second.
pub const MAX_FILE_BYTES: u64 = 1024 * 1024;

/// Answer of `GetGuiKeybindings`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GuiKeybindingsFile {
    /// Absolute path of the file (it may not exist).
    pub path: String,
    pub exists: bool,
    /// The parsed JSON array, untouched; empty when the file is missing or unusable.
    pub rules: Vec<Value>,
    /// Why the file could not be used (not an array, invalid JSON, unreadable, too big).
    pub error: Option<String>,
    /// Cheap change detector: modification time (ms since the epoch) combined with the length.
    pub stamp: String,
}

fn stamp_of(path: &Path) -> String {
    let Ok(meta) = std::fs::metadata(path) else {
        return "missing".into();
    };
    let ms = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_millis());
    format!("{ms}-{}", meta.len())
}

/// Read the file; never fails.
pub fn read(path: &Path) -> GuiKeybindingsFile {
    let mut out = GuiKeybindingsFile {
        path: path.to_string_lossy().into_owned(),
        exists: false,
        rules: Vec::new(),
        error: None,
        stamp: stamp_of(path),
    };
    match std::fs::metadata(path) {
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return out,
        Err(err) => {
            out.error = Some(err.to_string());
            return out;
        }
        Ok(meta) => {
            out.exists = true;
            if meta.len() > MAX_FILE_BYTES {
                out.error = Some("file is too big".into());
                return out;
            }
        }
    }
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) => {
            out.error = Some(err.to_string());
            return out;
        }
    };
    // Editors may add a BOM; an empty file is "no overrides".
    let text = text.trim_start_matches('\u{feff}');
    if text.trim().is_empty() {
        return out;
    }
    match serde_json::from_str::<Value>(text) {
        Ok(Value::Array(rules)) => out.rules = rules,
        Ok(_) => out.error = Some("expected a JSON array of { command, keys } rules".into()),
        Err(err) => out.error = Some(format!("invalid JSON: {err}")),
    }
    out
}

/// Create the file with `content` (a JSON array) unless it exists; `Ok(true)` when it was created. Atomic: the content is
/// written to a temporary file first and then linked into place, which fails when somebody else created the file in the
/// meantime (so a user's file is never replaced, and no reader sees a half-written template).
pub fn create_template(path: &Path, content: &str) -> Result<bool, String> {
    if content.len() > MAX_TEMPLATE_BYTES {
        return Err("template is too big".into());
    }
    if !matches!(serde_json::from_str::<Value>(content), Ok(Value::Array(_))) {
        return Err("template must be a JSON array".into());
    }
    if path.exists() {
        return Ok(false);
    }
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let tmp = path.with_file_name(format!(".{name}.{}.tmp", std::process::id()));
    let write = || -> std::io::Result<()> {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(content.as_bytes())?;
        f.sync_all()
    };
    if let Err(err) = write() {
        let _ = std::fs::remove_file(&tmp);
        return Err(err.to_string());
    }
    let linked = std::fs::hard_link(&tmp, path);
    let result = match linked {
        Ok(()) => Ok(true),
        Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
        // A file system without hard links: fall back to an exclusive create (not atomic for readers, but never replaces).
        Err(_) => std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .and_then(|mut f| f.write_all(content.as_bytes()))
            .map(|()| true)
            .or_else(|err| {
                if err.kind() == std::io::ErrorKind::AlreadyExists {
                    Ok(false)
                } else {
                    Err(err.to_string())
                }
            }),
    };
    let _ = std::fs::remove_file(&tmp);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_is_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let f = read(&dir.path().join("gui-keybindings.json"));
        assert!(!f.exists && f.rules.is_empty() && f.error.is_none());
        assert_eq!(f.stamp, "missing");
    }

    #[test]
    fn reads_rules_untouched_and_tolerates_a_bom_and_empty_files() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("k.json");
        std::fs::write(
            &p,
            "\u{feff}[{\"command\":\"new-agent\",\"keys\":\"mod+t\"}, 7]",
        )
        .unwrap();
        let f = read(&p);
        assert!(f.exists && f.error.is_none());
        assert_eq!(
            f.rules.len(),
            2,
            "validation of single rules is the front end's job"
        );
        std::fs::write(&p, "  \n").unwrap();
        let f = read(&p);
        assert!(f.exists && f.rules.is_empty() && f.error.is_none());
    }

    #[test]
    fn corrupt_files_are_reported() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("k.json");
        std::fs::write(&p, "{ not json").unwrap();
        let f = read(&p);
        assert!(f.exists && f.rules.is_empty());
        assert!(f.error.unwrap().starts_with("invalid JSON"));
        std::fs::write(&p, "{\"a\":1}").unwrap();
        assert!(read(&p).error.unwrap().contains("array"));
    }

    #[test]
    fn stamp_changes_with_the_content() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("k.json");
        std::fs::write(&p, "[]").unwrap();
        let a = read(&p).stamp;
        std::fs::write(&p, "[ ]  ").unwrap();
        assert_ne!(
            a,
            read(&p).stamp,
            "the length differs even within one mtime tick"
        );
    }

    #[test]
    fn template_is_created_once_and_never_replaces_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("sub").join("k.json");
        assert_eq!(create_template(&p, "[]"), Ok(true));
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "[]");
        std::fs::write(&p, "[1]").unwrap();
        assert_eq!(create_template(&p, "[]"), Ok(false));
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "[1]");
        // No temporary files are left behind.
        assert_eq!(std::fs::read_dir(p.parent().unwrap()).unwrap().count(), 1);
    }

    #[test]
    fn template_must_be_a_small_json_array() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("k.json");
        assert!(create_template(&p, "{}").is_err());
        assert!(create_template(&p, "nope").is_err());
        assert!(create_template(&p, &format!("[\"{}\"]", "x".repeat(MAX_TEMPLATE_BYTES))).is_err());
        assert!(!p.exists());
    }
}
