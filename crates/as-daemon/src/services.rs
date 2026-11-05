//! Shared daemon state: the services (agents, config, workspace), the client registry,
//! event dispatching, debounced persistence and the shutdown sequence.

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use as_core::AgentStatus;
use as_core::agents::env::{EnvOverrides, process_env};
use as_core::agents::shell::{Os, ShellConfig, fixed_path};
use as_core::agents::{ProviderModelCatalog, SessionLocator};
use as_core::headless::{HeadlessEvent, HeadlessOrchestrator, HeadlessRunStatus};
use as_core::overview::history_store::{HistoryStore, HistoryStoreOptions};
use as_core::overview::tracker::{OverviewTracker, PollResult, TrackerDeps};
use as_core::overview::{EventKind, OverviewStatus};
use as_core::paths::Paths;
use as_core::sessions::{
    CatalogSessionLocator, ListSessionsOptions, SessionSummary, profiles::make_resolver,
};
use as_core::terminals::{TerminalEvent, TerminalManager, TerminalSettings};
use as_core::terminals_store::TerminalStore;
use as_core::usage::alerts::{
    AlertConfig, Budgets, TodayUsage, UsageAlert, UsageAlerts, UsageAlertsOptions,
};
use as_core::usage::limits_client::{
    ClaudeLimitsClient, ClaudeLimitsClientOptions, ClientProfile, ReqwestHttp,
    SystemCredentialReader,
};
use as_core::usage::service::{UsageService, UsageServiceOptions};
use as_core::usage::store::{UsageStore, UsageStoreOptions, default_usage_sources_from_env};
use as_core::usage::{UsagePeriod, UsageProvider};
use as_core::{
    AgentEvent, AgentManager, ConfigStore, MAX_CONCURRENT_AGENTS_HARD_LIMIT, ProviderId,
    WorkspaceStore, get_default_model_for_provider,
};
use as_proto::Event;
use parking_lot::Mutex;
use tokio::sync::{Notify, broadcast, mpsc, oneshot};

use crate::mcp::McpServer;
use crate::notifications::{self, NotificationService};
use crate::registry::Registry;
use crate::{DaemonOptions, SessionCatalogs};

const SAVE_DEBOUNCE: Duration = Duration::from_millis(500);
const CONFIG_POLL: Duration = Duration::from_millis(500);
const STOP_AGENTS_TIMEOUT: Duration = Duration::from_secs(5);
/// How often the Overview tracker polls (Hydra: 2 s).
const OVERVIEW_POLL: Duration = Duration::from_secs(2);
/// Delay before re-polling after an agent status event (Hydra: 100 ms debounce).
const OVERVIEW_STATUS_DEBOUNCE: Duration = Duration::from_millis(100);
/// How often buffered history lines are appended to disk.
const HISTORY_FLUSH: Duration = Duration::from_secs(5);
/// What `GetOverviewHistory` returns without `since_ms`.
const HISTORY_DEFAULT_WINDOW_MS: i64 = 6 * 60 * 60 * 1000;
/// How often the usage service looks at what is due (scan every 60 s, limits every 5 min / 60 s).
const USAGE_TICK: Duration = Duration::from_secs(5);

/// Messages for the dispatcher task, which owns the ordering of output events.
pub enum Control {
    Subscribe {
        client: u64,
        agent_id: String,
        reply: oneshot::Sender<Vec<u8>>,
    },
}

