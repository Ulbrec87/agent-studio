//! Terminal UI: daemon client, app state, views, widgets, keymap and theme.
//!
//! * [`app`] — state machine (`App`, `Effect`, `Msg`), testable without a terminal;
//! * [`ui`] — rendering per view; [`widgets`] — small reusable widgets;
//! * [`keymap`] — data-driven key tables; [`theme`] — Hydra palettes;
//! * [`term`] — terminal emulation (`vt100` behind a trait) and xterm key encoding;
//! * [`client`] — effects -> daemon requests; [`runner`] — event loop and terminal lifecycle.

pub mod app;
pub mod bindings_file;
pub mod client;
pub mod grid;
pub mod headless;
pub mod highlight;
pub mod keymap;
pub mod new_agent;
pub mod overview;
pub mod palette;
pub mod panels;
pub mod runner;
pub mod sessions;
pub mod sidebar;
pub mod state;
pub mod suspend;
pub mod term;
pub mod theme;
pub mod ui;
pub mod usage;
pub mod widgets;

pub use runner::{TuiOptions, run, run_with};
