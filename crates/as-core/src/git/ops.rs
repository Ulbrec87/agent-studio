//! Read and write operations on a repository (Hydra `GitService`).

use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};

use super::error::{GitError, GitResult};
use super::runner::{GitRunner, NETWORK_TIMEOUT, Program, args, pathspec};
use super::types::{GitBranch, GitCommit, GitDiffStats, GitFileContents, detect_language};

/// Diffs longer than this are cut (on a character boundary), like Hydra's 100 000 characters but
/// with more room because the TUI scrolls.
pub const MAX_DIFF_BYTES: usize = 400_000;
/// Untracked files bigger than this are not counted in [`GitRunner::diff_stats`].
const MAX_COUNTED_FILE_BYTES: u64 = 2 * 1024 * 1024;

fn truncate_utf8(mut s: String, max: usize) -> String {
    if s.len() > max {
        let mut cut = max;
        while !s.is_char_boundary(cut) {
            cut -= 1;
        }
        s.truncate(cut);
    }
    s
}

fn check_ref_arg(name: &str, what: &str) -> GitResult<()> {
    if name.trim().is_empty() || name.starts_with('-') || name.contains('\0') {
        return Err(GitError::InvalidInput(format!("invalid {what}: {name:?}")));
    }
    Ok(())
}

/// A repository-relative path: not empty, not absolute, no `..`.
pub(crate) fn check_rel_path(path: &str) -> GitResult<()> {
    let p = Path::new(path);
    let bad = path.is_empty()
        || path.contains('\0')
        || p.is_absolute()
        || path.starts_with('/')
        || p.components()
            .any(|c| matches!(c, Component::ParentDir | Component::Prefix(_)));
    if bad {
        return Err(GitError::InvalidInput(format!("invalid path: {path:?}")));
    }
    Ok(())
}

fn parse_track(track: &str) -> (u32, u32) {
    let num = |key: &str| {
        track
            .split(key)
            .nth(1)
            .and_then(|r| r.split(|c: char| !c.is_ascii_digit()).next())
            .and_then(|n| n.parse().ok())
            .unwrap_or(0)
    };
    (num("ahead "), num("behind "))
}

fn parse_numstat(out: &[u8], files: &mut HashSet<String>) -> (u32, u32) {
    let (mut add, mut del) = (0u32, 0u32);
    let mut toks = out.split(|b| *b == 0).map(String::from_utf8_lossy);
    while let Some(tok) = toks.next() {
        if tok.is_empty() {
            continue;
        }
        let mut parts = tok.splitn(3, '\t');
        let (Some(a), Some(d), Some(p)) = (parts.next(), parts.next(), parts.next()) else {
            continue;
        };
        add += a.parse::<u32>().unwrap_or(0);
        del += d.parse::<u32>().unwrap_or(0);
        if p.is_empty() {
            // Rename: `add\tdel\t\0old\0new\0`.
            let _old = toks.next();
            if let Some(new) = toks.next() {
                files.insert(new.into_owned());
            }
        } else {
            files.insert(p.to_string());
        }
    }
    (add, del)
}

impl GitRunner {
    /// Repository root, or `NotARepo`.
    fn require_root(&self, dir: &Path) -> GitResult<PathBuf> {
        self.repo_root(dir)
    }

    fn has_head(&self, root: &Path) -> bool {
        self.git_ok(root, &args!["rev-parse", "--verify", "-q", "HEAD"])
    }

    /// Last `limit` commits of the current branch (empty for a fresh repository or a folder that
    /// is not a repository).
    pub fn log(&self, dir: &Path, limit: u32) -> GitResult<Vec<GitCommit>> {
        if !self.is_git_repo(dir) || !self.has_head(dir) {
            return Ok(Vec::new());
        }
        let out = self.git_raw(
            dir,
            &args![
                "log",
                format!("--max-count={limit}"),
                "-z",
                "--format=%H%x1f%s%x1f%an%x1f%aI"
            ],
        )?;
        let text = out.text();
        Ok(text
            .split('\0')
            .filter(|r| !r.trim().is_empty())
            .filter_map(|rec| {
                let mut f = rec.trim_start_matches('\n').split('\u{1f}');
                let hash = f.next()?.to_string();
                Some(GitCommit {
                    short_hash: hash.chars().take(8).collect(),
                    hash,
                    message: f.next()?.to_string(),
                    author: f.next()?.to_string(),
                    date: f.next()?.to_string(),
                })
            })
            .collect())
    }

