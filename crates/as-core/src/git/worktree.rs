//! `git worktree` support (Hydra `createWorktree` / `removeWorktree`).

use std::path::{Path, PathBuf};

use super::error::{GitError, GitResult};
use super::runner::{GitRunner, args};
use super::types::GitWorktree;

/// Parse `git worktree list --porcelain`.
pub fn parse_worktree_list(out: &str) -> Vec<GitWorktree> {
    let mut list = Vec::new();
    let mut cur: Option<GitWorktree> = None;
    for line in out.lines() {
        let line = line.trim_end_matches('\r');
        if let Some(p) = line.strip_prefix("worktree ") {
            list.extend(cur.take());
            cur = Some(GitWorktree {
                path: p.to_string(),
                ..GitWorktree::default()
            });
            continue;
        }
        let Some(w) = cur.as_mut() else { continue };
        let (key, val) = line.split_once(' ').unwrap_or((line, ""));
        match key {
            "HEAD" => w.head = Some(val.chars().take(8).collect()),
            "branch" => w.branch = Some(val.strip_prefix("refs/heads/").unwrap_or(val).to_string()),
            "bare" => w.bare = true,
            "detached" => w.detached = true,
            "locked" => w.locked = true,
            "prunable" => w.prunable = true,
            _ => {}
        }
    }
    list.extend(cur);
    list
}

impl GitRunner {
    /// All worktrees of the repository (the main one first).
    pub fn worktree_list(&self, dir: &Path) -> GitResult<Vec<GitWorktree>> {
        let out = self.git(dir, &args!["worktree", "list", "--porcelain"])?;
        Ok(parse_worktree_list(&out))
    }

    /// Folder of the main worktree (or the bare repository) that `dir`, possibly a linked
    /// worktree, belongs to.
    pub fn main_worktree(&self, dir: &Path) -> GitResult<PathBuf> {
        self.worktree_list(dir)?
            .into_iter()
            .next()
            .map(|w| PathBuf::from(w.path))
            .ok_or_else(|| GitError::NotARepo(dir.display().to_string()))
    }

    /// New worktree on new branch `branch` (from `start_point`, default HEAD) at
    /// `<repo root>/.worktrees/<branch with '/' replaced by '-'>` (Hydra's location rule).
    /// `.worktrees/` is added to the repository's `info/exclude` so it does not show as untracked.
    pub fn worktree_create(
        &self,
        dir: &Path,
        branch: &str,
        start_point: Option<&str>,
    ) -> GitResult<GitWorktree> {
        if branch.trim().is_empty() || branch.starts_with('-') || branch.contains('\0') {
            return Err(GitError::InvalidInput(format!(
                "invalid branch name: {branch:?}"
            )));
        }
        let root = self.repo_root(dir)?;
        let path = root
            .join(".worktrees")
            .join(branch.replace(['/', '\\'], "-"));
        self.with_repo_lock(&root, || {
            let mut a = args!["worktree", "add", "-b", branch, path.as_os_str()];
            if let Some(s) = start_point {
                a.push(s.into());
            }
            self.git(&root, &a)?;
            if let Err(e) = self.exclude_worktrees_dir(&root) {
                tracing::debug!(error = %e, "could not add .worktrees/ to info/exclude");
            }
            Ok(())
        })?;
        let head = self
            .git(&path, &args!["rev-parse", "--short=8", "HEAD"])
            .ok()
            .map(|h| h.trim().to_string());
        Ok(GitWorktree {
            path: path.to_string_lossy().into_owned(),
            branch: Some(branch.to_string()),
            head,
            ..GitWorktree::default()
        })
    }

    fn exclude_worktrees_dir(&self, root: &Path) -> GitResult<()> {
        let out = self.git(root, &args!["rev-parse", "--git-path", "info/exclude"])?;
        let rel = PathBuf::from(out.trim());
        let file = if rel.is_absolute() {
            rel
        } else {
            root.join(rel)
        };
        let current = std::fs::read_to_string(&file).unwrap_or_default();
        if current.lines().any(|l| l.trim() == "/.worktrees/") {
            return Ok(());
        }
        if let Some(parent) = file.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let sep = if current.is_empty() || current.ends_with('\n') {
            ""
        } else {
            "\n"
        };
        std::fs::write(&file, format!("{current}{sep}/.worktrees/\n"))?;
        Ok(())
    }

    /// Remove a worktree and optionally its branch. Without `force`, git refuses to remove a
    /// worktree with modified or untracked files (the error says so; nothing is deleted), and the
    /// branch is deleted with `-d` (kept when it has unmerged commits). With `force` both are
    /// removed regardless (Hydra's behavior).
    pub fn worktree_remove(
        &self,
        dir: &Path,
        worktree_path: &Path,
        delete_branch: Option<&str>,
        force: bool,
    ) -> GitResult<()> {
        let root = self.repo_root(dir)?;
        self.with_repo_lock(&root, || {
            if worktree_path.exists() {
                let mut a = args!["worktree", "remove"];
                if force {
                    a.push("--force".into());
                }
                a.push(worktree_path.as_os_str().into());
                self.git(&root, &a)?;
            } else {
                self.git(&root, &args!["worktree", "prune"])?;
            }
            if let Some(b) = delete_branch {
                let flag = if force { "-D" } else { "-d" };
                if let Err(e) = self.git(&root, &args!["branch", flag, b]) {
                    tracing::debug!(branch = b, error = %e, "worktree branch not deleted");
                }
            }
            Ok(())
        })
    }
}
