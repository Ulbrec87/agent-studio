//! The Overview: which agents need the user, what happened while away, how long they waited.
//! Ported from Hydra `shared/overview/*` (pure logic) and `electron/overview/*` (services),
//! which is modelled on the Overview view of agent-lookout (MIT, © 2026 Idris Olubisi).
//!
//! Pure logic: [`types`], [`status`], [`diff`], [`buckets`], [`grouping`], [`waits`],
//! [`waiting_text`], [`details`]. Services: [`claude_registry`] (+ pid matching),
//! [`process_tree`], [`git_info`], [`transcript_tail`], [`history_store`] (+ [`history_files`],
//! [`wait_ledger`]) and the [`tracker`] that ties them together.

pub mod buckets;
pub mod claude_registry;
pub mod details;
pub mod diff;
pub mod git_info;
pub mod grouping;
pub mod history_files;
pub mod history_store;
pub mod process_tree;
pub mod status;
pub mod tracker;
pub mod transcript_tail;
pub mod types;
pub mod wait_ledger;
pub mod waiting_text;
pub mod waits;

pub use types::*;
