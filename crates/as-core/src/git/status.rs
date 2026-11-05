//! `git status --porcelain=v2 --branch -z` parsing and [`GitRunner::status`].

use std::path::Path;

use super::error::{GitError, GitResult};
use super::runner::{GitRunner, args};
use super::types::{GitFileStatus, GitStatus, GitStatusKind};

fn kind_of(c: u8) -> Option<GitStatusKind> {
    match c {
        b'M' => Some(GitStatusKind::Modified),
        b'A' => Some(GitStatusKind::Added),
        b'D' => Some(GitStatusKind::Deleted),
        b'R' => Some(GitStatusKind::Renamed),
        b'C' => Some(GitStatusKind::Copied),
        b'T' => Some(GitStatusKind::TypeChanged),
        b'U' => Some(GitStatusKind::Unmerged),
        _ => None,
    }
}

fn entry(xy: &[u8], path: &str, orig: Option<String>) -> Option<GitFileStatus> {
    if xy.len() < 2 {
        return None;
    }
    let (index, worktree) = (kind_of(xy[0]), kind_of(xy[1]));
    let conflicted = index == Some(GitStatusKind::Unmerged)
        || worktree == Some(GitStatusKind::Unmerged)
        || (xy[0] == b'A' && xy[1] == b'A')
        || (xy[0] == b'D' && xy[1] == b'D');
    let (index, worktree) = if conflicted {
        (None, None)
    } else {
        (index, worktree)
    };
    let kind = if conflicted {
        GitStatusKind::Unmerged
    } else {
        index.or(worktree)?
    };
    Some(GitFileStatus {
        path: path.to_string(),
        orig_path: orig,
        kind,
        index,
        worktree,
        staged: index.is_some(),
        unstaged: worktree.is_some(),
        untracked: false,
        conflicted,
    })
}

/// Parse the output of `git status --porcelain=v2 --branch -z --untracked-files=all`.
/// Paths are taken as UTF-8 (lossy).
pub fn parse_status_v2(out: &[u8]) -> GitStatus {
    let mut st = GitStatus {
        is_repo: true,
        ..GitStatus::default()
    };
    let mut records = out
        .split(|b| *b == 0)
        .map(|r| String::from_utf8_lossy(r).into_owned());
    while let Some(rec) = records.next() {
        if rec.is_empty() {
            continue;
        }
        if let Some(h) = rec.strip_prefix("# ") {
            let (key, val) = h.split_once(' ').unwrap_or((h, ""));
            match key {
                "branch.oid" => {
                    if val != "(initial)" {
                        st.head = Some(val.chars().take(8).collect());
                    }
                }
                "branch.head" => {
                    if val == "(detached)" {
                        st.detached = true;
                    } else {
                        st.branch = val.to_string();
                    }
                }
                "branch.upstream" => st.upstream = Some(val.to_string()),
                "branch.ab" => {
                    for part in val.split_whitespace() {
                        if let Some(n) = part.strip_prefix('+') {
                            st.ahead = n.parse().unwrap_or(0);
                        } else if let Some(n) = part.strip_prefix('-') {
                            st.behind = n.parse().unwrap_or(0);
                        }
                    }
                }
                _ => {}
            }
            continue;
        }
        let kind = rec.as_bytes()[0];
        match kind {
            b'1' => {
                let f: Vec<&str> = rec.splitn(9, ' ').collect();
                if f.len() == 9 {
                    st.files.extend(entry(f[1].as_bytes(), f[8], None));
                }
            }
            b'2' => {
                let f: Vec<&str> = rec.splitn(10, ' ').collect();
                let orig = records.next().filter(|o| !o.is_empty());
                if f.len() == 10 {
                    st.files.extend(entry(f[1].as_bytes(), f[9], orig));
                }
            }
            b'u' => {
                let f: Vec<&str> = rec.splitn(11, ' ').collect();
                if f.len() == 11 {
                    st.files.extend(entry(b"UU", f[10], None));
                }
            }
            b'?' => {
                if let Some(path) = rec.strip_prefix("? ") {
                    st.files.push(GitFileStatus {
                        path: path.to_string(),
                        orig_path: None,
                        kind: GitStatusKind::Untracked,
                        index: None,
                        worktree: None,
                        staged: false,
                        unstaged: false,
                        untracked: true,
                        conflicted: false,
                    });
                }
            }
            _ => {}
        }
    }
    st
}

impl GitRunner {
    /// `true` when `dir` is inside a git work tree.
    pub fn is_git_repo(&self, dir: &Path) -> bool {
        self.git(dir, &args!["rev-parse", "--is-inside-work-tree"])
            .is_ok_and(|o| o.trim() == "true")
    }

    /// Branch, upstream, ahead/behind and changed files. A folder that is not a repository gives
    /// the default (empty, `is_repo: false`) status like Hydra.
    pub fn status(&self, dir: &Path) -> GitResult<GitStatus> {
        if !self.is_git_repo(dir) {
            return Ok(GitStatus::default());
        }
        let out = self.git_raw(
            dir,
            &args![
                "status",
                "--porcelain=v2",
                "--branch",
                "-z",
                "--untracked-files=all"
            ],
        );
        match out {
            Ok(o) => Ok(parse_status_v2(&o.stdout)),
            Err(GitError::NotARepo(_)) => Ok(GitStatus::default()),
            Err(e) => Err(e),
        }
    }
}
