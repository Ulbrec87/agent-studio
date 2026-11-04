//! In-process daemon on a temp data root with a fake PTY backend.

use std::sync::Arc;
use std::time::Duration;

use as_core::agents::fake_pty::FakePtyBackend;
use as_core::paths::Paths;
use as_core::{AgentStatus, CreateAgentPayload, ManagerOptions, ProviderId};
use as_daemon::{DaemonOptions, serve};
use as_proto::lifecycle::{ensure_daemon_with, stop_daemon};
use as_proto::{ClientMsg, DaemonClient, Event, Request, ServerMsg, codec, lock, transport};
use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

struct Harness {
    _dir: tempfile::TempDir,
    paths: Paths,
    backend: FakePtyBackend,
    stop: CancellationToken,
    task: JoinHandle<anyhow::Result<()>>,
}

fn options(backend: &FakePtyBackend) -> DaemonOptions {
    DaemonOptions {
        manager: ManagerOptions {
            backend: Arc::new(backend.clone()),
            kill_grace: Duration::from_millis(50),
            ..Default::default()
        },
        preflight_on_create: false,
        apply_fixed_path: false,
    }
}

async fn wait_for_socket(paths: &Paths) {
    for _ in 0..400 {
        if transport::connect(paths).await.is_ok() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("daemon did not come up");
}

async fn start() -> Harness {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::at(dir.path());
    let backend = FakePtyBackend::new();
    let stop = CancellationToken::new();
    let task = tokio::spawn(serve(paths.clone(), options(&backend), stop.clone()));
    wait_for_socket(&paths).await;
    Harness {
        _dir: dir,
        paths,
        backend,
        stop,
        task,
    }
}

/// Fake processes ignore signals; make them exit when asked to terminate, like real ones.
fn exit_on_terminate(backend: &FakePtyBackend) {
    let backend = backend.clone();
    tokio::spawn(async move {
        loop {
            for pty in backend.ptys() {
                if !pty.signals().is_empty() {
                    pty.emit_exit(0);
                }
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    });
}

fn payload() -> CreateAgentPayload {
    CreateAgentPayload {
        name: "demo".into(),
        project_dir: "/tmp".into(),
        provider: ProviderId::Claude,
        model: "sonnet".into(),
        ..Default::default()
    }
}

async fn next_event(
    rx: &mut UnboundedReceiver<Event>,
    what: &str,
    mut pick: impl FnMut(&Event) -> bool,
) -> Event {
    let wait = async {
        loop {
            match rx.recv().await {
                Some(e) if pick(&e) => return e,
                Some(_) => {}
                None => panic!("event stream closed while waiting for {what}"),
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(5), wait)
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for {what}"))
}

#[tokio::test]
async fn agent_flow_output_status_and_clean_shutdown() {
    let h = start().await;
    let client = DaemonClient::connect(&h.paths).await.unwrap();
    let mut events = client.take_events().unwrap();
    assert!(client.take_events().is_none());

    let hello = client.hello().await.unwrap();
    assert_eq!(hello.daemon_version, as_core::VERSION);
    client.ping().await.unwrap();

    let agent = client.create_agent(payload()).await.unwrap();
    assert_eq!(agent.name, "demo");
    next_event(&mut events, "agents changed", |e| {
        *e == Event::AgentsChanged
    })
    .await;
    assert_eq!(client.list_agents().await.unwrap().len(), 1);
    assert!(client.get_agent("nope").await.unwrap().is_none());

    let pty = h.backend.ptys()[0].clone();
    pty.emit_data(b"before ");
    // Wait until the manager buffered it, then subscribe: it must come back in the buffer.
    let mut buffered = Vec::new();
    for _ in 0..200 {
        buffered = client.subscribe(&agent.id).await.unwrap();
        if !buffered.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(buffered, b"before ");

    pty.emit_data(&[b'h', b'i', 0xe2, 0x82]); // ends inside a UTF-8 sequence
    let Event::AgentOutput { agent_id, data } = next_event(&mut events, "output", |e| {
        matches!(e, Event::AgentOutput { .. })
    })
    .await
    else {
        unreachable!()
    };
    assert_eq!(agent_id, agent.id);
    assert_eq!(data, vec![b'h', b'i', 0xe2, 0x82]);

    assert!(client.send_raw(&agent.id, b"abc".to_vec()).await.unwrap());
    assert!(client.resize(&agent.id, 100, 40).await.is_ok());
    let mut wrote = false;
    for _ in 0..200 {
        if pty.written().contains("abc") && pty.resizes().contains(&(100, 40)) {
            wrote = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(wrote, "raw input and resize reach the PTY");

    // Unsubscribed: no more output for this client.
    client.unsubscribe(&agent.id).await.unwrap();
    pty.emit_data(b"quiet");
    pty.emit_exit(0);
    let status = next_event(&mut events, "idle status", |e| {
        matches!(
            e,
            Event::AgentStatus {
                status: AgentStatus::Idle,
                ..
            }
        )
    })
    .await;
    assert!(matches!(status, Event::AgentStatus { agent_id: ref id, .. } if *id == agent.id));

    let st = client.status().await.unwrap();
    assert_eq!((st.agent_count, st.running_count, st.clients), (1, 0, 1));

    let cfg = client
        .update_config(serde_json::json!({"maxAgents": 3}))
        .await
        .unwrap();
    assert_eq!(cfg.max_agents, 3);
    next_event(
        &mut events,
        "config changed",
        |e| matches!(e, Event::ConfigChanged(c) if c.max_agents == 3),
    )
    .await;

    // Idle daemon may be stopped without --force.
    client.shutdown(false).await.unwrap();
    next_event(&mut events, "shutting down", |e| {
        *e == Event::DaemonShuttingDown
    })
    .await;
    tokio::time::timeout(Duration::from_secs(5), h.task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(!h.paths.daemon_lock_file().exists());
    assert!(!std::path::Path::new(&h.paths.daemon_socket_name()).exists());
    // The agent list was persisted.
    let saved = std::fs::read_to_string(h.paths.workspace_file()).unwrap();
    assert!(saved.contains("\"demo\""), "{saved}");
}

#[tokio::test]
async fn shutdown_without_force_is_refused_while_an_agent_runs() {
    let h = start().await;
    exit_on_terminate(&h.backend);
    let client = DaemonClient::connect(&h.paths).await.unwrap();
    client.create_agent(payload()).await.unwrap();
    let err = client.shutdown(false).await.unwrap_err().to_string();
    assert!(err.contains("agents are running"), "{err}");
    client.ping().await.unwrap();
    // A forced one stops the agents and the daemon.
    client.shutdown(true).await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), h.task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(
        !h.backend.ptys()[0].signals().is_empty(),
        "agent was terminated"
    );
}

#[tokio::test]
async fn cancelling_the_token_stops_agents_and_cleans_up() {
    let h = start().await;
    exit_on_terminate(&h.backend);
    let client = DaemonClient::connect(&h.paths).await.unwrap();
    client.create_agent(payload()).await.unwrap();
    h.stop.cancel();
    tokio::time::timeout(Duration::from_secs(8), h.task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(!h.paths.daemon_lock_file().exists());
    assert!(!h.backend.ptys()[0].signals().is_empty());
}

#[tokio::test]
async fn second_daemon_refuses_to_start() {
    let h = start().await;
    let second = serve(
        h.paths.clone(),
        options(&h.backend),
        CancellationToken::new(),
    )
    .await;
    let err = second.unwrap_err().to_string();
    assert!(err.contains("already running"), "{err}");
    // The first one is untouched.
    let client = DaemonClient::connect(&h.paths).await.unwrap();
    client.ping().await.unwrap();
    assert!(lock::read(&h.paths).is_some());
    client.shutdown(true).await.unwrap();
    h.task.await.unwrap().unwrap();
}

#[tokio::test]
async fn malformed_frames_get_an_error_and_do_not_kill_the_daemon() {
    let h = start().await;
    let conn = transport::connect(&h.paths).await.unwrap();
    let (mut sink, mut stream) = codec::framed(conn).split();

    sink.send(Bytes::from_static(b"this is not json"))
        .await
        .unwrap();
    let frame = stream.next().await.unwrap().unwrap();
    let ServerMsg::Response { id, result } = codec::decode(&frame).unwrap() else {
        panic!("expected a response")
    };
    assert_eq!(id, 0);
    assert!(result.unwrap_err().contains("malformed"));

    // Unknown request type, but with an id: the error carries the id.
    sink.send(Bytes::from_static(
        br#"{"type":"request","payload":{"id":9,"req":{"type":"nope"}}}"#,
    ))
    .await
    .unwrap();
    let frame = stream.next().await.unwrap().unwrap();
    let ServerMsg::Response { id, result } = codec::decode(&frame).unwrap() else {
        panic!("expected a response")
    };
    assert_eq!(id, 9);
    assert!(result.is_err());

    // The same connection still works.
    sink.send(
        codec::encode(&ClientMsg::Request {
            id: 10,
            req: Request::Ping,
        })
        .unwrap(),
    )
    .await
    .unwrap();
    let frame = stream.next().await.unwrap().unwrap();
    assert!(matches!(
        codec::decode::<ServerMsg>(&frame).unwrap(),
        ServerMsg::Response {
            id: 10,
            result: Ok(as_proto::Response::Pong)
        }
    ));
    h.stop.cancel();
    h.task.await.unwrap().unwrap();
}

#[tokio::test]
async fn ensure_daemon_spawns_when_missing_and_stop_daemon_stops_it() {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::at(dir.path());
    let backend = FakePtyBackend::new();
    let task: Arc<parking_lot::Mutex<Option<JoinHandle<anyhow::Result<()>>>>> = Arc::default();

    // Leftovers of a crashed daemon must be cleaned up first.
    std::fs::write(
        paths.daemon_lock_file(),
        r#"{"pid":999999999,"version":"0","socket":"x","started_at":"x"}"#,
    )
    .unwrap();

    let spawn = {
        let (task, backend) = (task.clone(), backend.clone());
        move |p: &Paths| {
            let handle = tokio::spawn(serve(
                p.clone(),
                options(&backend),
                CancellationToken::new(),
            ));
            *task.lock() = Some(handle);
            Ok(())
        }
    };
    let client = ensure_daemon_with(&paths, &spawn).await.unwrap();
    assert_eq!(client.hello().await.unwrap().pid, std::process::id());
    assert!(task.lock().is_some(), "spawned exactly once");

    // A second call finds the running one and does not spawn.
    let again = ensure_daemon_with(&paths, |_: &Paths| -> anyhow::Result<()> {
        panic!("must not spawn")
    })
    .await
    .unwrap();
    assert_eq!(again.status().await.unwrap().clients, 2);

    assert!(stop_daemon(&paths, false).await.unwrap());
    let handle = task.lock().take().unwrap();
    handle.await.unwrap().unwrap();
    assert!(
        !stop_daemon(&paths, false).await.unwrap(),
        "nothing left to stop"
    );
}
