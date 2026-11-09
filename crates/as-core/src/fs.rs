//! Project file access (port of Hydra's `FileSystemService`): reads, search, and [`write_file`].
//!
//! Relative paths on the wire always use `/`. Every function that takes a relative path refuses
//! absolute paths, `..` components and symlinks that leave the project root.
//! No file watcher: clients poll [`file_mtime_ms`] (about once a second) for the files they show.

use std::fs::File;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

use ignore::WalkBuilder;
use nucleo_matcher::pattern::{AtomKind, CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};
use serde::{Deserialize, Serialize};

/// Largest file `read_file` returns in full by default.
pub const DEFAULT_MAX_BYTES: u64 = 1024 * 1024;
const BINARY_SNIFF: usize = 8192;
const MAX_SCANNED: usize = 20_000;

#[derive(Debug, thiserror::Error)]
pub enum FsError {
    #[error("path is outside the project: {0}")]
    OutsideProject(String),
    #[error("{0}: {1}")]
    Io(String, #[source] std::io::Error),
    #[error("content is too large ({0} bytes, max {1})")]
    TooLarge(usize, usize),
    #[error("{0} is a directory")]
    IsDirectory(String),
}

/// Largest content `write_file` accepts (what the editor can hold; far below the protocol's frame limit).
pub const MAX_WRITE_BYTES: usize = 5 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct FsEntry {
    pub name: String,
    /// Relative to the project root, `/` separated.
    pub path: String,
    pub is_dir: bool,
    pub is_symlink: bool,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum FileContent {
    #[serde(rename_all = "camelCase")]
    Text {
        text: String,
        truncated: bool,
        size: u64,
        mtime_ms: u64,
    },
    #[serde(rename_all = "camelCase")]
    Binary { size: u64 },
    #[serde(rename_all = "camelCase")]
    TooLarge { size: u64 },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct SearchHit {
    pub path: String,
    pub name: String,
    pub score: u32,
}

fn io_err(path: &Path, e: std::io::Error) -> FsError {
    FsError::Io(path.display().to_string(), e)
}

/// Resolve `rel` below `root`, refusing anything that escapes it.
pub fn resolve_safe(root: &Path, rel: &str) -> Result<PathBuf, FsError> {
    let deny = || FsError::OutsideProject(rel.to_string());
    let rel_path = Path::new(rel);
    if rel_path.is_absolute() || rel.starts_with('/') || rel.starts_with('\\') {
        return Err(deny());
    }
    let mut joined = root.to_path_buf();
    for c in rel_path.components() {
        match c {
            Component::Normal(p) => joined.push(p),
            Component::CurDir => {}
            _ => return Err(deny()),
        }
    }
    let canon_root = root.canonicalize().map_err(|e| io_err(root, e))?;
    // The target may not exist yet (error surfaces from the read); check the deepest existing ancestor.
    let mut probe = joined.clone();
    let canon = loop {
        match probe.canonicalize() {
            Ok(c) => break c,
            Err(_) => {
                if !probe.pop() {
                    return Err(deny());
                }
            }
        }
    };
    if !canon.starts_with(&canon_root) {
        return Err(deny());
    }
    Ok(joined)
}

fn rel_string(root: &Path, full: &Path) -> String {
    let rel = full.strip_prefix(root).unwrap_or(full);
    rel.components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

/// List one directory: directories first, then case-insensitive by name. `.git` is always skipped.
pub fn list_dir(root: &Path, rel: &str, show_hidden: bool) -> Result<Vec<FsEntry>, FsError> {
    let dir = resolve_safe(root, rel)?;
    let mut out = Vec::new();
    for entry in std::fs::read_dir(&dir)
        .map_err(|e| io_err(&dir, e))?
        .flatten()
    {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name == ".git" || (!show_hidden && name.starts_with('.')) {
            continue;
        }
        let path = entry.path();
        let is_symlink = entry.file_type().map(|t| t.is_symlink()).unwrap_or(false);
        let Ok(meta) = std::fs::metadata(&path) else {
            continue;
        }; // broken symlink
        if !meta.is_file() && !meta.is_dir() {
            continue;
        }
        out.push(FsEntry {
            name,
            path: rel_string(root, &path),
            is_dir: meta.is_dir(),
            is_symlink,
            size: if meta.is_dir() { 0 } else { meta.len() },
        });
    }
    out.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.name.cmp(&b.name))
    });
    Ok(out)
}

pub fn file_mtime_ms(root: &Path, rel: &str) -> Result<u64, FsError> {
    let p = resolve_safe(root, rel)?;
    let meta = std::fs::metadata(&p).map_err(|e| io_err(&p, e))?;
    Ok(mtime_ms(&meta))
}

fn mtime_ms(meta: &std::fs::Metadata) -> u64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_millis() as u64)
}