pub struct Daemon {
    pub paths: Paths,
    pub manager: AgentManager,
    pub config: Arc<ConfigStore>,
    pub workspace: Arc<WorkspaceStore>,
    pub catalogs: SessionCatalogs,
    pub models: ProviderModelCatalog,
    pub registry: Registry,
    pub notifications: NotificationService,
    /// Overview tracker + its history; one writer (this daemon is single-instance).
    pub overview: Arc<Mutex<OverviewTracker>>,
    pub history: Arc<Mutex<HistoryStore>>,
    /// Headless runs (P8).
    pub headless: HeadlessOrchestrator,
    /// Free terminals (P9).
    pub terminals: Arc<TerminalManager>,
    /// `terminals.json`: the free-terminal layouts (saved debounced, restored lazily).
    terminal_store: TerminalStore,
    terminals_save: Notify,
    /// Set at shutdown, once the layouts are on disk: no later save may overwrite them.
    terminals_frozen: AtomicBool,
    /// Where the skills scan/toggle looks (real `~/.claude*` / `~/.codex` unless a test overrides it).
    pub skills: as_core::skills::SkillScanOptions,
    /// Hydra's data dir when not the default (tests).
    pub hydra_dir: Option<std::path::PathBuf>,
    /// The MCP server (P8) once started, or why it did not.
    pub mcp: Mutex<Option<McpServer>>,
    pub mcp_error: Mutex<Option<String>>,
    overview_kick: Notify,
    /// Usage: log scanner, Claude plan limits, scheduling (see `as_core::usage::service`).
    pub usage: Arc<UsageService>,
    usage_alerts: OnceLock<Arc<UsageAlerts>>,
    usage_kick: Notify,
    usage_idle: AtomicBool,
    /// Update check (P10): GitHub Releases, never replaces the binary.
    pub updates: Arc<as_core::updates::UpdateService>,
    /// The version the clients were last told about (one `UpdateAvailable` per version).
    updates_announced: Mutex<Option<String>>,
    pub started: Instant,
    pub preflight_on_create: bool,
    os_notifications: bool,
    save: Notify,
    shutdown: Notify,
    shutdown_flag: AtomicBool,
    services_stopped: AtomicBool,
    control: mpsc::UnboundedSender<Control>,
}

impl Daemon {
    /// Open the stores, build the manager, restore the workspace and start the background tasks.
    pub async fn start(paths: Paths, options: DaemonOptions) -> Arc<Self> {
        let config = Arc::new(ConfigStore::open(&paths));
        let workspace = Arc::new(WorkspaceStore::open(&paths));
        let mut manager_options = options.manager;
        let terminals = {
            let config = config.clone();
            TerminalManager::new(manager_options.backend.clone(), move || {
                TerminalSettings::from_config(&config.get())
            })
        };
        // The saved layouts only become shells when a project's terminal panel is first used.
        let terminal_store = TerminalStore::open(&paths);
        terminals.restore(terminal_store.load());
        if options.catalog_locator {
            let locator: Arc<dyn SessionLocator> = Arc::new(CatalogSessionLocator::new(
                options.catalogs.claude.clone(),
                options.catalogs.codex.clone(),
            ));
            manager_options.locator = locator;
        }
        let manager = AgentManager::with_options(manager_options);
        {
            let config = config.clone();
            manager.set_shell_config_provider(move || ShellConfig::from_config(&config.get()));
        }
        {
            // Claude profiles: a resumed session uses the profile that owns it, the rest follow
            // `claudeProfileByProject`.
            let config = config.clone();
            manager.set_claude_profile_resolver(make_resolver(move || {
                config.get().claude_profile_by_project
            }));
        }
        let headless =
            HeadlessOrchestrator::with_override(paths.headless_dir(), options.headless_override);
        {
            let config = config.clone();
            headless.set_shell_config_provider(move || ShellConfig::from_config(&config.get()));
        }
        {
            let config = config.clone();
            headless.set_claude_profile_resolver(make_resolver(move || {
                config.get().claude_profile_by_project
            }));
        }
        let models = ProviderModelCatalog::new();
        if options.apply_fixed_path {
            // Runs the login shell (bounded by its own timeout): keep it off the async threads.
            let fixed = tokio::task::spawn_blocking(|| fixed_path(&process_env(), Os::current()))
                .await
                .ok()
                .flatten();
            if let Some(path) = fixed {
                let mut overrides = EnvOverrides::new();
                overrides.insert("PATH".into(), Some(path));
                models.set_env_overrides(overrides.clone());
                headless.set_env_overrides(overrides.clone());
                terminals.set_env_overrides(overrides.clone());
                manager.set_env_overrides(overrides);
            }
        }
        let restored = manager.hydrate_workspace_agents(&workspace.get_agents());
        tracing::info!(restored, "workspace restored");

        let history = Arc::new(Mutex::new(HistoryStore::new(HistoryStoreOptions::new(
            paths.overview_history_dir(),
        ))));
        history.lock().start();
        let mut deps =
            TrackerDeps::production(Box::new(manager.clone()), Box::new(history.clone()));
        if let Some(dirs) = options.overview_config_dirs.clone() {
            deps.config_dirs = Box::new(move || dirs.clone());
        }
        let overview = Arc::new(Mutex::new(OverviewTracker::new(deps)));

        let usage = Arc::new(build_usage_service(&paths, options.usage_options));
        let updates = Arc::new(as_core::updates::UpdateService::new(
            as_core::updates::UpdateOptions::production(
                options
                    .update_http
                    .unwrap_or_else(|| Arc::new(ReqwestHttp::new())),
            ),
        ));

        let (control, control_rx) = mpsc::unbounded_channel();
        let daemon = Arc::new(Self {
            paths,
            manager,
            config,
            workspace,
            catalogs: options.catalogs,
            models,
            registry: Registry::default(),
            notifications: NotificationService::default(),
            overview,
            history,
            headless,
            terminals,
            terminal_store,
            terminals_save: Notify::new(),
            terminals_frozen: AtomicBool::new(false),
            skills: options
                .skills
                .unwrap_or_else(as_core::skills::SkillScanOptions::from_env),
            hydra_dir: options.hydra_dir,
            mcp: Mutex::new(None),
            mcp_error: Mutex::new(None),
            overview_kick: Notify::new(),
            usage,
            usage_alerts: OnceLock::new(),
            usage_kick: Notify::new(),
            usage_idle: AtomicBool::new(false),
            updates,
            updates_announced: Mutex::new(None),
            started: Instant::now(),
            preflight_on_create: options.preflight_on_create,
            os_notifications: options.os_notifications,
            save: Notify::new(),
            shutdown: Notify::new(),
            shutdown_flag: AtomicBool::new(false),
            services_stopped: AtomicBool::new(false),
            control,
        });
        daemon.spawn_tasks(control_rx);
        daemon.install_usage_hooks();
        daemon.spawn_headless_events();
        daemon.spawn_terminal_events();
        if options.mcp {
            daemon.start_mcp().await;
        }
        if options.overview {
            tokio::spawn(overview_loop(daemon.clone()));
        }
        if options.usage {
            tokio::spawn(usage_loop(daemon.clone()));
        }
        if options.updates {
            tokio::spawn(update_loop(daemon.clone()));
        }
        if options.import_sessions {
            daemon.spawn_session_import();
        }
        daemon
    }

