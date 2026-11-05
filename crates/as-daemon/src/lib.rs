//! Background daemon: owns every agent PTY and serves clients over a local socket.
//!
//! * [`serve`] runs the whole daemon on the current runtime (tests use it with a fake PTY backend).
//! * [`run`] is the process entry point: builds a runtime, handles SIGINT/SIGTERM/Ctrl+C and
//!   ignores SIGHUP (the daemon outlives the terminal that started it).
//!
//! Modules: `registry` (connected clients, event fan-out), `handler` (request -> response),
//! `connection` (one task per client), `services` (shared state, persistence, shutdown).

mod connection;
mod handler;
pub mod mcp;
mod notifications;
mod registry;
mod services;

use std::sync::Arc;

use anyhow::{Context, Result, bail};
use as_core::agents::ManagerOptions;
use as_core::paths::Paths;
use as_core::sessions::{ClaudeSessionCatalog, CodexSessionCatalog};
use as_proto::lock::{self, LockData, Probe};
use as_proto::transport;
use tokio_util::sync::CancellationToken;

pub use services::Daemon;

/// The provider session catalogs the daemon lists, imports and resolves profiles from.
#[derive(Clone)]
pub struct SessionCatalogs {
    pub claude: Arc<ClaudeSessionCatalog>,
    pub codex: Arc<CodexSessionCatalog>,
}

impl SessionCatalogs {
    /// The real ones: every Claude profile of this machine and `~/.codex/sessions`.
    pub fn discover() -> Self {
        Self {
            claude: Arc::new(ClaudeSessionCatalog::discover()),
            codex: Arc::new(CodexSessionCatalog::discover()),
        }
    }
}

/// What the usage service reads and calls out to. `None` = the real thing.
#[derive(Default)]
pub struct UsageOptions {
    /// Log folders to scan (every Claude profile and `~/.codex` by default).
    pub sources: Option<Vec<as_core::usage::store::UsageSource>>,
    /// Claude profiles whose plan limits are polled (every `~/.claude*` by default).
    pub profiles: Option<Vec<as_core::usage::limits_client::ClientProfile>>,
    /// The HTTP client of the limits poll (real `reqwest` by default; tests never hit the network).
    pub http: Option<Arc<dyn as_core::usage::limits_client::HttpClient>>,
    /// Where OAuth sign-ins come from (`.credentials.json` / Keychain by default).
    pub credentials: Option<Arc<dyn as_core::usage::limits_client::CredentialReader>>,
}

/// Knobs that tests override; `Default` is the production setup.
pub struct DaemonOptions {
    pub manager: ManagerOptions,
    pub catalogs: SessionCatalogs,
    /// Discover session ids from the catalogs after a spawn (replaces `manager.locator`).
    pub catalog_locator: bool,
    /// Import catalog sessions on startup (when the config asks for it).
    pub import_sessions: bool,
    /// Check the provider CLI before creating an agent (off with a fake backend).
    pub preflight_on_create: bool,
    /// Hand agents the login-shell PATH (`shell::fixed_path`).
    pub apply_fixed_path: bool,
    /// Show OS notifications when no client is focused (off in tests).
    pub os_notifications: bool,
    /// Run the Overview tracker (polls agents + Claude's registry, writes the history).
    pub overview: bool,
    /// Claude config dirs the tracker reads `sessions/<pid>.json` from; `None` discovers them
    /// (every `~/.claude*` profile and `$CLAUDE_CONFIG_DIR`).
    pub overview_config_dirs: Option<Vec<std::path::PathBuf>>,
    /// Run the usage service (log scanner, plan limits, alerts).
    pub usage: bool,
    /// Start the MCP server (binds a loopback port, writes the manager workspace).
    pub mcp: bool,
    /// Replaces the provider CLI of headless runs (tests).
    pub headless_override: Option<as_core::headless::HeadlessCommandOverride>,
    pub usage_options: UsageOptions,
}

