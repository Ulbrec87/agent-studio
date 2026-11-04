//! Agents: providers, spawning, PTY management.

pub mod env;
pub mod fake_pty;
pub mod manager;
pub mod model_catalog;
pub mod model_detect;
pub mod output;
pub mod providers;
pub mod pty;
pub mod shell;
pub mod startup;

pub use manager::{
    AgentActivity, AgentEvent, AgentManager, DiscoverySchedule, ManagerError, ManagerOptions,
    SessionLocator, SessionQuery,
};
pub use model_catalog::ProviderModelCatalog;
pub use startup::StartupOptions;
