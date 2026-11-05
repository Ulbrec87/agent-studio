//! In-process daemon on a temp data root with a fake PTY backend.

use std::sync::Arc;
use std::time::Duration;

use as_core::agents::fake_pty::FakePtyBackend;
use as_core::paths::Paths;
use as_core::sessions::{ClaudeSessionCatalog, CodexSessionCatalog};
use as_core::{AgentStatus, CreateAgentPayload, ManagerOptions, ProviderId};
use as_daemon::{DaemonOptions, SessionCatalogs, serve};
use as_proto::lifecycle::{ensure_daemon_with, stop_daemon};
use as_proto::{
    ClientMsg, DaemonClient, Event, NotificationKind, Request, ServerMsg, codec, lock, transport,
};
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
        catalogs: empty_catalogs(),
        catalog_locator: true,
        import_sessions: false,
        preflight_on_create: false,
        apply_fixed_path: false,
        os_notifications: false,
        overview: true,
        // Never the developer's real Claude registry.
        overview_config_dirs: Some(Vec::new()),
        // Never the developer's real logs, credentials or the network.
        usage: true,
        mcp: true,
        // The "CLI" of headless runs prints two stream-json lines.
        headless_override: Some(Arc::new(|_run| {
            Some(if cfg!(windows) {
                as_core::agents::shell::SpawnSpec {
                    cmd: "cmd.exe".into(),
                    args: vec![
                        "/c".into(),
                        r#"echo {"type":"system","session_id":"sess-9"} & echo {"type":"result","result":"ok"}"#.into(),
                    ],
                }
            } else {
                as_core::agents::shell::SpawnSpec {
                    cmd: "sh".into(),
                    args: vec![
                        "-c".into(),
                        r#"echo '{"type":"system","session_id":"sess-9"}'; echo '{"type":"result","result":"ok"}'"#.into(),
                    ],
                }
            })
        })),
        usage_options: as_daemon::UsageOptions {
            sources: Some(Vec::new()),
            profiles: Some(Vec::new()),
            ..Default::default()
        },
    }
}

