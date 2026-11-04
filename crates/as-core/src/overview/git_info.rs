// Adapted from agent-lookout (MIT, © 2026 Idris Olubisi): src/collector/git/repository.ts, gitHead.ts
//! Repository, branch and commit of a folder, read from git's own files (no `git` process per
//! poll: the Overview asks for every agent every few seconds). Walks up to the nearest `.git`
//! (folder, or file naming the git folder); a worktree's `commondir` leads to the repository it
//! belongs to.

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitInfo {
    /// Main working folder, or the bare repository folder (the container for `.bare`).
    pub repo_root: PathBuf,
    pub repo_name: String,
    pub branch: Option<String>,
    /// Short commit id, only when HEAD is detached.
    pub commit: Option<String>,
}

/// Files are one short line; a larger one gives no answer.
const MAX_GIT_FILE_BYTES: u64 = 4 * 1024;
const SHORT_COMMIT_LENGTH: usize = 7;
const MAX_BRANCH_LENGTH: usize = 100;
const BRANCH_REF: &str = "refs/heads/";
const MAX_WALK_UP: usize = 64;

fn read_small_file(path: &Path) -> Option<String> {
    let meta = std::fs::symlink_metadata(path).ok()?;
    if !meta.is_file() || meta.len() > MAX_GIT_FILE_BYTES {
        return None;
    }
    std::fs::read_to_string(path).ok()
}

fn one_line(content: &str) -> Option<&str> {
    let line = content.strip_suffix('\n').unwrap_or(content);
    let line = line.strip_suffix('\r').unwrap_or(line);
    (!line.contains(['\r', '\n'])).then_some(line)
}

fn path_in(text: &str) -> Option<&str> {
    let target = text.trim();
    (!target.is_empty() && !target.chars().any(char::is_control)).then_some(target)
}

