//! Git and GitHub CLI wrappers (Hydra `electron/git/GitService.ts`).
//!
//! Everything shells out to `git` / `gh`. All public functions are blocking (the daemon wraps
//! them in `spawn_blocking`); mutating ones are serialized per repository. Use [`GitRunner`]:
//! `GitRunner::new().status(dir)`.

// `args!` builds a Vec that is sometimes only borrowed; keeping one macro is simpler.
#![allow(clippy::useless_vec)]

mod error;
mod ops;
mod pr;
mod runner;
mod status;
mod types;
mod unified_diff;
mod worktree;

#[cfg(test)]
mod tests;

pub use error::{GitError, GitResult};
pub use ops::MAX_DIFF_BYTES;
pub use pr::{parse_pr_identifier, parse_pr_metadata, pr_files_from_diff};
pub use runner::{DEFAULT_TIMEOUT, GitRunner, NETWORK_TIMEOUT};
pub use status::parse_status_v2;
pub use types::*;
pub use unified_diff::{
    DiffFile, DiffFileStatus, DiffLine, DiffLineKind, FileDiffText, Hunk, extract_file_diff,
    parse_pr_diff, parse_unified_diff, split_diff_by_file,
};
pub use worktree::parse_worktree_list;
