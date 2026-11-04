//! Agents: providers, spawning, PTY management.

pub mod env;
pub mod fake_pty;
pub mod manager;
pub mod model_detect;
pub mod output;
pub mod providers;
pub mod pty;
pub mod shell;

pub use manager::{
    AgentActivity, AgentEvent, AgentManager, ManagerError, ManagerOptions, SessionLocator,
    SessionQuery,
};