impl Default for DaemonOptions {
    fn default() -> Self {
        Self {
            manager: ManagerOptions::default(),
            catalogs: SessionCatalogs::discover(),
            catalog_locator: true,
            import_sessions: true,
            preflight_on_create: true,
            apply_fixed_path: true,
            os_notifications: true,
            overview: true,
            overview_config_dirs: None,
            usage: true,
            mcp: true,
            headless_override: None,
            usage_options: UsageOptions::default(),
        }
    }
}

/// Process entry point. `foreground` only changes where logs go (decided by the caller).
pub fn run(paths: Paths, foreground: bool) -> Result<()> {
    if !foreground {
        detach_from_terminal();
    }
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("as-daemon")
        .build()
        .context("building the tokio runtime")?;
    runtime.block_on(async move {
        let stop = CancellationToken::new();
        spawn_signal_handlers(stop.clone());
        tracing::info!(foreground, root = %paths.root().display(), "daemon starting");
        let result = serve(paths, DaemonOptions::default(), stop).await;
        if let Err(err) = &result {
            tracing::error!(%err, "daemon failed");
        }
        result
    })
}

/// New session, no controlling terminal: an interactive login shell (`bash -ilc`, used to find
/// the PATH) otherwise stops on SIGTTIN/SIGTTOU in the daemon's background process group and
/// every start would wait for its timeout.
fn detach_from_terminal() {
    #[cfg(unix)]
    if let Err(err) = nix::unistd::setsid() {
        // EPERM when already a group leader (started by hand); harmless.
        tracing::debug!(%err, "setsid failed");
    }
}

fn spawn_signal_handlers(stop: CancellationToken) {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        if let Ok(mut hup) = signal(SignalKind::hangup()) {
            tokio::spawn(async move { while hup.recv().await.is_some() {} });
        }
        if let Ok(mut term) = signal(SignalKind::terminate()) {
            let stop = stop.clone();
            tokio::spawn(async move {
                term.recv().await;
                tracing::info!("SIGTERM received");
                stop.cancel();
            });
        }
    }
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            tracing::info!("Ctrl+C received");
            stop.cancel();
        }
    });
}

/// Run the daemon until a `Shutdown` request arrives or `stop` is cancelled (which stops the agents).
pub async fn serve(paths: Paths, options: DaemonOptions, stop: CancellationToken) -> Result<()> {
    paths.ensure()?;
    match lock::probe(&paths).await {
        Probe::Live(lock) => bail!(
            "another daemon is already running{}",
            lock.map(|l| format!(" (pid {})", l.pid))
                .unwrap_or_default()
        ),
        Probe::Stale => {
            tracing::info!("removing stale daemon lock/socket");
            lock::clear_stale(&paths);
        }
        Probe::Absent => {}
    }
    let listener = transport::bind(&paths).context("binding the daemon socket")?;
    let lock_data = LockData::for_current_process(&paths);
    lock::write(&paths, &lock_data)?;
    tracing::info!(pid = lock_data.pid, socket = %lock_data.socket, "daemon listening");

    let daemon = Daemon::start(paths.clone(), options).await;
    let result = accept_loop(&daemon, &listener, &stop).await;

    // Leave nothing behind, whatever the reason we stop.
    daemon.shutdown_services(true).await;
    // Let the last responses/events reach their clients.
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    if lock::read(&paths).is_some_and(|l| l.pid == lock_data.pid) {
        lock::remove(&paths);
    }
    drop(listener);
    transport::remove_socket_file(&paths);
    tracing::info!("daemon stopped");
    result
}

async fn accept_loop(
    daemon: &Arc<Daemon>,
    listener: &transport::Server,
    stop: &CancellationToken,
) -> Result<()> {
    loop {
        tokio::select! {
            () = daemon.shutdown_requested() => return Ok(()),
            () = stop.cancelled() => return Ok(()),
            accepted = transport::accept(listener) => match accepted {
                Ok(conn) => {
                    tokio::spawn(connection::handle(daemon.clone(), conn));
                }
                Err(err) => {
                    tracing::warn!(%err, "accept failed");
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                }
            },
        }
    }
}