    /// Unified diff text. `file: None` is the whole repository; `staged` diffs the index against
    /// HEAD instead of the work tree against the index. An untracked file is diffed against
    /// nothing (all additions). Cut at [`MAX_DIFF_BYTES`].
    pub fn diff(&self, dir: &Path, file: Option<&str>, staged: bool) -> GitResult<String> {
        let root = self.require_root(dir)?;
        let mut a = args![
            "diff",
            "--no-color",
            "--no-ext-diff",
            "--src-prefix=a/",
            "--dst-prefix=b/"
        ];
        if staged {
            a.push("--cached".into());
        }
        if let Some(f) = file {
            check_rel_path(f)?;
            if !staged {
                let others = self.git(
                    &root,
                    &args![
                        "ls-files",
                        "--others",
                        "--exclude-standard",
                        "-z",
                        "--",
                        pathspec(f)
                    ],
                )?;
                if !others.is_empty() {
                    let out = self.exec(
                        Program::Git,
                        &root,
                        &args![
                            "diff",
                            "--no-index",
                            "--no-color",
                            "--no-ext-diff",
                            "--src-prefix=a/",
                            "--dst-prefix=b/",
                            "--",
                            "/dev/null",
                            pathspec(f)
                        ],
                        super::runner::DEFAULT_TIMEOUT,
                        &[1],
                    )?;
                    return Ok(truncate_utf8(out.text(), MAX_DIFF_BYTES));
                }
            }
            a.push("--".into());
            a.push(pathspec(f));
        }
        Ok(truncate_utf8(self.git(&root, &a)?, MAX_DIFF_BYTES))
    }

    /// Files changed, added and deleted lines against HEAD, plus untracked files as pure additions.
    pub fn diff_stats(&self, dir: &Path) -> GitResult<GitDiffStats> {
        if !self.is_git_repo(dir) {
            return Ok(GitDiffStats::default());
        }
        let root = self.require_root(dir)?;
        let mut files = HashSet::new();
        let (mut add, mut del) = (0u32, 0u32);
        let runs: Vec<Vec<std::ffi::OsString>> = if self.has_head(&root) {
            vec![args!["diff", "HEAD", "--numstat", "-z", "--no-ext-diff"]]
        } else {
            vec![
                args!["diff", "--cached", "--numstat", "-z", "--no-ext-diff"],
                args!["diff", "--numstat", "-z", "--no-ext-diff"],
            ]
        };
        for r in runs {
            let out = self.git_raw(&root, &r)?;
            let (a, d) = parse_numstat(&out.stdout, &mut files);
            add += a;
            del += d;
        }
        let untracked = self.git_raw(
            &root,
            &args!["ls-files", "--others", "--exclude-standard", "-z"],
        )?;
        for rel in untracked
            .stdout
            .split(|b| *b == 0)
            .filter(|r| !r.is_empty())
        {
            let rel = String::from_utf8_lossy(rel).into_owned();
            let full = root.join(&rel);
            files.insert(rel);
            let Ok(meta) = std::fs::metadata(&full) else {
                continue;
            };
            if !meta.is_file() || meta.len() == 0 || meta.len() > MAX_COUNTED_FILE_BYTES {
                continue;
            }
            let Ok(bytes) = std::fs::read(&full) else {
                continue;
            };
            if bytes.contains(&0) {
                continue;
            }
            let newlines = bytes.iter().filter(|b| **b == b'\n').count();
            let lines = newlines + usize::from(bytes.last() != Some(&b'\n'));
            add += u32::try_from(lines).unwrap_or(u32::MAX);
        }
        Ok(GitDiffStats {
            files: u32::try_from(files.len()).unwrap_or(u32::MAX),
            additions: add,
            deletions: del,
        })
    }

