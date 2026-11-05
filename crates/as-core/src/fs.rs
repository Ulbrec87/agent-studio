//! Read-only project file access (port of Hydra's `FileSystemService`).
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
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FsEntry {
    pub name: String,
    /// Relative to the project root, `/` separated.
    pub path: String,
    pub is_dir: bool,
    pub is_symlink: bool,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
}
