//! Agent Studio core: domain logic with no UI and no transport.
//!
//! The daemon (`as-daemon`) owns an instance of these services; the TUI
//! (`as-tui`) only talks to the daemon through `as-proto`.

pub mod paths;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
