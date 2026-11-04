//! Shared daemon state: the services (agents, config, workspace), the client registry,
//! event dispatching, debounced persistence and the shutdown sequence.

use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use as_core::agents::env::{EnvOverrides, process_env};
use as_core::agents::shell::{Os, ShellConfig, fixed_path};
use as_core::agents::{ProviderModelCatalog, SessionLocator};
use as_core::overview::history_store::{HistoryStore, HistoryStoreOptions};
use as_core::overview::tracker::{OverviewTracker, PollResult, TrackerDeps};
use as_core::overview::{EventKind, OverviewStatus};
use as_core::paths::Paths;
use as_core::sessions::{
    CatalogSessionLocator, ListSessionsOptions, SessionSummary, profiles::make_resolver,
};
use as_core::{
    AgentEvent, AgentManager, ConfigStore, ProviderId, WorkspaceStore,
    get_default_model_for_provider,
};
use as_proto::Event;
use parking_lot::Mutex;
use tokio::sync::{Notify, broadcast, mpsc, oneshot};

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
    overview_kick: Notify,
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
            overview_kick: Notify::new(),
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
        if options.overview {
            tokio::spawn(overview_loop(daemon.clone()));
        }
        if options.import_sessions {
            daemon.spawn_session_import();
        }
        daemon
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
        self.history.lock().stop();
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