    /// Start the MCP server and write the manager workspace; a failure is kept for `GetMcpStatus`.
    async fn start_mcp(self: &Arc<Self>) {
        let config = self.config.clone();
        let started = McpServer::start(
            self.manager.clone(),
            self.paths.manager_workspace_dir(),
            move || {
                config
                    .get()
                    .max_agents
                    .min(MAX_CONCURRENT_AGENTS_HARD_LIMIT)
            },
        )
        .await;
        match started {
            Ok(server) => *self.mcp.lock() = Some(server),
            Err(err) => {
                tracing::warn!(err = %format!("{err:#}"), "MCP server failed to start");
                *self.mcp_error.lock() = Some(format!("{err:#}"));
            }
        }
    }

    /// `GetMcpStatus`.
    pub fn mcp_status(&self) -> as_proto::McpStatus {
        match &*self.mcp.lock() {
            Some(server) => as_proto::McpStatus {
                running: true,
                port: Some(server.port),
                error: None,
                manager_workspace: Some(server.workspace.to_string_lossy().into_owned()),
            },
            None => as_proto::McpStatus {
                running: false,
                port: None,
                error: self.mcp_error.lock().clone(),
                manager_workspace: None,
            },
        }
    }

    /// Headless output and state changes -> clients (+ notifications when a run ends).
    fn spawn_headless_events(self: &Arc<Self>) {
        let this = self.clone();
        let mut rx = self.headless.subscribe();
        tokio::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(HeadlessEvent::Output { run_id, data }) => this
                        .registry
                        .broadcast(&Event::HeadlessRunEvent { run_id, data }),
                    Ok(HeadlessEvent::Changed(run)) => {
                        match run.status {
                            HeadlessRunStatus::Completed => {
                                this.notify(this.notifications.headless_completed(&run.id))
                            }
                            HeadlessRunStatus::Errored => this.notify(
                                this.notifications
                                    .headless_errored(&run.id, run.error.as_deref()),
                            ),
                            _ => {}
                        }
                        this.registry.broadcast(&Event::HeadlessRunChanged(run));
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!(skipped = n, "headless event dispatcher lagged");
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        });
    }

    /// Free-terminal output, exits and layout changes -> clients; plus the idle sweep.
    fn spawn_terminal_events(self: &Arc<Self>) {
        let this = self.clone();
        let mut rx = self.terminals.subscribe();
        tokio::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(TerminalEvent::Output {
                        terminal_id, data, ..
                    }) => {
                        this.registry.terminal_output(&terminal_id, &data);
                    }
                    Ok(TerminalEvent::Exit {
                        terminal_id,
                        project_dir,
                        exit_code,
                    }) => {
                        this.registry.unsubscribe_all_terminal(&terminal_id);
                        this.registry.broadcast(&Event::TerminalExit {
                            terminal_id,
                            project_dir,
                            exit_code,
                        });
                    }
                    Ok(TerminalEvent::LayoutChanged {
                        project_dir,
                        layout,
                    }) => {
                        this.terminals_save.notify_one();
                        this.registry.broadcast(&Event::TerminalLayoutChanged {
                            project_dir,
                            layout,
                        });
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!(skipped = n, "terminal event dispatcher lagged");
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        });
        // Debounced persistence of the layouts (`terminals.json`).
        let this = self.clone();
        tokio::spawn(async move {
            loop {
                this.terminals_save.notified().await;
                tokio::time::sleep(SAVE_DEBOUNCE).await;
                let saver = this.clone();
                let _ = tokio::task::spawn_blocking(move || saver.persist_terminals()).await;
            }
        });
        let this = self.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(60));
            loop {
                tick.tick().await;
                let t = this.terminals.clone();
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_millis() as u64);
                let _ = tokio::task::spawn_blocking(move || t.sweep_idle(now)).await;
            }
        });
    }

    /// Sessions of a provider's catalog (blocking: scans files; call from `spawn_blocking`).
    /// OpenCode has no catalog.
    pub fn list_sessions_blocking(&self, options: &ListSessionsOptions) -> Vec<SessionSummary> {
        let hidden: HashSet<String> = self.config.get().hidden_session_ids.into_iter().collect();
        match options.provider {
            ProviderId::Claude => self.catalogs.claude.list_sessions(options, &hidden),
            ProviderId::Codex => self.catalogs.codex.list_sessions(options, &hidden),
            ProviderId::Opencode => Vec::new(),
        }
    }

    /// Import catalog sessions as idle agents (Hydra `importSessionsOnStartup`), in the background:
    /// scanning must not delay the first client. Every catalog gives its own provider, so a session
    /// is always resumed with the CLI that created it.
    fn spawn_session_import(self: &Arc<Self>) {
        let config = self.config.get();
        if !config.import_sessions_on_startup {
            return;
        }
        let this = self.clone();
        tokio::spawn(async move {
            for provider in [ProviderId::Claude, ProviderId::Codex] {
                let options = ListSessionsOptions {
                    provider,
                    limit: (config.session_import_limit > 0)
                        .then_some(config.session_import_limit as usize),
                    max_age_days: (config.session_max_age_days > 0)
                        .then_some(config.session_max_age_days),
                    project_path_prefix: Some(config.session_import_project_prefix.clone())
                        .filter(|p| !p.trim().is_empty()),
                    ..Default::default()
                };
                let scan = this.clone();
                let sessions =
                    tokio::task::spawn_blocking(move || scan.list_sessions_blocking(&options))
                        .await
                        .unwrap_or_default();
                // Nothing to resume in a session without a prompt and without messages.
                let sessions: Vec<_> = sessions
                    .into_iter()
                    .filter(|s| !(s.first_prompt.trim().is_empty() && s.message_count == 0))
                    .collect();
                let model = get_default_model_for_provider(provider);
                let imported = this.manager.import_sessions(&sessions, &model, provider);
                if imported > 0 {
                    tracing::info!(imported, %provider, "imported sessions");
                }
            }
        });
    }

    fn spawn_tasks(self: &Arc<Self>, control_rx: mpsc::UnboundedReceiver<Control>) {
        // Agent events -> clients.
        tokio::spawn(dispatch_loop(
            self.clone(),
            self.manager.subscribe(),
            control_rx,
        ));

        // Debounced workspace persistence.
        let this = self.clone();
        tokio::spawn(async move {
            loop {
                this.save.notified().await;
                tokio::time::sleep(SAVE_DEBOUNCE).await;
                this.persist(false);
            }
        });

        // Hot reload of config.json + fan-out of every config change.
        let config = self.config.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(CONFIG_POLL);
            loop {
                tick.tick().await;
                config.reload_if_changed();
            }
        });
        let this = self.clone();
        let mut rx = self.config.subscribe();
        tokio::spawn(async move {
            while rx.changed().await.is_ok() {
                let cfg = rx.borrow_and_update().clone();
                this.registry
                    .broadcast(&Event::ConfigChanged(Box::new(cfg)));
                // Alert settings and budgets may have changed.
                let alerts = this.clone();
                let _ = tokio::task::spawn_blocking(move || alerts.evaluate_usage_alerts()).await;
            }
        });
    }

    /// Ask the dispatcher to subscribe `client` to `agent_id`; returns the buffered output.
    pub async fn subscribe(&self, client: u64, agent_id: &str) -> Option<Vec<u8>> {
        let (reply, rx) = oneshot::channel();
        self.control
            .send(Control::Subscribe {
                client,
                agent_id: agent_id.to_string(),
                reply,
            })
            .ok()?;
        rx.await.ok()
    }

    /// Persist the agent list soon (debounced).
    pub fn request_save(&self) {
        self.save.notify_one();
    }

    pub fn persist(&self, ending_now: bool) {
        let agents = self.manager.export_workspace_agents(ending_now);
        if let Err(err) = self.workspace.set_agents(agents) {
            tracing::error!(%err, "saving workspace failed");
        }
    }

    /// Write the free-terminal layouts. Not once they were frozen at shutdown: closing the shells at
    /// shutdown must not erase the layout that the next start restores.
    pub fn persist_terminals(&self) {
        if self.terminals_frozen.load(Ordering::SeqCst) {
            return;
        }
        if let Err(err) = self.terminal_store.save(&self.terminals.export()) {
            tracing::error!(%err, "saving the terminal layouts failed");
        }
    }

    pub fn is_shutting_down(&self) -> bool {
        self.shutdown_flag.load(Ordering::SeqCst)
    }

    /// Resolves once a client (or the daemon itself) asked to exit.
    pub async fn shutdown_requested(&self) {
        loop {
            let notified = self.shutdown.notified();
            if self.is_shutting_down() {
                return;
            }
            notified.await;
        }
    }

    /// Make the accept loop exit (the services were already stopped by the caller).
    pub fn request_exit(&self) {
        self.shutdown_flag.store(true, Ordering::SeqCst);
        self.shutdown.notify_waiters();
        self.shutdown.notify_one();
    }

    /// Persist, optionally stop every agent, tell the clients. Idempotent.
    pub async fn shutdown_services(&self, stop_agents: bool) {
        if self.services_stopped.swap(true, Ordering::SeqCst) {
            return;
        }
        tracing::info!(stop_agents, "stopping services");
        self.persist(true);
        // The layouts are written before the shells go away (and not again afterwards).
        self.persist_terminals();
        self.terminals_frozen.store(true, Ordering::SeqCst);
        self.terminals.kill_all();
        self.history.lock().stop();
        self.usage.stop(); // flushes the usage cache
        if let Some(mcp) = &*self.mcp.lock() {
            mcp.stop();
        }
        self.headless.cancel_all(Duration::from_secs(5)).await;
        if stop_agents {
            self.manager.kill_all();
            if !self.manager.wait_all_stopped(STOP_AGENTS_TIMEOUT).await {
                tracing::warn!("some agents did not stop in time");
            }
            self.persist(false);
        }
        self.registry.broadcast(&Event::DaemonShuttingDown);
    }

    fn agent_name(&self, agent_id: &str) -> String {
        self.manager
            .get(agent_id)
            .map_or_else(|| agent_id.to_string(), |a| a.name.clone())
    }

    /// Record, push to every client and show an OS notification when no client is focused.
    fn notify(&self, notification: as_proto::StudioNotification) {
        self.notifications.record(&notification);
        if self.os_notifications && !self.registry.any_focused() {
            notifications::show_os_notification(&notification);
        }
        self.registry.broadcast(&Event::Notification(notification));
    }

    /// Push the result of an Overview poll to the clients, and raise "needs you" notifications
    /// for Claude agents that just started waiting.
    fn publish_overview(&self, poll: PollResult) {
        if !poll.events.is_empty() {
            // A Claude agent that goes working -> idle has just written a transcript and moved the
            // plan limits (Hydra `notifyAgentIdle`).
            let claude_went_idle = poll.events.iter().any(|e| {
                e.kind == EventKind::Status
                    && e.from == Some(OverviewStatus::Working)
                    && e.to == Some(OverviewStatus::Idle)
                    && poll
                        .snapshot
                        .sessions
                        .iter()
                        .any(|s| s.agent_id == e.agent_id && s.provider == ProviderId::Claude)
            });
            if claude_went_idle {
                self.usage_idle.store(true, Ordering::SeqCst);
                self.usage_kick.notify_one();
            }
            for event in &poll.events {
                if event.kind != EventKind::Status || event.to != Some(OverviewStatus::NeedsYou) {
                    continue;
                }
                let session = poll
                    .snapshot
                    .sessions
                    .iter()
                    .find(|s| s.agent_id == event.agent_id);
                self.notify(
                    self.notifications.needs_you(
                        &event.agent_id,
                        session.map_or(event.name.as_str(), |s| s.name.as_str()),
                        session
                            .and_then(|s| s.waiting_reason)
                            .or(event.waiting_reason),
                        session.and_then(|s| s.waiting_text.as_deref()),
                    ),
                );
            }
            self.registry.broadcast(&Event::OverviewEvents(poll.events));
        }
        if poll.changed {
            self.registry
                .broadcast(&Event::OverviewSnapshot(Box::new(poll.snapshot)));
        }
    }

    /// A client started or stopped showing a usage surface.
    pub fn sync_usage_visible(&self) {
        let visible = self.registry.any_usage_visible();
        let was = self.usage.is_visible();
        self.usage.set_visible(visible);
        if visible && !was {
            // Poll the limits now instead of at the next 5 min mark.
            self.usage_kick.notify_one();
        }
    }

    fn running_agents(&self, provider: UsageProvider) -> usize {
        let provider = match provider {
            UsageProvider::Claude => ProviderId::Claude,
            UsageProvider::Codex => ProviderId::Codex,
        };
        self.manager
            .list()
            .iter()
            .filter(|a| {
                a.provider == provider
                    && matches!(a.status, AgentStatus::Running | AgentStatus::Starting)
            })
            .count()
    }

    /// Push usage changes to the clients and evaluate the alerts on every change.
    fn install_usage_hooks(self: &Arc<Self>) {
        let weak = Arc::downgrade(self);
        let alerts = {
            let (w_config, w_notify, w_run) = (weak.clone(), weak.clone(), weak.clone());
            Arc::new(UsageAlerts::new(UsageAlertsOptions {
                file: self.paths.usage_alerts_file(),
                config: Box::new(move || {
                    let config = w_config.upgrade().map(|d| d.config.get());
                    match config {
                        Some(c) => AlertConfig {
                            settings: c.usage.clone(),
                            budgets: Budgets {
                                daily_tokens: c.usage_daily_token_budget,
                                daily_cost_usd: c.usage_daily_cost_budget_usd,
                                warning_pct: c.usage_budget_warning_threshold_pct,
                            },
                        },
                        None => AlertConfig {
                            settings: as_core::UsageSettings::default(),
                            budgets: Budgets::default(),
                        },
                    }
                }),
                notify: Box::new(move |alert: UsageAlert| {
                    if let Some(d) = w_notify.upgrade() {
                        d.notify(usage_notification(alert));
                    }
                }),
                running_agents: Some(Box::new(move |p| {
                    w_run.upgrade().map_or(0, |d| d.running_agents(p))
                })),
                now: Box::new(|| chrono::Utc::now().timestamp_millis()),
            }))
        };
        let _ = self.usage_alerts.set(alerts);
        self.usage.on_changed(Arc::new(move |state| {
            if let Some(d) = weak.upgrade() {
                d.registry
                    .broadcast(&Event::UsageChanged(Box::new(state.clone())));
                d.evaluate_usage_alerts();
            }
        }));
    }

    /// Low-limit and budget alerts for the current usage state (blocking: reads/writes a file).
    pub fn evaluate_usage_alerts(&self) {
        let Some(alerts) = self.usage_alerts.get() else {
            return;
        };
        let today = TodayUsage::from_summary(
            &self.usage.summary(UsagePeriod::Today, None),
            &chrono::Local::now(),
        );
        alerts.evaluate(&self.usage.state(), Some(&today));
    }

    /// Poll the tracker off the async threads and publish the result.
    async fn poll_overview(self: &Arc<Self>) {
        let this = self.clone();
        match tokio::task::spawn_blocking(move || this.overview.lock().refresh()).await {
            Ok(poll) => self.publish_overview(poll),
            Err(err) => tracing::warn!(%err, "overview poll failed"),
        }
    }

    /// `GetOverviewHistory`.
    pub fn history_since(&self, since_ms: Option<i64>) -> as_core::overview::HistoryResponse {
        let since = since_ms
            .unwrap_or_else(|| chrono::Utc::now().timestamp_millis() - HISTORY_DEFAULT_WINDOW_MS);
        self.history.lock().query(since)
    }

    fn dispatch(&self, event: AgentEvent) {
        match event {
            AgentEvent::Output { agent_id, data } => self.registry.output(&agent_id, &data),
            AgentEvent::Status {
                agent_id,
                status,
                session_id,
                model,
            } => {
                let name = self.agent_name(&agent_id);
                if let Some(n) = self.notifications.status(&agent_id, &name, status) {
                    self.notify(n);
                }
                self.overview_kick.notify_one();
                self.registry.broadcast(&Event::AgentStatus {
                    agent_id,
                    status,
                    session_id,
                    model,
                });
                self.request_save();
            }
            AgentEvent::Waiting { agent_id } => {
                let name = self.agent_name(&agent_id);
                // The 5 s silence rule fires falsely for a busy-but-quiet Claude agent: when the
                // tracker reads that agent's registry entry, its own `needs-you` notification
                // (see `publish_overview`) replaces it.
                let tracked = self.manager.get(&agent_id).is_some_and(|a| {
                    a.provider == ProviderId::Claude
                        && self.overview.lock().has_registry_match(&agent_id)
                });
                if !tracked {
                    self.notify(self.notifications.waiting(&agent_id, &name));
                }
                self.registry.broadcast(&Event::AgentWaiting { agent_id });
            }
            AgentEvent::Changed => {
                self.registry.broadcast(&Event::AgentsChanged);
                self.request_save();
            }
        }
    }
}

