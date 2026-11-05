//! Agent Studio core: domain logic with no UI and no transport.
//!
//! The daemon (`as-daemon`) owns an instance of these services; the TUI
//! (`as-tui`) only talks to the daemon through `as-proto`.

pub mod agents;
pub mod config;
pub mod headless;
pub mod keybindings;
pub mod overview;
pub mod paths;
pub mod sessions;
pub mod types;
pub mod usage;
pub mod usage_settings;
pub mod util;
pub mod workspace;

pub use agents::{AgentActivity, AgentEvent, AgentManager, ManagerError, ManagerOptions};
pub use config::ConfigStore;
pub use types::*;
pub use usage_settings::{UsageChipProvider, UsageSettings};
pub use workspace::{PersistedWorkspaceAgent, WorkspaceStore};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
