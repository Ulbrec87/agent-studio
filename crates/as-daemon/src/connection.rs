//! One task per connected client: read requests, answer them in order, push events.

use std::sync::Arc;
use std::time::Duration;

use as_proto::{ClientMsg, ServerMsg, codec};
use futures_util::{SinkExt, StreamExt};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::handler;
use crate::registry::OUTGOING_CAPACITY;
use crate::services::Daemon;

pub async fn handle<T>(daemon: Arc<Daemon>, io: T)
where
    T: AsyncRead + AsyncWrite + Send + 'static,
{
    let (mut sink, mut stream) = codec::framed(io).split();
    let (tx, mut rx) = mpsc::channel::<ServerMsg>(OUTGOING_CAPACITY);
    let kick = CancellationToken::new();
    let client = daemon.registry.add(tx.clone(), kick.clone());
    tracing::debug!(client, "client connected");

    let writer_kick = kick.clone();
    let writer = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            let Ok(frame) = codec::encode(&msg) else {
                tracing::error!("cannot encode an outgoing message");
                continue;
            };
            if sink.send(frame).await.is_err() {
                break;
            }
        }
        writer_kick.cancel();
    });

    loop {
        let frame = tokio::select! {
            () = kick.cancelled() => break,
            frame = stream.next() => frame,
        };
        let bytes = match frame {
            Some(Ok(bytes)) => bytes,
            Some(Err(err)) => {
                // Framing error (e.g. oversized frame): the stream cannot be trusted any more.
                let _ = tx
                    .send(error_response(0, format!("bad frame: {err}")))
                    .await;
                break;
            }
            None => break,
        };
        let reply = match codec::decode::<ClientMsg>(&bytes) {
            Ok(ClientMsg::Request { id, req }) => ServerMsg::Response {
                id,
                result: handler::handle(&daemon, client, req).await,
            },
            Err(err) => {
                tracing::warn!(client, %err, "malformed request");
                error_response(request_id(&bytes), format!("malformed request: {err:#}"))
            }
        };
        if tx.send(reply).await.is_err() {
            break;
        }
    }

    daemon.registry.remove(client);
    daemon.sync_usage_visible();
    drop(tx);
    // Flush what is already queued (the last response), but never hang on a dead peer.
    let _ = tokio::time::timeout(Duration::from_secs(2), writer).await;
    tracing::debug!(client, "client disconnected");
}

fn error_response(id: u64, message: String) -> ServerMsg {
    ServerMsg::Response {
        id,
        result: Err(message),
    }
}

/// Best-effort request id of a frame that did not parse as a `ClientMsg` (0 when unknown).
fn request_id(bytes: &[u8]) -> u64 {
    serde_json::from_slice::<serde_json::Value>(bytes)
        .ok()
        .and_then(|v| v.get("payload")?.get("id")?.as_u64())
        .unwrap_or(0)
}
