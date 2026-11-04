//! Shared daemon state: the services (agents, config, workspace), the client registry,
//! event dispatching, debounced persistence and the shutdown sequence.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use as_core::agents::env::{EnvOverrides, process_env};
use as_core::agents::shell::{Os, ShellConfig, fixed_path};
use as_core::paths::Paths;
use as_core::{AgentEvent, AgentManager, ConfigStore, WorkspaceStore};
use as_proto::Event;
use tokio::sync::{Notify, broadcast, mpsc, oneshot};

use crate::DaemonOptions;
use crate::registry::Registry;

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
    pub registry: Registry,
    pub started: Instant,
    pub preflight_on_create: bool,
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
        let manager = AgentManager::with_options(options.manager);
        {
            let config = config.clone();
            manager.set_shell_config_provider(move || ShellConfig::from_config(&config.get()));
        }
        if options.apply_fixed_path {
            // Runs the login shell (bounded by its own timeout): keep it off the async threads.
            let fixed = tokio::task::spawn_blocking(|| fixed_path(&process_env(), Os::current()))
                .await
                .ok()
                .flatten();
            if let Some(path) = fixed {
                let mut overrides = EnvOverrides::new();
                overrides.insert("PATH".into(), Some(path));
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
            registry: Registry::default(),
            started: Instant::now(),
            preflight_on_create: options.preflight_on_create,
            save: Notify::new(),
            shutdown: Notify::new(),
            shutdown_flag: AtomicBool::new(false),
            services_stopped: AtomicBool::new(false),
            control,
        });
        daemon.spawn_tasks(control_rx);
        daemon
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

    fn dispatch(&self, event: AgentEvent) {
        match event {
            AgentEvent::Output { agent_id, data } => self.registry.output(&agent_id, &data),
            AgentEvent::Status {
                agent_id,
                status,
                session_id,
                model,
            } => {
                self.registry.broadcast(&Event::AgentStatus {
                    agent_id,
                    status,
                    session_id,
                    model,
                });
                self.request_save();
            }
            AgentEvent::Waiting { agent_id } => {
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
