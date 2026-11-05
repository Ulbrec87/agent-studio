//! Dev bridge: the request/event plumbing of [`crate::link`] behind a localhost WebSocket.
//!
//! **Dev/test tool only.** It lets the Vite front end run in a normal browser against a real daemon (browser automation,
//! screenshots). It is never started without `--dev-bridge`, listens on `127.0.0.1` only, on a random port, and refuses
//! any connection whose URL does not carry the random token (`ws://127.0.0.1:<port>/?token=<token>`).
//!
//! One JSON text frame per message:
//! * client -> server `{"id": N, "request": <as_proto::Request JSON>}`; the special request `{"kind": "app_info"}` answers the shell's `AppInfo`,
//!   `{"kind": "shell", "command": "log_event", "args": {..}}` runs the one shell command a browser may use (the renderer log).
//! * server -> client `{"id": N, "result": <Response JSON>}` or `{"id": N, "error": "<message>"}`
//! * server -> client pushes `{"event": <Event JSON>}` and `{"status": {"connected": bool, "error"?: string}}`
//!   (the current status is sent right after connecting).
//!
//! All browser tabs share the shell's single daemon connection (and therefore its output subscriptions).

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::{Context, Result};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{broadcast, mpsc};
use tokio::task::JoinHandle;
use tokio_tungstenite::accept_hdr_async;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tokio_tungstenite::tungstenite::http::StatusCode;

use crate::link::{DaemonLink, Outbound, Sink};

/// Fan-out of the link's output to every connected WebSocket.
#[derive(Clone)]
pub struct Hub {
    tx: broadcast::Sender<Outbound>,
}

impl Default for Hub {
    fn default() -> Self {
        Self::new()
    }
}

impl Hub {
    pub fn new() -> Self {
        Self {
            tx: broadcast::channel(8192).0,
        }
    }

    /// The sink to give to `DaemonLink::run`.
    pub fn sink(&self) -> Sink {
        let tx = self.tx.clone();
        Arc::new(move |out| {
            // No receiver (no browser connected) is fine.
            let _ = tx.send(out);
        })
    }
}

/// A random, unguessable token (UUID v4 from the OS RNG, 122 bits).
pub fn new_token() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

/// The one line printed on stdout so scripts can find the bridge.
pub fn announce_line(addr: SocketAddr, token: &str) -> String {
    format!("DEV_BRIDGE ws://{addr} token={token}")
}

pub struct Bound {
    pub addr: SocketAddr,
    pub task: JoinHandle<()>,
}

/// Listen on `127.0.0.1:0` and serve connections until the task is aborted.
pub async fn bind(
    link: Arc<DaemonLink>,
    hub: Hub,
    token: String,
    app_info: Value,
) -> Result<Bound> {
    let listener = TcpListener::bind(("127.0.0.1", 0))
        .await
        .context("binding the dev bridge socket")?;
    let addr = listener.local_addr()?;
    let token: Arc<str> = token.into();
    let task = tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                continue;
            };
            let (link, hub, token, app_info) =
                (link.clone(), hub.clone(), token.clone(), app_info.clone());
            tokio::spawn(async move {
                if let Err(err) = serve_connection(stream, link, hub, token, app_info).await {
                    tracing::debug!(%err, "dev bridge connection ended");
                }
            });
        }
    });
    Ok(Bound { addr, task })
}

/// The shell commands a browser may use through the dev bridge: only the renderer log (there is no window to open anything in,
/// quit or ask; "open in" stays a daemon request there). Request: `{"kind":"shell","command":"log_event","args":{"event":{..}}}`.
fn run_shell_command(request: &Value) -> Result<Value, String> {
    match request.get("command").and_then(Value::as_str) {
        Some("log_event") => {
            if let Some(event) = request.get("args").and_then(|a| a.get("event")) {
                crate::gui_log::record(event);
            }
            Ok(Value::Null)
        }
        other => Err(format!(
            "shell command not available in the dev bridge: {}",
            other.unwrap_or("?")
        )),
    }
}

fn has_token(query: Option<&str>, token: &str) -> bool {
    query
        .unwrap_or("")
        .split('&')
        .any(|pair| pair.strip_prefix("token=") == Some(token))
}

async fn serve_connection(
    stream: TcpStream,
    link: Arc<DaemonLink>,
    hub: Hub,
    token: Arc<str>,
    app_info: Value,
) -> Result<()> {
    // The error type is dictated by tungstenite's callback signature.
    #[allow(clippy::result_large_err)]
    let check = |req: &Request, resp: Response| {
        if has_token(req.uri().query(), &token) {
            Ok(resp)
        } else {
            let mut err = ErrorResponse::new(Some("invalid token".to_string()));
            *err.status_mut() = StatusCode::UNAUTHORIZED;
            Err(err)
        }
    };
    let ws = accept_hdr_async(stream, check).await?;
    let (mut sink, mut stream) = ws.split();
    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<String>();

    let writer = tokio::spawn(async move {
        while let Some(text) = out_rx.recv().await {
            if sink.send(Message::text(text)).await.is_err() {
                break;
            }
        }
    });

    // Subscribe before reading the current status so a change in between is not lost.
    let mut events = hub.tx.subscribe();
    let _ = out_tx.send(json!({ "status": link.status() }).to_string());
    let pusher = {
        let out_tx = out_tx.clone();
        tokio::spawn(async move {
            loop {
                let frame = match events.recv().await {
                    Ok(Outbound::Event(event)) => json!({ "event": event }),
                    Ok(Outbound::Status(status)) => json!({ "status": status }),
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => break,
                };
                if out_tx.send(frame.to_string()).is_err() {
                    break;
                }
            }
        })
    };

    while let Some(Ok(message)) = stream.next().await {
        let Message::Text(text) = message else {
            continue;
        };
        let Ok(frame) = serde_json::from_str::<Value>(text.as_str()) else {
            continue;
        };
        let Some(id) = frame.get("id").cloned() else {
            continue;
        };
        let request = frame.get("request").cloned().unwrap_or(Value::Null);
        let (link, app_info, out_tx) = (link.clone(), app_info.clone(), out_tx.clone());
        tokio::spawn(async move {
            let result = match request.get("kind").and_then(Value::as_str) {
                Some("app_info") => Ok(app_info),
                Some("shell") => run_shell_command(&request),
                _ => link.request(request).await,
            };
            let reply = match result {
                Ok(result) => json!({ "id": id, "result": result }),
                Err(error) => json!({ "id": id, "error": error }),
            };
            let _ = out_tx.send(reply.to_string());
        });
    }
    pusher.abort();
    writer.abort();
    Ok(())
}
