//! The link and the dev bridge against an in-process daemon (fake PTY backend), no Tauri involved.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use as_core::agents::fake_pty::FakePtyBackend;
use as_core::paths::Paths;
use as_core::sessions::{ClaudeSessionCatalog, CodexSessionCatalog};
use as_core::{CreateAgentPayload, ManagerOptions, ProviderId};
use as_daemon::{DaemonOptions, SessionCatalogs, UsageOptions, serve};
use as_gui::devbridge::{self, Hub};
use as_gui::link::{Connector, DaemonLink, DaemonStatus, Outbound, Sink};
use as_proto::{DaemonClient, Request, transport};
use futures_util::{FutureExt, SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;
use tokio_util::sync::CancellationToken;

struct Daemon {
    _dir: tempfile::TempDir,
    paths: Paths,
    backend: FakePtyBackend,
    stop: CancellationToken,
}

fn options(backend: &FakePtyBackend) -> DaemonOptions {
    DaemonOptions {
        manager: ManagerOptions {
            backend: Arc::new(backend.clone()),
            kill_grace: Duration::from_millis(50),
            ..Default::default()
        },
        catalogs: SessionCatalogs {
            claude: Arc::new(ClaudeSessionCatalog::with_dirs(Vec::new(), 0)),
            codex: Arc::new(CodexSessionCatalog::with_dir(
                std::env::temp_dir().join("as-gui-test-no-codex"),
                0,
            )),
        },
        catalog_locator: true,
        import_sessions: false,
        preflight_on_create: false,
        apply_fixed_path: false,
        os_notifications: false,
        overview: false,
        overview_config_dirs: Some(Vec::new()),
        usage: false,
        mcp: false,
        updates: false,
        update_http: None,
        headless_override: None,
        usage_options: UsageOptions {
            sources: Some(Vec::new()),
            profiles: Some(Vec::new()),
            ..Default::default()
        },
        skills: None,
        hydra_dir: None,
    }
}

async fn start_daemon() -> Daemon {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::at(dir.path());
    let backend = FakePtyBackend::new();
    let stop = CancellationToken::new();
    tokio::spawn(serve(paths.clone(), options(&backend), stop.clone()));
    for _ in 0..400 {
        if transport::connect(&paths).await.is_ok() {
            return Daemon {
                _dir: dir,
                paths,
                backend,
                stop,
            };
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("daemon did not come up");
}

fn connector(paths: Paths, attempts: Arc<AtomicUsize>, fail_first: usize) -> Connector {
    Arc::new(move || {
        let paths = paths.clone();
        let attempts = attempts.clone();
        async move {
            if attempts.fetch_add(1, Ordering::SeqCst) < fail_first {
                anyhow::bail!("not yet");
            }
            DaemonClient::connect(&paths).await
        }
        .boxed()
    })
}

fn channel_sink() -> (Sink, UnboundedReceiver<Outbound>) {
    let (tx, rx) = unbounded_channel();
    (
        Arc::new(move |out| {
            let _ = tx.send(out);
        }),
        rx,
    )
}

async fn recv_until(
    rx: &mut UnboundedReceiver<Outbound>,
    what: &str,
    mut pick: impl FnMut(&Outbound) -> bool,
) -> Outbound {
    let wait = async {
        loop {
            match rx.recv().await {
                Some(out) if pick(&out) => return out,
                Some(_) => {}
                None => panic!("sink closed while waiting for {what}"),
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(10), wait)
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for {what}"))
}

fn req(request: Request) -> Value {
    serde_json::to_value(request).unwrap()
}

fn is_connected(out: &Outbound) -> bool {
    matches!(out, Outbound::Status(s) if s.connected)
}

#[tokio::test]
async fn requests_round_trip_as_protocol_json_and_events_are_forwarded() {
    let daemon = start_daemon().await;
    let link = DaemonLink::new();
    // Before the first connection every request fails cleanly.
    assert_eq!(
        link.request(req(Request::Ping)).await.unwrap_err(),
        "daemon not connected"
    );
    let (sink, mut rx) = channel_sink();
    let attempts = Arc::new(AtomicUsize::new(0));
    let run = tokio::spawn(
        link.clone()
            .run(connector(daemon.paths.clone(), attempts, 0), sink),
    );
    recv_until(&mut rx, "connected", is_connected).await;
    assert_eq!(link.status(), DaemonStatus::connected());

    assert_eq!(
        link.request(req(Request::Ping)).await.unwrap(),
        json!({"type": "pong"})
    );
    let created = link
        .request(req(Request::CreateAgent(CreateAgentPayload {
            name: "demo".into(),
            project_dir: std::env::temp_dir().to_string_lossy().into_owned(),
            provider: ProviderId::Claude,
            model: "sonnet".into(),
            ..Default::default()
        })))
        .await
        .unwrap();
    assert_eq!(created["type"], "agent");
    let id = created["payload"]["id"].as_str().unwrap().to_string();
    assert_eq!(created["payload"]["name"], "demo");

    let buffer = link
        .request(req(Request::Subscribe {
            agent_id: id.clone(),
        }))
        .await
        .unwrap();
    assert_eq!(buffer["type"], "buffer");

    daemon.backend.ptys()[0].emit_data(&[0xe2, 0x82, b'x']);
    let out = recv_until(
        &mut rx,
        "agent_output",
        |o| matches!(o, Outbound::Event(e) if e["type"] == "agent_output"),
    )
    .await;
    let Outbound::Event(event) = out else {
        unreachable!()
    };
    assert_eq!(event["payload"]["agent_id"], json!(id));
    // 0xe2 0x82 'x' in base64: bytes survive untouched (UTF-8 is split by the front end's streaming decoder).
    assert_eq!(event["payload"]["data"], "4oJ4");

    // Malformed JSON is rejected before it reaches the daemon.
    assert!(
        link.request(json!({"type": "definitely_not_a_request"}))
            .await
            .unwrap_err()
            .starts_with("invalid request")
    );
    run.abort();
    daemon.stop.cancel();
}

#[tokio::test]
async fn connects_with_backoff_and_reconnects_after_the_daemon_closes_the_connection() {
    let daemon = start_daemon().await;
    let link = DaemonLink::new();
    let (sink, mut rx) = channel_sink();
    let attempts = Arc::new(AtomicUsize::new(0));
    let run = tokio::spawn(
        link.clone()
            .run(connector(daemon.paths.clone(), attempts.clone(), 1), sink),
    );
    let first = recv_until(&mut rx, "first status", |o| {
        matches!(o, Outbound::Status(_))
    })
    .await;
    assert_eq!(
        first,
        Outbound::Status(DaemonStatus::disconnected("not yet"))
    );
    recv_until(&mut rx, "connected", is_connected).await;
    assert_eq!(attempts.load(Ordering::SeqCst), 2);
    run.abort();
    daemon.stop.cancel();
}

#[tokio::test]
async fn reconnects_when_the_connection_drops() {
    let daemon = start_daemon().await;
    let link = DaemonLink::new();
    let (sink, mut rx) = channel_sink();
    // The first client is a private connection we can kill from the outside.
    let (client_io, server_io) = tokio::io::duplex(1 << 16);
    drop(server_io); // never served: the stream ends at once
    let first = Arc::new(std::sync::Mutex::new(Some(DaemonClient::from_stream(
        client_io,
    ))));
    let paths = daemon.paths.clone();
    let connector: Connector = Arc::new(move || {
        let first = first.clone();
        let paths = paths.clone();
        async move {
            let taken = first.lock().unwrap().take();
            match taken {
                Some(c) => Ok(c),
                None => DaemonClient::connect(&paths).await,
            }
        }
        .boxed()
    });
    let run = tokio::spawn(link.clone().run(connector, sink));
    recv_until(&mut rx, "connected", is_connected).await;
    // `server_io` was dropped, so the first stream ends: disconnected, then connected again.
    recv_until(
        &mut rx,
        "disconnected",
        |o| matches!(o, Outbound::Status(s) if !s.connected),
    )
    .await;
    recv_until(&mut rx, "reconnected", is_connected).await;
    assert_eq!(
        link.request(req(Request::Ping)).await.unwrap(),
        json!({"type": "pong"})
    );
    run.abort();
    daemon.stop.cancel();
}

#[tokio::test]
async fn dev_bridge_rejects_a_missing_or_wrong_token_and_serves_requests_and_pushes() {
    let daemon = start_daemon().await;
    let link = DaemonLink::new();
    let hub = Hub::new();
    let attempts = Arc::new(AtomicUsize::new(0));
    let run = tokio::spawn(
        link.clone()
            .run(connector(daemon.paths.clone(), attempts, 0), hub.sink()),
    );
    let token = devbridge::new_token();
    assert_eq!(token.len(), 32);
    let bound = devbridge::bind(
        link.clone(),
        hub.clone(),
        token.clone(),
        json!({"name": "Agent Studio"}),
    )
    .await
    .unwrap();
    assert!(bound.addr.ip().is_loopback());
    assert_eq!(
        devbridge::announce_line(bound.addr, &token),
        format!("DEV_BRIDGE ws://{} token={token}", bound.addr)
    );

    for url in [
        format!("ws://{}/", bound.addr),
        format!("ws://{}/?token=wrong", bound.addr),
        format!("ws://{}/?tok=en&token={token}x", bound.addr),
    ] {
        assert!(connect_async(url.as_str()).await.is_err(), "{url}");
    }

    let (mut ws, _) = connect_async(format!("ws://{}/?x=1&token={token}", bound.addr))
        .await
        .unwrap();
    // Wait until the link is connected so requests reach the daemon.
    for _ in 0..500 {
        if link.status().connected {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let next = async |ws: &mut tokio_tungstenite::WebSocketStream<_>| -> Value {
        loop {
            let frame = tokio::time::timeout(Duration::from_secs(10), ws.next())
                .await
                .expect("frame in time")
                .expect("open")
                .expect("ok");
            if let Message::Text(text) = frame {
                return serde_json::from_str(text.as_str()).unwrap();
            }
        }
    };
    let status = next(&mut ws).await;
    assert!(status.get("status").is_some(), "{status}");

    ws.send(Message::text(
        json!({"id": 1, "request": {"type": "ping"}}).to_string(),
    ))
    .await
    .unwrap();
    ws.send(Message::text(
        json!({"id": 2, "request": {"kind": "app_info"}}).to_string(),
    ))
    .await
    .unwrap();
    ws.send(Message::text(
        json!({"id": 3, "request": {"type": "nope"}}).to_string(),
    ))
    .await
    .unwrap();
    let mut seen = std::collections::HashMap::new();
    while seen.len() < 3 {
        let frame = next(&mut ws).await;
        if let Some(id) = frame.get("id").and_then(Value::as_u64) {
            seen.insert(id, frame);
        }
    }
    assert_eq!(seen[&1]["result"], json!({"type": "pong"}));
    assert_eq!(seen[&2]["result"]["name"], "Agent Studio");
    assert!(
        seen[&3]["error"]
            .as_str()
            .unwrap()
            .starts_with("invalid request")
    );

    // Pushed events: create an agent, subscribe, emit output.
    let created = link
        .request(req(Request::CreateAgent(CreateAgentPayload {
            name: "d".into(),
            project_dir: std::env::temp_dir().to_string_lossy().into_owned(),
            provider: ProviderId::Claude,
            model: "sonnet".into(),
            ..Default::default()
        })))
        .await
        .unwrap();
    let id = created["payload"]["id"].as_str().unwrap().to_string();
    link.request(req(Request::Subscribe { agent_id: id }))
        .await
        .unwrap();
    daemon.backend.ptys()[0].emit_data(b"hi");
    loop {
        let frame = next(&mut ws).await;
        if frame["event"]["type"] == "agent_output" {
            assert_eq!(frame["event"]["payload"]["data"], "aGk=");
            break;
        }
    }
    bound.task.abort();
    run.abort();
    daemon.stop.cancel();
}