/// Catalogs that see nothing (never the developer's real sessions).
fn empty_catalogs() -> SessionCatalogs {
    SessionCatalogs {
        claude: Arc::new(ClaudeSessionCatalog::with_dirs(Vec::new(), 0)),
        codex: Arc::new(CodexSessionCatalog::with_dir(
            std::path::PathBuf::from("/nonexistent-codex-sessions"),
            0,
        )),
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
    // A chunk racing the Subscribe may be delivered again as a live event
    // (documented in AGENTLOG, P2): skip a duplicate of the buffered bytes.
    let Event::AgentOutput { agent_id, data } = next_event(
        &mut events,
        "output",
        |e| matches!(e, Event::AgentOutput { data, .. } if data.as_slice() != b"before "),
    )
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

/// Next `Response` frame, skipping pushed events (the overview tracker
/// broadcasts snapshots/events to every client, also raw ones).
async fn next_response<S>(stream: &mut S) -> (u64, Result<as_proto::Response, String>)
where
    S: futures_util::Stream<Item = std::io::Result<bytes::BytesMut>> + Unpin,
{
    loop {
        let frame = stream.next().await.unwrap().unwrap();
        if let ServerMsg::Response { id, result } = codec::decode(&frame).unwrap() {
            return (id, result);
        }
    }
}

#[tokio::test]
async fn malformed_frames_get_an_error_and_do_not_kill_the_daemon() {
    let h = start().await;
    let conn = transport::connect(&h.paths).await.unwrap();
    let (mut sink, mut stream) = codec::framed(conn).split();

    sink.send(Bytes::from_static(b"this is not json"))
        .await
        .unwrap();
    let (id, result) = next_response(&mut stream).await;
    assert_eq!(id, 0);
    assert!(result.unwrap_err().contains("malformed"));

    // Unknown request type, but with an id: the error carries the id.
    sink.send(Bytes::from_static(
        br#"{"type":"request","payload":{"id":9,"req":{"type":"nope"}}}"#,
    ))
    .await
    .unwrap();
    let (id, result) = next_response(&mut stream).await;
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
    // Pushed events (the first Overview snapshot) may come first.
    let reply = loop {
        let frame = stream.next().await.unwrap().unwrap();
        match codec::decode::<ServerMsg>(&frame).unwrap() {
            ServerMsg::Event(_) => {}
            response => break response,
        }
    };
    assert!(matches!(
        reply,
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
    // Bounded: a hang here must fail fast instead of burning the CI runner.
    tokio::time::timeout(
        Duration::from_secs(60),
        ensure_daemon_spawns_when_missing_and_stop_daemon_stops_it_body(),
    )
    .await
    .expect("timed out");
}

async fn ensure_daemon_spawns_when_missing_and_stop_daemon_stops_it_body() {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::at(dir.path());
    let backend = FakePtyBackend::new();
    let task: Arc<parking_lot::Mutex<Option<JoinHandle<anyhow::Result<()>>>>> = Arc::default();

    // Leftovers of a crashed daemon must be cleaned up first.
    std::fs::write(
        paths.daemon_lock_file(),
        format!(
            r#"{{"pid":999999999,"version":"0","socket":{},"started_at":"x"}}"#,
            serde_json::to_string(&paths.daemon_socket_name()).unwrap()
        ),
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
    // Windows: a named pipe stays "busy" for new connections (and the final probe) while any
    // server-side instance is still open, so close our own connections before stopping.
    drop(client);
    drop(again);

    assert!(stop_daemon(&paths, false).await.unwrap());
    let handle = task.lock().take().unwrap();
    handle.await.unwrap().unwrap();
    assert!(
        !stop_daemon(&paths, false).await.unwrap(),
        "nothing left to stop"
    );
}

#[tokio::test]
async fn notifications_are_pushed_and_deduplicated() {
    let h = start().await;
    let client = DaemonClient::connect(&h.paths).await.unwrap();
    let mut events = client.take_events().unwrap();
    client.hello().await.unwrap();
    client.set_focus(true).await.unwrap();
    client.set_focus(false).await.unwrap();

    let agent = client.create_agent(payload()).await.unwrap();
    let pick = |kind: NotificationKind| move |e: &Event| matches!(e, Event::Notification(n) if n.kind == kind);
    let Event::Notification(n) =
        next_event(&mut events, "started", pick(NotificationKind::AgentStarted)).await
    else {
        unreachable!()
    };
    assert_eq!(n.agent_id.as_deref(), Some(agent.id.as_str()));
    assert_eq!(n.body, "demo is now running");

    h.backend.ptys()[0].emit_exit(0);
    next_event(&mut events, "completed", pick(NotificationKind::AgentIdle)).await;
    h.stop.cancel();
}

fn write_file(path: &std::path::Path, content: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
}

#[tokio::test]
async fn sessions_import_hide_profiles_and_models() {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::at(dir.path().join("home"));
    let claude_projects = dir.path().join("claude/projects");
    let codex_sessions = dir.path().join("codex/sessions");
    let claude_line = |sid: &str, cwd: &str, prompt: &str| {
        format!(
            "{}\n{}\n",
            serde_json::json!({"sessionId": sid, "cwd": cwd}),
            serde_json::json!({"type":"user","message":{"role":"user","content":prompt}})
        )
    };
    write_file(
        &claude_projects.join("p/s-a.jsonl"),
        &claude_line("s-a", "/proj/api", "Fix the login bug"),
    );
    write_file(
        &claude_projects.join("p/s-hidden.jsonl"),
        &claude_line("s-hidden", "/proj/web", "Hidden one"),
    );
    write_file(
        &codex_sessions.join("2026/01/01/rollout-c-a.jsonl"),
        &format!(
            "{}\n{}\n",
            serde_json::json!({"type":"session_meta","payload":{"id":"c-a","cwd":"/proj/cx","timestamp":"2026-01-01T00:00:00Z"}}),
            serde_json::json!({"type":"event_msg","payload":{"type":"user_message","message":"port it"}})
        ),
    );
    write_file(
        &paths.config_file(),
        r#"{"importSessionsOnStartup": true, "sessionMaxAgeDays": 0, "hiddenSessionIds": ["s-hidden"]}"#,
    );

    let backend = FakePtyBackend::new();
    let mut opts = options(&backend);
    opts.import_sessions = true;
    opts.catalogs = SessionCatalogs {
        claude: Arc::new(ClaudeSessionCatalog::with_dirs(vec![claude_projects], 0)),
        codex: Arc::new(CodexSessionCatalog::with_dir(codex_sessions, 0)),
    };
    let stop = CancellationToken::new();
    let task = tokio::spawn(serve(paths.clone(), opts, stop.clone()));
    wait_for_socket(&paths).await;
    let client = DaemonClient::connect(&paths).await.unwrap();

    // Startup import: visible sessions only, each with its own provider, idle, no process.
    let mut agents = Vec::new();
    for _ in 0..300 {
        agents = client.list_agents().await.unwrap();
        if agents.len() >= 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let mut got: Vec<(String, ProviderId, AgentStatus)> = agents
        .iter()
        .map(|a| (a.id.clone(), a.provider, a.status))
        .collect();
    got.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(
        got,
        vec![
            ("sess-c-a".into(), ProviderId::Codex, AgentStatus::Idle),
            ("sess-s-a".into(), ProviderId::Claude, AgentStatus::Idle),
        ]
    );
    assert_eq!(backend.spawn_count(), 0);

    // Listing honours the hidden ids unless asked.
    let list = |provider, include_hidden| {
        client.list_sessions(as_core::sessions::ListSessionsOptions {
            provider,
            include_hidden,
            ..Default::default()
        })
    };
    let visible = list(ProviderId::Claude, false).await.unwrap();
    assert_eq!(
        visible
            .iter()
            .map(|s| s.session_id.as_str())
            .collect::<Vec<_>>(),
        ["s-a"]
    );
    assert_eq!(visible[0].first_prompt, "Fix the login bug");
    let all = list(ProviderId::Claude, true).await.unwrap();
    assert_eq!(all.len(), 2);
    assert!(all.iter().any(|s| s.session_id == "s-hidden" && s.hidden));
    assert_eq!(list(ProviderId::Codex, false).await.unwrap().len(), 1);
    assert!(list(ProviderId::Opencode, true).await.unwrap().is_empty());

    // Hide / unhide persists in the config.
    let cfg = client.hide_session("s-a", true).await.unwrap();
    assert_eq!(cfg.hidden_session_ids, ["s-hidden", "s-a"]);
    assert!(list(ProviderId::Claude, false).await.unwrap().is_empty());
    let cfg = client.hide_session("s-hidden", false).await.unwrap();
    assert_eq!(cfg.hidden_session_ids, ["s-a"]);
    assert!(
        std::fs::read_to_string(paths.config_file())
            .unwrap()
            .contains("\"s-a\"")
    );

    // Project profiles: stored without the trailing slash, used for new Claude agents.
    let cfg = client
        .set_project_profile("/proj/api/", Some("/x/.claude-work".into()))
        .await
        .unwrap();
    assert_eq!(
        cfg.claude_profile_by_project["/proj/api"],
        "/x/.claude-work"
    );
    let agent = client
        .create_agent(CreateAgentPayload {
            project_dir: "/proj/api/sub".into(),
            ..payload()
        })
        .await
        .unwrap();
    let env = backend.env_of(0);
    assert_eq!(
        env.get("CLAUDE_CONFIG_DIR").map(String::as_str),
        Some("/x/.claude-work"),
        "{}",
        agent.id
    );
    let cfg = client.set_project_profile("/proj/api", None).await.unwrap();
    assert!(cfg.claude_profile_by_project.is_empty());
    assert!(client.set_project_profile("  ", None).await.is_err());
    client.list_claude_profiles().await.unwrap();

    // Models: Claude is the static list.
    let models = client
        .list_provider_models(ProviderId::Claude)
        .await
        .unwrap();
    assert_eq!(
        models.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
        ["opus", "sonnet", "haiku"]
    );

    exit_on_terminate(&backend);
    stop.cancel();
    tokio::time::timeout(Duration::from_secs(8), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}

// ── Overview (P6) ───────────────────────────────────────────────────────────

fn registry_entry(
    cfg: &std::path::Path,
    pid: u32,
    session: &str,
    status: &str,
    waiting: Option<&str>,
) {
    let mut v = serde_json::json!({"pid": pid, "sessionId": session, "status": status});
    if let Some(w) = waiting {
        v["waitingFor"] = serde_json::json!(w);
    }
    write_file(
        &cfg.join("sessions").join(format!("{pid}.json")),
        &v.to_string(),
    );
}

#[tokio::test]
async fn overview_snapshot_events_history_and_details() {
    let h = start().await;
    let client = DaemonClient::connect(&h.paths).await.unwrap();
    let mut events = client.take_events().unwrap();
    client.hello().await.unwrap();
    assert!(client.get_overview().await.unwrap().sessions.is_empty());

    let agent = client.create_agent(payload()).await.unwrap();
    // Pushed only when something changed: the new agent shows up in a snapshot.
    let Event::OverviewSnapshot(snapshot) = next_event(&mut events, "overview snapshot", |e| {
        matches!(e, Event::OverviewSnapshot(s) if s.sessions.iter().any(|x| x.agent_id == agent.id))
    })
    .await
    else {
        unreachable!()
    };
    assert_eq!(snapshot.sessions[0].name, "demo");
    assert_eq!(snapshot.counts.total, 1);
    let polled = client.get_overview().await.unwrap();
    assert_eq!(polled.sessions.len(), 1);

    // The agent exits: an event is pushed and lands in the history.
    h.backend.ptys()[0].emit_exit(0);
    let Event::OverviewEvents(evs) = next_event(
        &mut events,
        "overview events",
        |e| matches!(e, Event::OverviewEvents(v) if v.iter().any(|x| x.agent_id == agent.id)),
    )
    .await
    else {
        unreachable!()
    };
    assert!(
        evs.iter()
            .any(|e| e.to == Some(as_core::overview::OverviewStatus::Finished))
    );
    let history = client.get_overview_history(Some(0)).await.unwrap();
    assert!(!history.points.is_empty(), "points are sampled");
    assert_eq!(history.starts.len(), 1);
    let listed = client.get_overview_events(Some(0), Some(1)).await.unwrap();
    assert_eq!(listed.len(), 1);
    assert!(client.get_overview_waits().await.unwrap().spans.len() <= 1);

    let details = client.get_agent_details(&agent.id).await.unwrap().unwrap();
    assert!(matches!(
        details.last_message,
        as_core::overview::LastMessage::Unavailable { .. }
    ));
    assert!(client.get_agent_details("nope").await.unwrap().is_none());

    h.stop.cancel();
    h.task.await.unwrap().unwrap();
    // The history is on disk, private to the data root.
    let files: Vec<_> = std::fs::read_dir(h.paths.overview_history_dir())
        .unwrap()
        .filter_map(Result::ok)
        .collect();
    assert_eq!(files.len(), 1);
    let text = std::fs::read_to_string(files[0].path()).unwrap();
    assert!(text.starts_with("{\"start\":"), "{text}");
    assert!(text.contains("\"point\""), "{text}");
}

#[tokio::test]
async fn a_waiting_claude_registry_entry_notifies_needs_you_once_with_what_it_asks() {
    let cfg = tempfile::tempdir().unwrap();
    let transcripts = cfg.path().join("projects/p");
    write_file(
        &transcripts.join("sess-1.jsonl"),
        &serde_json::json!({"type":"assistant","message":{"id":"m1","content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"ls"}}]}}).to_string(),
    );
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::at(dir.path());
    let backend = FakePtyBackend::new();
    let mut opts = options(&backend);
    opts.overview_config_dirs = Some(vec![cfg.path().to_path_buf()]);
    let stop = CancellationToken::new();
    let task = tokio::spawn(serve(paths.clone(), opts, stop.clone()));
    wait_for_socket(&paths).await;
    let client = DaemonClient::connect(&paths).await.unwrap();
    let mut events = client.take_events().unwrap();
    client.hello().await.unwrap();

    let agent = client
        .create_agent(CreateAgentPayload {
            resume_session_id: Some("sess-1".into()),
            ..payload()
        })
        .await
        .unwrap();
    // The agent is tracked (idle: no output yet) before it starts waiting.
    next_event(&mut events, "agent in the snapshot", |e| {
        matches!(e, Event::OverviewSnapshot(s) if s.sessions.iter().any(|x| x.agent_id == agent.id))
    })
    .await;
    // The registry entry's pid must be alive: use this test process.
    registry_entry(
        cfg.path(),
        std::process::id(),
        "sess-1",
        "waiting",
        Some("permission prompt"),
    );

    let Event::Notification(n) = next_event(
        &mut events,
        "needs-you notification",
        |e| matches!(e, Event::Notification(n) if n.title == "Needs you"),
    )
    .await
    else {
        unreachable!()
    };
    assert_eq!(n.kind, NotificationKind::AgentWaiting);
    assert_eq!(n.agent_id.as_deref(), Some(agent.id.as_str()));
    assert_eq!(n.body, "demo needs permission: Run: ls");
    let snapshot = client.get_overview().await.unwrap();
    assert_eq!(snapshot.counts.needs_you, 1);
    assert_eq!(
        snapshot.sessions[0].waiting_text.as_deref(),
        Some("Run: ls")
    );
    stop.cancel();
    task.await.unwrap().unwrap();
}

#[tokio::test]
async fn the_silence_rule_is_kept_for_unmatched_agents_and_skipped_for_matched_claude_agents() {
    use as_core::AgentManager;
    let cfg = tempfile::tempdir().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::at(dir.path());
    paths.ensure().unwrap();
    let backend = FakePtyBackend::new();
    let mut opts = options(&backend);
    opts.overview_config_dirs = Some(vec![cfg.path().to_path_buf()]);
    let daemon = as_daemon::Daemon::start(paths, opts).await;
    let (tx, mut rx) = tokio::sync::mpsc::channel(64);
    daemon.registry.add(tx, CancellationToken::new());

    let matched = daemon
        .manager
        .create(CreateAgentPayload {
            name: "matched".into(),
            resume_session_id: Some("s-match".into()),
            ..payload()
        })
        .await
        .unwrap();
    let unmatched = daemon
        .manager
        .create(CreateAgentPayload {
            name: "unmatched".into(),
            provider: ProviderId::Codex,
            model: "gpt-5.4".into(),
            ..payload()
        })
        .await
        .unwrap();
    registry_entry(cfg.path(), std::process::id(), "s-match", "busy", None);
    daemon.manager.get(&matched.id).unwrap();
    // The tracker sees the registry entry on its next poll.
    for _ in 0..100 {
        if daemon.overview.lock().has_registry_match(&matched.id) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(daemon.overview.lock().has_registry_match(&matched.id));

    // Both agents write something, then go quiet for longer than 5 s.
    for pty in backend.ptys() {
        pty.emit_data(b"hello");
    }
    tokio::time::sleep(Duration::from_millis(200)).await;
    daemon
        .manager
        .poll_activity_at(u64::try_from(chrono::Utc::now().timestamp_millis()).unwrap() + 60_000);

    let mut waiting_bodies = Vec::new();
    let mut flagged = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_millis(800);
    while let Ok(Some(msg)) = tokio::time::timeout_at(deadline, rx.recv()).await {
        if let ServerMsg::Event(Event::Notification(n)) = &msg
            && n.kind == NotificationKind::AgentWaiting
        {
            waiting_bodies.push(n.body.clone());
        }
        if let ServerMsg::Event(Event::AgentWaiting { agent_id }) = msg {
            flagged.push(agent_id);
        }
    }
    assert_eq!(waiting_bodies, ["unmatched is waiting for input"]);
    // The sidebar flag is still sent for both (Hydra keeps `agent_waiting` for the UI).
    assert!(
        flagged.contains(&matched.id) && flagged.contains(&unmatched.id),
        "{flagged:?}"
    );
    let _: &AgentManager = &daemon.manager;
    daemon.shutdown_services(true).await;
}

#[tokio::test]
async fn mcp_status_manager_agent_and_headless_runs() {
    let h = start().await;
    let client = DaemonClient::connect(&h.paths).await.unwrap();
    let mut events = client.take_events().unwrap();

    // MCP server + manager workspace.
    let status = client.get_mcp_status().await.unwrap();
    assert!(status.running && status.port.is_some() && status.error.is_none());
    let workspace = status.manager_workspace.unwrap();
    assert_eq!(
        std::path::Path::new(&workspace),
        h.paths.manager_workspace_dir()
    );
    assert!(std::path::Path::new(&workspace).join(".mcp.json").exists());
    assert!(std::path::Path::new(&workspace).join("CLAUDE.md").exists());

    // A manager agent lives in the workspace and is always Claude.
    let manager = client
        .create_agent(CreateAgentPayload {
            name: "boss".into(),
            project_dir: "/ignored".into(),
            provider: ProviderId::Codex,
            model: "opus".into(),
            is_manager: true,
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(manager.is_manager);
    assert_eq!(manager.provider, ProviderId::Claude);
    assert_eq!(manager.project_dir, workspace);

    // Headless runs: start, events, completion notification, log, listing.
    let work = tempfile::tempdir().unwrap();
    let run = client
        .start_headless_run(as_core::headless::StartHeadlessRunPayload {
            prompt: "say ok".into(),
            project_dir: work.path().to_string_lossy().into_owned(),
            model: "sonnet".into(),
            ..Default::default()
        })
        .await
        .unwrap();
    next_event(&mut events, "headless output", |e| {
        matches!(e, Event::HeadlessRunEvent { run_id, data } if *run_id == run.id && data.contains("sess-9"))
    })
    .await;
    next_event(
        &mut events,
        "completion notification",
        |e| matches!(e, Event::Notification(n) if n.kind == NotificationKind::HeadlessCompleted),
    )
    .await;
    let done = client.get_headless_run(&run.id).await.unwrap().unwrap();
    assert_eq!(done.session_id.as_deref(), Some("sess-9"));
    let log = client
        .get_headless_run_log(&run.id, Default::default())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(log.total_lines, 2);
    let listed = client
        .list_headless_runs(as_core::headless::ListHeadlessRunsOptions {
            query: Some("say ok".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(listed.len(), 1);
    assert!(!client.cancel_headless_run(&run.id).await.unwrap());
    assert!(
        client
            .start_headless_run(as_core::headless::StartHeadlessRunPayload {
                prompt: "x".into(),
                project_dir: "/definitely/not/a/dir".into(),
                model: "sonnet".into(),
                ..Default::default()
            })
            .await
            .is_err()
    );
}