fn is_commit_id(id: &str) -> bool {
    matches!(id.len(), 40 | 64) && id.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// Control characters and bidi reordering marks, which git allows in a branch name.
fn is_unsafe_branch_char(c: char) -> bool {
    c.is_control() || matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
}

/// `HEAD`: the branch, or the short commit id when detached; `None` for anything else.
pub fn parse_head(content: &str) -> Option<(Option<String>, Option<String>)> {
    let line = one_line(content)?;
    if let Some(rest) = line.strip_prefix("ref:") {
        let reference = rest.trim();
        let branch: String = reference
            .strip_prefix(BRANCH_REF)?
            .chars()
            .filter(|c| !is_unsafe_branch_char(*c))
            .collect();
        if branch.is_empty() {
            return None;
        }
        return Some((Some(branch.chars().take(MAX_BRANCH_LENGTH).collect()), None));
    }
    let id = line.trim();
    is_commit_id(id).then(|| (None, Some(id.chars().take(SHORT_COMMIT_LENGTH).collect())))
}

/// The path a `.git` file (`gitdir: <path>`) names.
pub fn parse_git_file(content: &str) -> Option<String> {
    let line = one_line(content)?;
    path_in(line.strip_prefix("gitdir:")?).map(str::to_string)
}

/// Lexical normalisation (like Node's `path.resolve`): no symlink resolution, `..` pops.
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn resolve_from(base: &Path, target: &str) -> PathBuf {
    let target = Path::new(target);
    if target.is_absolute() {
        normalize(target)
    } else {
        normalize(&base.join(target))
    }
}

fn base_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Name of a shared git folder: the folder holding it when hidden (`.git`, `.bare`), else its
/// name without `.git`.
fn shared_name(common: &Path) -> String {
    let base = base_name(common);
    if base.starts_with('.') {
        return common.parent().map(base_name).unwrap_or_default();
    }
    base.strip_suffix(".git")
        .map_or(base.clone(), str::to_string)
}

fn shared_root(common: &Path) -> PathBuf {
    if base_name(common).starts_with('.') {
        common.parent().map(Path::to_path_buf).unwrap_or_default()
    } else {
        common.to_path_buf()
    }
}

/// Reads repository, branch and commit of a folder from git's own files. `None` when there is no
/// repository or nothing could be read.
pub fn read_git_info(project_dir: &Path) -> Option<GitInfo> {
    let mut dir = normalize(&std::path::absolute(project_dir).ok()?);
    for _ in 0..MAX_WALK_UP {
        let dot_git = dir.join(".git");
        match std::fs::symlink_metadata(&dot_git) {
            Ok(meta) if meta.is_dir() => return info_for(&dir, &dot_git, None),
            Ok(meta) if meta.is_file() => {
                let target = parse_git_file(&read_small_file(&dot_git)?)?;
                let git_dir = resolve_from(&dir, &target);
                let common = read_small_file(&git_dir.join("commondir")).and_then(|c| {
                    one_line(&c)
                        .and_then(path_in)
                        .map(|t| resolve_from(&git_dir, t))
                });
                return info_for(&dir, &git_dir, common.as_deref());
            }
            _ => {}
        }
        match dir.parent() {
            Some(parent) if parent != dir => dir = parent.to_path_buf(),
            _ => return None,
        }
    }
    None
}

fn info_for(dir: &Path, git_dir: &Path, common: Option<&Path>) -> Option<GitInfo> {
    let repo_name = match common {
        None => base_name(dir),
        Some(c) => shared_name(c),
    };
    if repo_name.is_empty() {
        return None;
    }
    let repo_root = common.map_or_else(|| dir.to_path_buf(), shared_root);
    let head = read_small_file(&git_dir.join("HEAD")).and_then(|h| parse_head(&h));
    let (branch, commit) = head.unwrap_or((None, None));
    Some(GitInfo {
        repo_root,
        repo_name,
        branch,
        commit,
    })
}

pub const GIT_INFO_REFRESH_MS: i64 = 10_000;

/// Reads the git info of a folder (injectable for tests).
pub type GitReader = Box<dyn Fn(&Path) -> Option<GitInfo> + Send>;

/// Caches [`read_git_info`] per project folder, re-reading at most every 10 s.
pub struct GitInfoCache {
    entries: HashMap<String, (i64, Option<GitInfo>)>,
    read: GitReader,
    refresh_ms: i64,
}

impl Default for GitInfoCache {
    fn default() -> Self {
        Self::new(Box::new(read_git_info), GIT_INFO_REFRESH_MS)
    }
}

impl GitInfoCache {
    pub fn new(read: GitReader, refresh_ms: i64) -> Self {
        Self {
            entries: HashMap::new(),
            read,
            refresh_ms,
        }
    }

    pub fn get(&mut self, project_dir: &str, now: i64) -> Option<GitInfo> {
        if let Some((at, info)) = self.entries.get(project_dir)
            && now - at < self.refresh_ms
        {
            return info.clone();
        }
        let info = (self.read)(Path::new(project_dir));
        self.entries
            .insert(project_dir.to_string(), (now, info.clone()));
        info
    }

    /// Drops the folders not in `project_dirs`.
    pub fn retain<'a>(&mut self, project_dirs: impl IntoIterator<Item = &'a str>) {
        let keep: std::collections::HashSet<&str> = project_dirs.into_iter().collect();
        self.entries.retain(|dir, _| keep.contains(dir.as_str()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    const SHA: &str = "a1b2c3d4e5f60718293a4b5c6d7e8f9012345678";

    fn root() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }
    fn norm(p: &Path) -> PathBuf {
        normalize(p)
    }

    #[test]
    fn reads_a_normal_repository_from_a_subfolder() {
        let tmp = root();
        let repo = tmp.path().join("shop");
        fs::create_dir_all(repo.join(".git")).unwrap();
        fs::create_dir_all(repo.join("src/deep")).unwrap();
        fs::write(repo.join(".git/HEAD"), "ref: refs/heads/feat/cart\n").unwrap();
        assert_eq!(
            read_git_info(&repo.join("src/deep")),
            Some(GitInfo {
                repo_root: norm(&repo),
                repo_name: "shop".into(),
                branch: Some("feat/cart".into()),
                commit: None
            })
        );
    }

    #[test]
    fn groups_a_worktree_under_its_main_repository() {
        let tmp = root();
        let repo = tmp.path().join("shop");
        let wt_git = repo.join(".git/worktrees/wt");
        fs::create_dir_all(&wt_git).unwrap();
        fs::write(repo.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
        fs::write(wt_git.join("HEAD"), "ref: refs/heads/fix\n").unwrap();
        fs::write(wt_git.join("commondir"), "../..\n").unwrap();
        let wt = tmp.path().join("wt-folder");
        fs::create_dir(&wt).unwrap();
        fs::write(wt.join(".git"), format!("gitdir: {}\n", wt_git.display())).unwrap();
        assert_eq!(
            read_git_info(&wt),
            Some(GitInfo {
                repo_root: norm(&repo),
                repo_name: "shop".into(),
                branch: Some("fix".into()),
                commit: None
            })
        );
    }

    #[test]
    fn names_a_repository_after_the_folder_beside_bare() {
        let tmp = root();
        let wt_git = tmp.path().join("storefront/.bare/worktrees/main");
        fs::create_dir_all(&wt_git).unwrap();
        fs::write(wt_git.join("HEAD"), "ref: refs/heads/main\n").unwrap();
        fs::write(wt_git.join("commondir"), "../..\n").unwrap();
        let wt = tmp.path().join("storefront/main");
        fs::create_dir(&wt).unwrap();
        fs::write(wt.join(".git"), "gitdir: ../.bare/worktrees/main\n").unwrap();
        let info = read_git_info(&wt).unwrap();
        assert_eq!(info.repo_name, "storefront");
        assert_eq!(info.repo_root, norm(&tmp.path().join("storefront")));
        assert_eq!(info.branch.as_deref(), Some("main"));
    }

    #[test]
    fn strips_dot_git_from_a_bare_repository_name() {
        let tmp = root();
        let bare = tmp.path().join("tool.git");
        let wt_git = bare.join("worktrees/a");
        fs::create_dir_all(&wt_git).unwrap();
        fs::write(wt_git.join("HEAD"), "ref: refs/heads/a\n").unwrap();
        fs::write(wt_git.join("commondir"), "../..").unwrap();
        let wt = tmp.path().join("a");
        fs::create_dir(&wt).unwrap();
        fs::write(wt.join(".git"), format!("gitdir: {}", wt_git.display())).unwrap();
        let info = read_git_info(&wt).unwrap();
        assert_eq!(info.repo_name, "tool");
        assert_eq!(info.repo_root, norm(&bare));
    }

    #[test]
    fn a_detached_head_is_a_short_commit() {
        let tmp = root();
        let repo = tmp.path().join("r");
        fs::create_dir_all(repo.join(".git")).unwrap();
        fs::write(repo.join(".git/HEAD"), format!("{SHA}\n")).unwrap();
        let info = read_git_info(&repo).unwrap();
        assert_eq!(
            (info.branch, info.commit.as_deref()),
            (None, Some("a1b2c3d"))
        );
    }

    #[test]
    fn none_outside_a_repository_and_for_an_unreadable_git_file() {
        let tmp = root();
        let plain = tmp.path().join("plain");
        fs::create_dir(&plain).unwrap();
        // The walk goes up to the filesystem root: only assert when no ancestor is a repo.
        if !tmp.path().ancestors().any(|a| a.join(".git").exists()) {
            assert_eq!(read_git_info(&plain), None);
        }
        let odd = tmp.path().join("odd");
        fs::create_dir(&odd).unwrap();
        fs::write(odd.join(".git"), "nonsense").unwrap();
        assert_eq!(read_git_info(&odd), None);
    }

    #[test]
    fn parses_head_strictly() {
        assert_eq!(parse_head("ref: refs/tags/v1\n"), None);
        assert_eq!(parse_head("garbage"), None);
        assert_eq!(parse_head("ref: refs/heads/a\nb"), None);
        assert_eq!(
            parse_head("ref: refs/heads/main\n"),
            Some((Some("main".into()), None))
        );
    }

    #[test]
    fn the_cache_rereads_at_most_every_10_seconds() {
        let calls = Arc::new(AtomicUsize::new(0));
        let c = calls.clone();
        let mut cache = GitInfoCache::new(
            Box::new(move |_| {
                c.fetch_add(1, Ordering::SeqCst);
                None
            }),
            GIT_INFO_REFRESH_MS,
        );
        cache.get("/a", 1000);
        cache.get("/a", 10_000);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        cache.get("/a", 12_000);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        cache.get("/b", 12_000);
        assert_eq!(calls.load(Ordering::SeqCst), 3);
        cache.retain(["/a"]);
        cache.get("/b", 12_001);
        assert_eq!(calls.load(Ordering::SeqCst), 4);
    }
}