/// Polls the Overview tracker every 2 s and shortly after agent status changes, and flushes the
/// history to disk every 5 s, until the services stop.
async fn overview_loop(daemon: Arc<Daemon>) {
    let mut poll = tokio::time::interval(OVERVIEW_POLL);
    let mut flush = tokio::time::interval(HISTORY_FLUSH);
    flush.tick().await;
    loop {
        tokio::select! {
            _ = poll.tick() => {}
            () = daemon.overview_kick.notified() => {
                tokio::time::sleep(OVERVIEW_STATUS_DEBOUNCE).await;
            }
            _ = flush.tick() => {
                let d = daemon.clone();
                let _ = tokio::task::spawn_blocking(move || d.history.lock().flush()).await;
                continue;
            }
        }
        if daemon.services_stopped.load(Ordering::SeqCst) {
            break;
        }
        daemon.poll_overview().await;
    }
}

/// Runs what the usage service has due every few seconds, off the async threads: the first tick
/// scans the logs and polls the limits (a big first index never delays a client), a client that
/// shows a usage surface or an agent that just finished a turn wakes it early.
async fn usage_loop(daemon: Arc<Daemon>) {
    let d = daemon.clone();
    let _ = tokio::task::spawn_blocking(move || d.usage.start()).await;
    let mut tick = tokio::time::interval(USAGE_TICK);
    loop {
        tokio::select! {
            _ = tick.tick() => {}
            () = daemon.usage_kick.notified() => {}
        }
        if daemon.services_stopped.load(Ordering::SeqCst) {
            break;
        }
        let d = daemon.clone();
        let idle = daemon.usage_idle.swap(false, Ordering::SeqCst);
        let _ = tokio::task::spawn_blocking(move || {
            if idle {
                d.usage.notify_agent_idle();
            }
            d.usage.tick();
        })
        .await;
    }
}

