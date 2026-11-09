//! The request/event plumbing between a front end and the daemon, without any UI toolkit.
//!
//! A [`DaemonLink`] owns the one shared `DaemonClient`. Requests arrive as the JSON of an [`as_proto::Request`] and leave as the
//! JSON of the [`as_proto::Response`] (no per-method code here: the Hydra API -> protocol mapping lives in TypeScript). [`DaemonLink::run`]
//! keeps the connection alive: `ensure_daemon`, forward every event to a [`Sink`], reconnect with backoff when the connection drops
//! and announce each change as a [`DaemonStatus`] (a `connected: true` status tells the front end to re-subscribe, because
//! subscriptions belong to the connection).

use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use anyhow::Result;
use as_core::paths::Paths;
use as_proto::{DaemonClient, Request};
use futures_util::FutureExt;
use futures_util::future::BoxFuture;
use serde::Serialize;
use serde_json::Value;

/// Connection state shown to the user (`daemon-status` event, `daemon_status` command).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DaemonStatus {
    pub connected: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Connected, but something needs the user's attention (e.g. an older daemon keeps running because agents are running).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
}

impl DaemonStatus {
    pub fn connected() -> Self {
        Self {
            connected: true,
            error: None,
            warning: None,
        }
    }
    pub fn connected_with_warning(warning: impl Into<String>) -> Self {
        Self {
            connected: true,
            error: None,
            warning: Some(warning.into()),
        }
    }
    pub fn disconnected(error: impl Into<String>) -> Self {
        Self {
            connected: false,
            error: Some(error.into()),
            warning: None,
        }
    }
}

/// What the link pushes to the front end.
#[derive(Debug, Clone, PartialEq)]
pub enum Outbound {
    /// JSON of an `as_proto::Event` (`{"type": "...", "payload": ...}`).
    Event(Value),
    Status(DaemonStatus),
}

pub type Sink = Arc<dyn Fn(Outbound) + Send + Sync>;

/// How to obtain a connected, handshaken client (production: `ensure_daemon`; tests inject their own).
pub type Connector = Arc<dyn Fn() -> BoxFuture<'static, Result<DaemonClient>> + Send + Sync>;

/// The production connector: start the daemon when none runs (the GUI binary hosts it) and connect.
pub fn ensure_connector(paths: Paths) -> Connector {
    Arc::new(move || {
        let paths = paths.clone();
        async move { as_proto::lifecycle::ensure_daemon(&paths).await }.boxed()
    })
}

/// A connected daemon of another version/protocol than this app. `ensure_daemon` already replaced an *idle* one (nothing running),
/// so what reaches the link is a daemon that was kept because agents run: say so, plainly. `None` when it matches the app.
pub fn version_notice(status: &as_proto::DaemonStatus) -> Option<String> {
    let ours = as_core::VERSION;
    if status.version == ours && status.protocol == as_proto::PROTOCOL_VERSION {
        return None;
    }
    let running = if status.running_count == 1 {
        "1 agent is running".to_string()
    } else {
        format!("{} agents are running", status.running_count)
    };
    Some(format!(
        "The background daemon is v{} but this app is v{ours}. It was kept because {running}; \
         quit all agents and restart the app to update it.",
        status.version
    ))
}

pub struct DaemonLink {
    client: RwLock<Option<Arc<DaemonClient>>>,
    status: Mutex<DaemonStatus>,
}

const BACKOFF_START: Duration = Duration::from_millis(500);
const BACKOFF_MAX: Duration = Duration::from_secs(5);