/// Read a file for preview. Binary files (NUL in the first 8 KB) and files above `max_bytes` yield a placeholder variant.
pub fn read_file(root: &Path, rel: &str, max_bytes: u64) -> Result<FileContent, FsError> {
    let p = resolve_safe(root, rel)?;
    let meta = std::fs::metadata(&p).map_err(|e| io_err(&p, e))?;
    let size = meta.len();
    let mut file = File::open(&p).map_err(|e| io_err(&p, e))?;
    let mut head = Vec::new();
    (&mut file)
        .take(BINARY_SNIFF as u64)
        .read_to_end(&mut head)
        .map_err(|e| io_err(&p, e))?;
    if head.contains(&0) {
        return Ok(FileContent::Binary { size });
    }
    if size > max_bytes {
        return Ok(FileContent::TooLarge { size });
    }
    file.read_to_end(&mut head).map_err(|e| io_err(&p, e))?;
    let text = String::from_utf8_lossy(&head).into_owned();
    Ok(FileContent::Text {
        text,
        truncated: false,
        size,
        mtime_ms: mtime_ms(&meta),
    })
}

/// Replace the text file `rel` below `root` with `content` (the editor's save).
///
/// Same escape protections as the reads (absolute paths, `..`, symlinks leaving the project are refused,
/// including a dangling symlink that points outside), the parent folder must exist, a directory is an
/// error and the content is capped at [`MAX_WRITE_BYTES`]. The write is atomic (temporary file in the same
/// folder, then rename) and keeps the permissions of the file it replaces. A symlink inside the project is
/// followed, so the file it points to is the one replaced (the link stays).
pub fn write_file(root: &Path, rel: &str, content: &str) -> Result<(), FsError> {
    if content.len() > MAX_WRITE_BYTES {
        return Err(FsError::TooLarge(content.len(), MAX_WRITE_BYTES));
    }
    let mut target = resolve_safe(root, rel)?;
    if Path::new(rel)
        .components()
        .all(|c| matches!(c, Component::CurDir))
    {
        return Err(FsError::IsDirectory(rel.to_string()));
    }
    if std::fs::symlink_metadata(&target).is_ok_and(|m| m.file_type().is_symlink()) {
        let canon_root = root.canonicalize().map_err(|e| io_err(root, e))?;
        // `resolve_safe` cannot see where a dangling link points: it must resolve to a place inside the project.
        match target.canonicalize() {
            Ok(real) if real.starts_with(&canon_root) => target = real,
            _ => return Err(FsError::OutsideProject(rel.to_string())),
        }
    }
    let existing = std::fs::metadata(&target).ok();
    if existing.as_ref().is_some_and(|m| m.is_dir()) {
        return Err(FsError::IsDirectory(rel.to_string()));
    }
    if !target.parent().is_some_and(Path::is_dir) {
        let missing =
            std::io::Error::new(std::io::ErrorKind::NotFound, "the folder does not exist");
        return Err(io_err(&target, missing));
    }
    crate::util::write_atomic(&target, content.as_bytes())
        .map_err(|e| io_err(&target, std::io::Error::other(format!("{e:#}"))))?;
    if let Some(meta) = existing {
        // The temporary file was created with default permissions (e.g. the executable bit would be lost).
        let _ = std::fs::set_permissions(&target, meta.permissions());
    }
    Ok(())
}

/// A folder suggestion for the "new agent" dialog (Hydra `DirSuggestion`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct DirSuggestion {
    pub path: String,
    pub name: String,
    pub is_git_repo: bool,
}

const MAX_DIR_SUGGESTIONS: usize = 50;

fn with_separator(dir: &str) -> String {
    if dir.ends_with(['/', '\\']) {
        dir.to_string()
    } else {
        format!("{dir}{}", std::path::MAIN_SEPARATOR)
    }
}