    /// `git add -- <files>` (paths relative to the repository root).
    pub fn stage(&self, dir: &Path, files: &[String]) -> GitResult<()> {
        let root = self.require_root(dir)?;
        if files.is_empty() {
            return Ok(());
        }
        for f in files {
            check_rel_path(f)?;
        }
        self.with_repo_lock(&root, || {
            let mut a = args!["add", "--"];
            a.extend(files.iter().map(|f| pathspec(f)));
            self.git(&root, &a).map(drop)
        })
    }

    /// Remove `files` from the index, keeping the work tree (works before the first commit too).
    pub fn unstage(&self, dir: &Path, files: &[String]) -> GitResult<()> {
        let root = self.require_root(dir)?;
        if files.is_empty() {
            return Ok(());
        }
        for f in files {
            check_rel_path(f)?;
        }
        self.with_repo_lock(&root, || {
            let mut a = if self.has_head(&root) {
                args!["reset", "-q", "--"]
            } else {
                args!["rm", "--cached", "-r", "-q", "--"]
            };
            a.extend(files.iter().map(|f| pathspec(f)));
            self.git(&root, &a).map(drop)
        })
    }

    /// Stage `files` and commit only them (other staged changes stay staged). With no `files`
    /// the already staged changes are committed. Returns the short hash.
    pub fn commit(&self, dir: &Path, message: &str, files: &[String]) -> GitResult<String> {
        self.do_commit(dir, message, Some(files))
    }

    /// Hydra's "commit all": `git add -A` then commit everything. Returns the short hash.
    pub fn commit_all(&self, dir: &Path, message: &str) -> GitResult<String> {
        self.do_commit(dir, message, None)
    }

    fn do_commit(&self, dir: &Path, message: &str, files: Option<&[String]>) -> GitResult<String> {
        if message.trim().is_empty() {
            return Err(GitError::InvalidInput("empty commit message".into()));
        }
        let root = self.require_root(dir)?;
        for f in files.unwrap_or_default() {
            check_rel_path(f)?;
        }
        self.with_repo_lock(&root, || {
            let mut commit = args!["commit", "-m", message];
            match files {
                None => {
                    self.git(&root, &args!["add", "-A"])?;
                }
                Some(fs) if !fs.is_empty() => {
                    let paths: Vec<_> = fs.iter().map(|f| pathspec(f)).collect();
                    let mut add = args!["add", "--"];
                    add.extend(paths.iter().cloned());
                    self.git(&root, &add)?;
                    commit.push("--only".into());
                    commit.push("--".into());
                    commit.extend(paths);
                }
                Some(_) => {}
            }
            self.git(&root, &commit)?;
            Ok(self
                .git(&root, &args!["rev-parse", "--short", "HEAD"])?
                .trim()
                .to_string())
        })
    }

    /// Push the current branch; without an upstream, `push -u <origin|first remote> <branch>`.
    /// Returns git's progress output.
    pub fn push(&self, dir: &Path) -> GitResult<String> {
        let root = self.require_root(dir)?;
        self.with_repo_lock(&root, || {
            let has_upstream = self.git_ok(
                &root,
                &args![
                    "rev-parse",
                    "--abbrev-ref",
                    "--symbolic-full-name",
                    "@{upstream}"
                ],
            );
            let a = if has_upstream {
                args!["push"]
            } else {
                let branch = self
                    .git(&root, &args!["symbolic-ref", "--short", "-q", "HEAD"])
                    .map_err(|_| {
                        GitError::InvalidInput("HEAD is detached: no branch to push".into())
                    })?;
                let branch = branch.trim().to_string();
                let remotes = self.git(&root, &args!["remote"])?;
                let remotes: Vec<&str> = remotes
                    .lines()
                    .map(str::trim)
                    .filter(|r| !r.is_empty())
                    .collect();
                let remote = remotes
                    .iter()
                    .find(|r| **r == "origin")
                    .or(remotes.first())
                    .ok_or_else(|| GitError::InvalidInput("no git remote configured".into()))?;
                args!["push", "-u", *remote, branch]
            };
            let out = self.exec(Program::Git, &root, &a, NETWORK_TIMEOUT, &[])?;
            Ok(if out.stderr.is_empty() {
                out.text().trim().to_string()
            } else {
                out.stderr
            })
        })
    }

