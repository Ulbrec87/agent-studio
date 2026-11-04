//! The Overview view: state (Hydra's `useOverview*` hooks as plain data), text helpers and the
//! card view-models. Rendering lives in `ui::overview`.
//!
//! Modeled on the Overview view of agent-lookout (MIT, © 2026 Idris Olubisi), like Hydra's.

pub mod format;
pub mod model;
pub mod state;

pub use state::{Card, OverviewState, SessionsMode, WaitsTab};