/// Folders matching a typed absolute path (Hydra `fs:list-dirs`): `/home/me/pro` lists the folders of
/// `/home/me` that start with `pro`, `/home/me/` lists all of them; `~` stands for `home`. Dot folders
/// only when the typed prefix starts with a dot. Git repositories first, then by name; at most 50.
/// Relative input and unreadable folders give an empty list.
pub fn list_dirs(query: &str, home: &Path) -> Vec<DirSuggestion> {
    if query.is_empty() {
        return Vec::new();
    }
    // Split the text, not a `Path`: `Path` drops a trailing `.` and the typed prefix may be exactly that.
    let home_text = home.to_string_lossy();
    let expanded: String = if query == "~" {
        format!("{home_text}{}", std::path::MAIN_SEPARATOR)
    } else if let Some(rest) = query
        .strip_prefix("~/")
        .or_else(|| query.strip_prefix("~\\"))
    {
        format!("{}{rest}", with_separator(&home_text))
    } else {
        query.to_string()
    };
    if !Path::new(&expanded).is_absolute() {
        return Vec::new();
    }
    let split = expanded.rfind(['/', '\\']).map_or(0, |i| i + 1);
    let (parent, prefix) = (
        PathBuf::from(&expanded[..split]),
        expanded[split..].to_string(),
    );
    let prefix_lower = prefix.to_lowercase();
    let include_hidden = prefix.starts_with('.');
    let Ok(entries) = std::fs::read_dir(&parent) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        // `metadata` follows symlinks, like Node's `isDirectory()` after a `stat` would.
        if !entry.path().metadata().is_ok_and(|m| m.is_dir()) {
            continue;
        }
        if (!include_hidden && name.starts_with('.'))
            || (!prefix_lower.is_empty() && !name.to_lowercase().starts_with(&prefix_lower))
        {
            continue;
        }
        let full = entry.path();
        out.push(DirSuggestion {
            path: full.to_string_lossy().into_owned(),
            is_git_repo: full.join(".git").exists(),
            name,
        });
        if out.len() >= MAX_DIR_SUGGESTIONS {
            break;
        }
    }
    out.sort_by(|a, b| {
        b.is_git_repo
            .cmp(&a.is_git_repo)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.name.cmp(&b.name))
    });
    out
}

