//! Shared daemon state: the services (agents, config, workspace), the client registry,
//! event dispatching, debounced persistence and the shutdown sequence.

use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use as_core::agents::env::{EnvOverrides, process_env};
use as_core::agents::shell::{Os, ShellConfig, fixed_path};
use as_core::agents::{ProviderModelCatalog, SessionLocator};
use as_core::paths::Paths;
use as_core::sessions::{
    CatalogSessionLocator, ListSessionsOptions, SessionSummary, profiles::make_resolver,
};
use as_core::{
    AgentEvent, AgentManager, ConfigStore, ProviderId, WorkspaceStore,
    get_default_model_for_provider,
};
use as_proto::Event;
use tokio::sync::{Notify, broadcast, mpsc, oneshot};

use crate::notifications::{self, NotificationService};
use crate::registry::Registry;
use crate::{DaemonOptions, SessionCatalogs};

const SAVE_DEBOUNCE: Duration = Duration::from_millis(500);
const CONFIG_POLL: Duration = Duration::from_millis(500);
const STOP_AGENTS_TIMEOUT: Duration = Duration::from_secs(5);

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
                self.notify(self.notifications.waiting(&agent_id, &name));
                self.registry.broadcast(&Event::AgentWaiting { agent_id });
            }
            AgentEvent::Changed => {
                self.registry.broadcast(&Event::AgentsChanged);
                self.request_save();
            }
        }
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