impl DaemonLink {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            client: RwLock::new(None),
            status: Mutex::new(DaemonStatus::disconnected("connecting to the daemon")),
        })
    }

    pub fn status(&self) -> DaemonStatus {
        self.status
            .lock()
            .map(|s| s.clone())
            .unwrap_or_else(|_| DaemonStatus::disconnected("status unavailable"))
    }

    fn set_status(&self, status: DaemonStatus) {
        if let Ok(mut current) = self.status.lock() {
            *current = status;
        }
    }

    fn current(&self) -> Option<Arc<DaemonClient>> {
        self.client.read().ok().and_then(|c| c.clone())
    }

    fn set_client(&self, client: Option<Arc<DaemonClient>>) {
        if let Ok(mut slot) = self.client.write() {
            *slot = client;
        }
    }

    /// Send one request (JSON of `as_proto::Request`) and return the JSON of the `Response`.
    /// Daemon errors come back as their message.
    pub async fn request(&self, request: Value) -> Result<Value, String> {
        let request: Request =
            serde_json::from_value(request).map_err(|err| format!("invalid request: {err}"))?;
        let client = self
            .current()
            .ok_or_else(|| "daemon not connected".to_string())?;
        let response = client
            .request(request)
            .await
            .map_err(|err| err.to_string())?;
        serde_json::to_value(response).map_err(|err| format!("cannot encode response: {err}"))
    }

    /// Connect, forward events, reconnect with backoff. Runs until the task is aborted.
    pub async fn run(self: Arc<Self>, connector: Connector, sink: Sink) {
        let mut backoff = BACKOFF_START;
        let mut last_error: Option<String> = None;
        loop {
            match connector().await {
                Ok(client) => {
                    backoff = BACKOFF_START;
                    last_error = None;
                    let client = Arc::new(client);
                    let Some(mut events) = client.take_events() else {
                        tracing::error!("event stream already taken");
                        tokio::time::sleep(backoff).await;
                        continue;
                    };
                    let connected =
                        match tokio::time::timeout(Duration::from_secs(3), client.status()).await {
                            Ok(Ok(status)) => match version_notice(&status) {
                                Some(warning) => {
                                    tracing::warn!(%warning, "daemon version mismatch");
                                    DaemonStatus::connected_with_warning(warning)
                                }
                                None => DaemonStatus::connected(),
                            },
                            _ => DaemonStatus::connected(),
                        };
                    self.set_client(Some(client));
                    self.set_status(connected.clone());
                    sink(Outbound::Status(connected));
                    tracing::info!("connected to the daemon");
                    while let Some(event) = events.recv().await {
                        match serde_json::to_value(&event) {
                            Ok(value) => sink(Outbound::Event(value)),
                            Err(err) => tracing::warn!(%err, "cannot encode daemon event"),
                        }
                    }
                    self.set_client(None);
                    let status = DaemonStatus::disconnected("daemon connection closed");
                    self.set_status(status.clone());
                    sink(Outbound::Status(status));
                    tracing::warn!("daemon connection closed, reconnecting");
                }
                Err(err) => {
                    let message = format!("{err:#}");
                    if last_error.as_deref() != Some(&message) {
                        tracing::warn!(error = %message, "cannot reach the daemon");
                        let status = DaemonStatus::disconnected(message.clone());
                        self.set_status(status.clone());
                        sink(Outbound::Status(status));
                        last_error = Some(message);
                    }
                }
            }
            tokio::time::sleep(backoff).await;
            backoff = (backoff * 2).min(BACKOFF_MAX);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(version: &str, protocol: u32, running: usize) -> as_proto::DaemonStatus {
        as_proto::DaemonStatus {
            pid: 1,
            version: version.into(),
            protocol,
            uptime_secs: 1,
            agent_count: running,
            running_count: running,
            clients: 1,
        }
    }

    #[test]
    fn matching_daemon_has_no_notice() {
        let ok = status(as_core::VERSION, as_proto::PROTOCOL_VERSION, 2);
        assert_eq!(version_notice(&ok), None);
    }

    #[test]
    fn other_version_or_protocol_is_reported_with_the_agent_count() {
        let old = version_notice(&status("0.0.1", as_proto::PROTOCOL_VERSION, 1)).unwrap();
        assert!(
            old.contains("v0.0.1") && old.contains("1 agent is running"),
            "{old}"
        );
        let other =
            version_notice(&status(as_core::VERSION, as_proto::PROTOCOL_VERSION + 1, 3)).unwrap();
        assert!(other.contains("3 agents are running"), "{other}");
    }

    #[test]
    fn warning_travels_with_a_connected_status() {
        let json = serde_json::to_value(DaemonStatus::connected_with_warning("w")).unwrap();
        assert_eq!(json, serde_json::json!({"connected": true, "warning": "w"}));
        let json = serde_json::to_value(DaemonStatus::connected()).unwrap();
        assert_eq!(json, serde_json::json!({"connected": true}));
    }
}