/// Fuzzy search file paths (gitignore aware, hidden files skipped, `.git` excluded).
/// An empty query returns the shallowest files first.
pub fn search_files(root: &Path, query: &str, limit: usize) -> Vec<SearchHit> {
    let mut files: Vec<String> = Vec::new();
    let walker = WalkBuilder::new(root)
        .hidden(true)
        .require_git(false)
        .filter_entry(|e| e.file_name() != ".git")
        .build();
    for entry in walker.flatten() {
        if files.len() >= MAX_SCANNED {
            break;
        }
        if entry.file_type().is_some_and(|t| t.is_file()) {
            files.push(rel_string(root, entry.path()));
        }
    }
    let name_of = |p: &str| p.rsplit('/').next().unwrap_or(p).to_string();
    let query = query.trim();
    let mut hits: Vec<SearchHit> = if query.is_empty() {
        files
            .into_iter()
            .map(|p| SearchHit {
                name: name_of(&p),
                path: p,
                score: 0,
            })
            .collect()
    } else {
        let mut matcher = Matcher::new(Config::DEFAULT.match_paths());
        let pattern = Pattern::new(
            query,
            CaseMatching::Smart,
            Normalization::Smart,
            AtomKind::Fuzzy,
        );
        let mut buf = Vec::new();
        files
            .into_iter()
            .filter_map(|p| {
                let score = pattern.score(Utf32Str::new(&p, &mut buf), &mut matcher)?;
                Some(SearchHit {
                    name: name_of(&p),
                    path: p,
                    score,
                })
            })
            .collect()
    };
    hits.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then_with(|| {
                a.path
                    .matches('/')
                    .count()
                    .cmp(&b.path.matches('/').count())
            })
            .then_with(|| a.path.cmp(&b.path))
    });
    hits.truncate(limit);
    hits
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        let r = d.path();
        std::fs::create_dir_all(r.join("src/deep")).unwrap();
        std::fs::create_dir_all(r.join(".git")).unwrap();
        std::fs::create_dir_all(r.join(".hidden")).unwrap();
        std::fs::write(r.join("README.md"), "hello\n").unwrap();
        std::fs::write(r.join("src/main.rs"), "fn main() {}\n").unwrap();
        std::fs::write(r.join("src/deep/lib.rs"), "x").unwrap();
        std::fs::write(r.join(".env"), "A=1").unwrap();
        std::fs::write(r.join("a.bin"), [1u8, 0, 2]).unwrap();
        std::fs::write(r.join(".gitignore"), "ignored.txt\n").unwrap();
        std::fs::write(r.join("ignored.txt"), "no").unwrap();
        d
    }

    #[test]
    fn lists_dirs_first_and_hides_dotfiles() {
        let d = fixture();
        let names: Vec<_> = list_dir(d.path(), "", false)
            .unwrap()
            .into_iter()
            .map(|e| e.name)
            .collect();
        assert_eq!(names, ["src", "a.bin", "ignored.txt", "README.md"]);
        let all: Vec<_> = list_dir(d.path(), "", true)
            .unwrap()
            .into_iter()
            .map(|e| e.name)
            .collect();
        assert!(all.contains(&".hidden".to_string()) && all.contains(&".env".to_string()));
        assert!(!all.contains(&".git".to_string()));
        let sub = list_dir(d.path(), "src", false).unwrap();
        assert_eq!(sub[0].path, "src/deep");
    }

    #[test]
    fn reads_text_binary_and_too_large() {
        let d = fixture();
        match read_file(d.path(), "README.md", DEFAULT_MAX_BYTES).unwrap() {
            FileContent::Text { text, size, .. } => {
                assert_eq!((text.as_str(), size), ("hello\n", 6))
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(
            read_file(d.path(), "a.bin", DEFAULT_MAX_BYTES).unwrap(),
            FileContent::Binary { size: 3 }
        );
        assert_eq!(
            read_file(d.path(), "README.md", 2).unwrap(),
            FileContent::TooLarge { size: 6 }
        );
        assert!(read_file(d.path(), "missing.txt", 10).is_err());
    }

    #[test]
    fn rejects_traversal() {
        let d = fixture();
        for bad in ["../x", "src/../../x", "/etc/passwd", "\\windows"] {
            assert!(
                matches!(
                    read_file(d.path(), bad, 10),
                    Err(FsError::OutsideProject(_))
                ),
                "{bad}"
            );
        }
        assert!(matches!(
            list_dir(d.path(), "..", false),
            Err(FsError::OutsideProject(_))
        ));
        assert!(file_mtime_ms(d.path(), "README.md").unwrap() > 0);
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlink_escape() {
        let d = fixture();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret"), "s").unwrap();
        std::os::unix::fs::symlink(outside.path(), d.path().join("link")).unwrap();
        assert!(matches!(
            read_file(d.path(), "link/secret", 10),
            Err(FsError::OutsideProject(_))
        ));
        assert!(matches!(
            list_dir(d.path(), "link", false),
            Err(FsError::OutsideProject(_))
        ));
    }

    #[test]
    fn search_respects_gitignore_and_ranks() {
        let d = fixture();
        let hits = search_files(d.path(), "main", 10);
        assert_eq!(hits[0].path, "src/main.rs");
        assert!(search_files(d.path(), "ignored", 10).is_empty());
        assert!(search_files(d.path(), ".env", 10).is_empty());
        let all = search_files(d.path(), "", 100);
        assert_eq!(all[0].path, "README.md"); // shallowest first
        assert!(all.iter().all(|h| !h.path.starts_with(".git/")));
        assert_eq!(search_files(d.path(), "", 2).len(), 2);
    }

    #[test]
    fn writes_replaces_and_leaves_no_temp_file() {
        let d = fixture();
        write_file(d.path(), "README.md", "changed\n").unwrap();
        assert_eq!(
            std::fs::read_to_string(d.path().join("README.md")).unwrap(),
            "changed\n"
        );
        write_file(d.path(), "src/new.rs", "pub fn x() {}\n").unwrap();
        assert!(d.path().join("src/new.rs").is_file());
        write_file(d.path(), "./src/new.rs", "again").unwrap();
        // Unicode and empty content survive.
        write_file(d.path(), "README.md", "").unwrap();
        assert_eq!(std::fs::read(d.path().join("README.md")).unwrap(), b"");
        write_file(d.path(), "README.md", "caf\u{e9} \u{1f600}").unwrap();
        assert_eq!(
            std::fs::read_to_string(d.path().join("README.md")).unwrap(),
            "caf\u{e9} \u{1f600}"
        );
        for dir in [d.path().to_path_buf(), d.path().join("src")] {
            let stray: Vec<_> = std::fs::read_dir(dir)
                .unwrap()
                .flatten()
                .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
                .collect();
            assert!(stray.is_empty(), "{stray:?}");
        }
    }

    #[test]
    fn write_rejects_traversal_and_bad_targets() {
        let d = fixture();
        for bad in ["../x", "src/../../x", "/etc/passwd", "\\windows"] {
            let err = write_file(d.path(), bad, "x");
            assert!(
                matches!(err, Err(FsError::OutsideProject(_))),
                "{bad}: {err:?}"
            );
        }
        assert!(!d.path().parent().unwrap().join("x").exists());
        for dir in ["src", "", ".", "src/deep"] {
            assert!(
                matches!(write_file(d.path(), dir, "x"), Err(FsError::IsDirectory(_))),
                "{dir}"
            );
        }
        // The parent must exist: a save never creates folders.
        assert!(matches!(
            write_file(d.path(), "nope/a.txt", "x"),
            Err(FsError::Io(..))
        ));
        assert!(!d.path().join("nope").exists());
        let big = "x".repeat(MAX_WRITE_BYTES + 1);
        assert!(matches!(
            write_file(d.path(), "big.txt", &big),
            Err(FsError::TooLarge(..))
        ));
        assert!(!d.path().join("big.txt").exists());
    }

    #[cfg(unix)]
    #[test]
    fn write_rejects_symlink_escapes() {
        let d = fixture();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret"), "s").unwrap();
        // A directory link, a file link and a dangling link, all pointing outside.
        std::os::unix::fs::symlink(outside.path(), d.path().join("link")).unwrap();
        std::os::unix::fs::symlink(outside.path().join("secret"), d.path().join("filelink"))
            .unwrap();
        std::os::unix::fs::symlink(outside.path().join("new"), d.path().join("dangling")).unwrap();
        for bad in ["link/secret", "link/new", "filelink", "dangling"] {
            let err = write_file(d.path(), bad, "pwned");
            assert!(
                matches!(err, Err(FsError::OutsideProject(_))),
                "{bad}: {err:?}"
            );
        }
        assert_eq!(
            std::fs::read_to_string(outside.path().join("secret")).unwrap(),
            "s"
        );
        assert!(!outside.path().join("new").exists());
    }

    #[cfg(unix)]
    #[test]
    fn write_follows_inner_symlinks_and_keeps_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let d = fixture();
        let script = d.path().join("run.sh");
        std::fs::write(&script, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        write_file(d.path(), "run.sh", "#!/bin/sh\necho hi\n").unwrap();
        assert_eq!(
            std::fs::metadata(&script).unwrap().permissions().mode() & 0o777,
            0o755
        );
        std::os::unix::fs::symlink(d.path().join("src/main.rs"), d.path().join("alias.rs"))
            .unwrap();
        write_file(d.path(), "alias.rs", "// via alias\n").unwrap();
        assert!(
            std::fs::symlink_metadata(d.path().join("alias.rs"))
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(
            std::fs::read_to_string(d.path().join("src/main.rs")).unwrap(),
            "// via alias\n"
        );
    }

    #[test]
    fn lists_directory_suggestions() {
        let d = tempfile::tempdir().unwrap();
        let r = d.path();
        for n in ["alpha", "Apple", "beta", ".hidden"] {
            std::fs::create_dir(r.join(n)).unwrap();
        }
        std::fs::create_dir(r.join("alpha/.git")).unwrap();
        std::fs::write(r.join("afile"), "x").unwrap();
        let home = r.join("alpha");
        let names =
            |q: &str| -> Vec<String> { list_dirs(q, &home).into_iter().map(|s| s.name).collect() };
        let base = format!("{}{}", r.display(), std::path::MAIN_SEPARATOR);
        // Git repositories first, then by name; files and dot folders are left out.
        assert_eq!(names(&base), ["alpha", "Apple", "beta"]);
        assert_eq!(names(&format!("{base}A")), ["alpha", "Apple"]);
        assert_eq!(names(&format!("{base}.")), [".hidden"]);
        assert!(list_dirs(&format!("{base}alpha"), &home)[0].is_git_repo);
        assert!(names("relative/path").is_empty() && names("").is_empty());
        assert!(names(&format!("{base}missing{}", std::path::MAIN_SEPARATOR)).is_empty());
        // `~` is the home folder (here `alpha`, holding only `.git`, which is hidden).
        assert!(names("~/").is_empty());
        assert_eq!(list_dirs("~", r).len(), 3);
    }
}
