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
}

impl DaemonStatus {
    pub fn connected() -> Self {
        Self {
            connected: true,
            error: None,
        }
    }
    pub fn disconnected(error: impl Into<String>) -> Self {
        Self {
            connected: false,
            error: Some(error.into()),
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
                    self.set_client(Some(client));
                    self.set_status(DaemonStatus::connected());
                    sink(Outbound::Status(DaemonStatus::connected()));
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
