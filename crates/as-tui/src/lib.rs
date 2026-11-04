//! Terminal UI: daemon client, app state, views, widgets, keymap and theme.
//!
//! * [`app`] — state machine (`App`, `Effect`, `Msg`), testable without a terminal;
//! * [`ui`] — rendering per view; [`widgets`] — small reusable widgets;
//! * [`keymap`] — data-driven key tables; [`theme`] — Hydra palettes;
//! * [`term`] — terminal emulation (`vt100` behind a trait) and xterm key encoding;
//! * [`client`] — effects -> daemon requests; [`runner`] — event loop and terminal lifecycle.

pub mod app;
pub mod client;
pub mod keymap;
pub mod new_agent;
pub mod runner;
pub mod sidebar;
pub mod term;
pub mod theme;
pub mod ui;
pub mod widgets;

pub use runner::{TuiOptions, run, run_with};