/// Delay before the first update check (the daemon has more urgent things to do at start).
const UPDATE_STARTUP_DELAY: Duration = Duration::from_secs(10);

/// Update check at startup and every 6 h while `checkForUpdates` is on. Offline, rate limits and
/// 404 only change the state; nothing is shown for them.
async fn update_loop(daemon: Arc<Daemon>) {
    tokio::time::sleep(UPDATE_STARTUP_DELAY).await;
    loop {
        if daemon.services_stopped.load(Ordering::SeqCst) {
            break;
        }
        if daemon.config.get().check_for_updates {
            daemon.check_updates().await;
        }
        tokio::time::sleep(as_core::updates::CHECK_INTERVAL).await;
    }
}

impl Daemon {
    /// The update state with the config's `checkForUpdates` filled in (no network).
    pub fn update_state(&self) -> as_core::updates::UpdateState {
        self.updates
            .set_enabled(self.config.get().check_for_updates);
        self.updates.state()
    }

    /// Ask GitHub Releases now. A newer version that the clients were not told about yet is
    /// broadcast as `UpdateAvailable`.
    pub async fn check_updates(self: &Arc<Self>) -> as_core::updates::UpdateState {
        let service = self.updates.clone();
        let checked = tokio::task::spawn_blocking(move || service.check()).await;
        if checked.is_err() {
            tracing::warn!("update check task failed");
        }
        let state = self.update_state();
        if let Some(info) = state.available() {
            let mut announced = self.updates_announced.lock();
            if announced.as_deref() != Some(info.version.as_str()) {
                *announced = Some(info.version.clone());
                drop(announced);
                tracing::info!(version = %info.version, "update available");
                self.registry
                    .broadcast(&Event::UpdateAvailable(Box::new(state.clone())));
            }
        }
        state
    }
}

