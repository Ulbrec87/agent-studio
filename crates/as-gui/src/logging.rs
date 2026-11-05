//! Tracing setup for the desktop binary: same as the CLI's `src/logging.rs` (JSON lines in a daily rolling file under
//! `logs_dir()`). Duplicated on purpose (it is 30 lines); fold both into one crate when a third caller appears.

use as_core::paths::Paths;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::{EnvFilter, Layer, fmt, layer::SubscriberExt, util::SubscriberInitExt};

/// Log file prefix of the desktop app (and of the browser dev bridge): `<data>/logs/agent-studio-gui.<date>`. The renderer's
/// structured events (`log_event`) go there too, see [`crate::gui_log`].
pub const GUI_LOG_PREFIX: &str = "agent-studio-gui";

/// Env var overriding the default `info` filter.
pub const LOG_ENV: &str = "AGENT_STUDIO_LOG";

fn filter() -> EnvFilter {
    EnvFilter::try_from_env(LOG_ENV).unwrap_or_else(|_| EnvFilter::new("info"))
}

/// Initialise logging. `prefix` is the log file prefix (`agent-studio` or
/// `daemon`); `stderr` additionally logs human-readable lines to stderr
/// (only for a foreground daemon, never in the TUI). Keep the guard alive.
pub fn init(paths: &Paths, prefix: &str, stderr: bool) -> WorkerGuard {
    let appender = tracing_appender::rolling::daily(paths.logs_dir(), prefix);
    let (writer, guard) = tracing_appender::non_blocking(appender);
    let file_layer = fmt::layer()
        .json()
        .with_ansi(false)
        .with_writer(writer)
        .with_filter(filter());
    let stderr_layer = stderr.then(|| {
        fmt::layer()
            .with_writer(std::io::stderr)
            .with_filter(filter())
    });
    tracing_subscriber::registry()
        .with(file_layer)
        .with(stderr_layer)
        .init();
    guard
}