    /// Local and remote branches (`refs/remotes/*/HEAD` aliases are skipped).
    pub fn list_branches(&self, dir: &Path) -> GitResult<Vec<GitBranch>> {
        let root = self.require_root(dir)?;
        let out = self.git(
            &root,
            &args![
                "for-each-ref",
                "--format=%(HEAD)%00%(refname)%00%(refname:short)%00%(upstream:short)%00%(upstream:track,nobracket)",
                "refs/heads",
                "refs/remotes"
            ],
        )?;
        let mut branches = Vec::new();
        for line in out.lines() {
            let f: Vec<&str> = line.split('\0').collect();
            if f.len() < 5 || f[1].ends_with("/HEAD") {
                continue;
            }
            let is_remote = f[1].starts_with("refs/remotes/");
            let (ahead, behind) = parse_track(f[4]);
            branches.push(GitBranch {
                name: f[2].to_string(),
                is_current: f[0] == "*",
                is_remote,
                upstream: (!f[3].is_empty()).then(|| f[3].to_string()),
                ahead_of_upstream: ahead,
                behind_upstream: behind,
            });
        }
        Ok(branches)
    }

    /// Switch branch. A remote branch (`origin/x`) without a local namesake creates a tracking branch.
    pub fn checkout(&self, dir: &Path, branch: &str) -> GitResult<()> {
        check_ref_arg(branch, "branch name")?;
        let root = self.require_root(dir)?;
        self.with_repo_lock(&root, || {
            let local = self.git_ok(
                &root,
                &args!["show-ref", "--verify", "-q", format!("refs/heads/{branch}")],
            );
            let remote = !local
                && self.git_ok(
                    &root,
                    &args![
                        "show-ref",
                        "--verify",
                        "-q",
                        format!("refs/remotes/{branch}")
                    ],
                );
            let a = if remote {
                args!["checkout", "--track", branch]
            } else {
                args!["checkout", branch, "--"]
            };
            self.git(&root, &a).map(drop)
        })
    }

    /// `git checkout -b <name> [<from>]`.
    pub fn create_branch(&self, dir: &Path, name: &str, from: Option<&str>) -> GitResult<()> {
        check_ref_arg(name, "branch name")?;
        if let Some(f) = from {
            check_ref_arg(f, "start point")?;
        }
        let root = self.require_root(dir)?;
        self.with_repo_lock(&root, || {
            let mut a = args!["checkout", "-b", name];
            if let Some(f) = from {
                a.push(f.into());
            }
            self.git(&root, &a).map(drop)
        })
    }

    /// Both sides of a file for a diff view. `staged == false`: index (empty when absent) versus
    /// the work tree; `staged == true`: HEAD (empty when absent) versus the index.
    pub fn file_contents(
        &self,
        dir: &Path,
        path: &str,
        staged: bool,
    ) -> GitResult<GitFileContents> {
        check_rel_path(path)?;
        let root = self.require_root(dir)?;
        let rel = path.replace('\\', "/");
        let show = |spec: String| -> Vec<u8> {
            self.git_raw(&root, &args!["show", spec])
                .map(|o| o.stdout)
                .unwrap_or_default()
        };
        let (original, modified) = if staged {
            (show(format!("HEAD:{rel}")), show(format!(":{rel}")))
        } else {
            (
                show(format!(":{rel}")),
                std::fs::read(root.join(&rel)).unwrap_or_default(),
            )
        };
        let binary = original.contains(&0) || modified.contains(&0);
        Ok(GitFileContents {
            original: String::from_utf8_lossy(&original).into_owned(),
            modified: String::from_utf8_lossy(&modified).into_owned(),
            language: detect_language(&rel).to_string(),
            binary,
        })
    }
}