fn usage_notification(alert: UsageAlert) -> as_proto::StudioNotification {
    as_proto::StudioNotification {
        id: uuid::Uuid::new_v4().simple().to_string()[..12].to_string(),
        kind: as_proto::NotificationKind::UsageLimit,
        title: alert.title,
        body: alert.body,
        agent_id: None,
        timestamp: chrono::Utc::now().to_rfc3339(),
    }
}

/// The store (log scanner), the limits client and the service that schedules them.
fn build_usage_service(paths: &Paths, options: crate::UsageOptions) -> UsageService {
    let sources = options.sources;
    let store = UsageStore::new(UsageStoreOptions::new(
        paths.usage_cache_file(),
        move || {
            sources
                .clone()
                .unwrap_or_else(default_usage_sources_from_env)
        },
    ));
    let profiles = options.profiles;
    let limits = ClaudeLimitsClient::new(ClaudeLimitsClientOptions {
        profiles: Box::new(move || {
            profiles.clone().unwrap_or_else(|| {
                as_core::sessions::profiles::list_profiles_from_env()
                    .into_iter()
                    .map(|p| ClientProfile {
                        config_dir: p.config_dir,
                        label: p.label,
                        is_default: p.is_default,
                    })
                    .collect()
            })
        }),
        http: options.http.unwrap_or_else(|| Arc::new(ReqwestHttp::new())),
        credentials: options
            .credentials
            .unwrap_or_else(|| Arc::new(SystemCredentialReader)),
        now: Box::new(|| chrono::Utc::now().timestamp_millis()),
        version: Some(as_core::VERSION.to_string()),
    });
    UsageService::new(UsageServiceOptions {
        store: Arc::new(store),
        limits: Arc::new(limits),
        now: Box::new(|| chrono::Utc::now().timestamp_millis()),
    })
}

async fn dispatch_loop(
    daemon: Arc<Daemon>,
    mut rx: broadcast::Receiver<AgentEvent>,
    mut control: mpsc::UnboundedReceiver<Control>,
) {
    loop {
        tokio::select! {
            event = rx.recv() => match event {
                Ok(event) => daemon.dispatch(event),
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!(skipped = n, "event dispatcher lagged behind the agents");
                }
                Err(broadcast::error::RecvError::Closed) => break,
            },
            Some(Control::Subscribe { client, agent_id, reply }) = control.recv() => {
                // Deliver everything emitted so far to the others first, so that the snapshot
                // below and the live stream that follows do not overlap for the new subscriber.
                loop {
                    match rx.try_recv() {
                        Ok(event) => daemon.dispatch(event),
                        Err(broadcast::error::TryRecvError::Lagged(n)) => {
                            tracing::warn!(skipped = n, "event dispatcher lagged behind the agents");
                        }
                        Err(_) => break,
                    }
                }
                let buffer = daemon.manager.get_buffer(&agent_id);
                daemon.registry.subscribe(client, &agent_id);
                let _ = reply.send(buffer);
            }
        }
    }
}
