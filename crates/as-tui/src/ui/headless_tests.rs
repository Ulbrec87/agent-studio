//! Headless runs panel, the manager agent dialog and the MCP status: snapshots and key flows on
//! a fixed clock.

use std::path::PathBuf;

use as_core::headless::{HeadlessRun, HeadlessRunLog, HeadlessRunStatus, StartHeadlessRunPayload};
use as_core::{AppConfig, ProviderId};
use as_proto::{Event, McpStatus};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::overview_tests::{NOW, draw, text};
use crate::app::{App, Effect, Modal, Msg};
use crate::overview::format::Clock;
use crate::theme::ColorMode;

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn press(app: &mut App, c: char) -> Vec<Effect> {
    app.handle_key(key(KeyCode::Char(c)))
}

fn prefix(app: &mut App, c: char) -> Vec<Effect> {
    app.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL));
    press(app, c)
}

fn iso(minutes_ago: i64) -> String {
    chrono::DateTime::from_timestamp_millis(NOW - minutes_ago * 60_000)
        .unwrap()
        .to_rfc3339()
}

fn run(
    id: &str,
    minutes_ago: i64,
    status: HeadlessRunStatus,
    dir: &str,
    prompt: &str,
) -> HeadlessRun {
    HeadlessRun {
        id: id.into(),
        prompt: prompt.into(),
        project_dir: dir.into(),
        provider: ProviderId::Claude,
        model: "sonnet".into(),
        status,
        started_at: iso(minutes_ago),
        ended_at: (status != HeadlessRunStatus::Running).then(|| iso(minutes_ago - 1)),
        session_id: Some("0123456789abcdef".into()),
        error: (status == HeadlessRunStatus::Errored).then(|| "CLI exited with code 1".into()),
        ..Default::default()
    }
}

fn runs() -> Vec<HeadlessRun> {
    vec![
        run(
            "r1aa",
            3,
            HeadlessRunStatus::Running,
            "/work/shop",
            "refactor the cart module and add tests",
        ),
        run(
            "r2bb",
            95,
            HeadlessRunStatus::Completed,
            "/work/web",
            "reply with the word ok",
        ),
        run(
            "r3cc",
            600,
            HeadlessRunStatus::Errored,
            "/work/docs",
            "summarize the changelog\nof the last release",
        ),
        run(
            "r4dd",
            3000,
            HeadlessRunStatus::Canceled,
            "/work/shop",
            "migrate the database",
        ),
    ]
}

fn app() -> App {
    let mut app = App::new(
        AppConfig::default(),
        Vec::new(),
        ColorMode::TrueColor,
        PathBuf::from("/tmp"),
    );
    app.on_resize(110, 30);
    app.start();
    app.overview.clock_override = Some(Clock {
        now: NOW,
        utc: true,
    });
    app
}

fn open_panel() -> App {
    let mut app = app();
    let fx = prefix(&mut app, 'H');
    assert_eq!(fx, vec![Effect::LoadHeadlessRuns]);
    assert!(matches!(app.modal, Some(Modal::Headless)));
    app.handle_msg(Msg::HeadlessRuns(Ok(runs())));
    app
}

const STREAM: &[&str] = &[
    r#"{"type":"system","subtype":"init","session_id":"0123456789abcdef","model":"sonnet","cwd":"/work/shop"}"#,
    r#"{"type":"assistant","message":{"content":[{"type":"text","text":"I will look at the cart module first."},{"type":"tool_use","name":"Read","input":{"file_path":"/work/shop/src/cart.rs"}}]}}"#,
    r#"{"type":"user","message":{"content":[{"type":"tool_result","content":"fn total() {}\nfn add() {}\nfn remove() {}"}]}}"#,
    r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Bash","input":{"command":"cargo test cart"}}]}}"#,
    r#"{"type":"stderr","text":"warning: unused variable"}"#,
    r#"{"type":"assistant","message":{"content":[{"type":"text","text":"All tests pass."}]}}"#,
    r#"{"type":"result","subtype":"success","is_error":false,"result":"All tests pass.","num_turns":3,"duration_ms":8200,"total_cost_usd":0.0421}"#,
];

fn open_log(app: &mut App) {
    let fx = app.handle_key(key(KeyCode::Enter));
    assert_eq!(fx, vec![Effect::LoadHeadlessLog("r1aa".into())]);
    app.handle_msg(Msg::HeadlessLog {
        run_id: "r1aa".into(),
        result: Ok(Some(HeadlessRunLog {
            run_id: "r1aa".into(),
            content: STREAM.join("\n"),
            total_lines: STREAM.len(),
            returned_lines: STREAM.len(),
            truncated: false,
        })),
    });
}

#[test]
fn the_run_list() {
    let mut app = open_panel();
    let t = draw(&mut app, 110, 30);
    insta::assert_snapshot!(text(&t));
}

#[test]
fn the_log_viewer_renders_stream_json() {
    let mut app = open_panel();
    open_log(&mut app);
    let t = draw(&mut app, 110, 30);
    insta::assert_snapshot!(text(&t));
}

#[test]
fn raw_toggle_and_following_live_lines() {
    let mut app = open_panel();
    app.on_resize(110, 16);
    open_log(&mut app);
    // The raw view shows the JSON as the daemon stored it.
    press(&mut app, 'r');
    let screen = text(&draw(&mut app, 110, 16));
    assert!(screen.contains(r#""type":"result""#), "{screen}");
    assert!(screen.contains("raw"), "{screen}");
    press(&mut app, 'r');

    // A live line is appended while following.
    app.handle_event(Event::HeadlessRunEvent {
        run_id: "r1aa".into(),
        data: r#"{"type":"assistant","message":{"content":[{"type":"text","text":"one more thing"}]}}"#.into(),
    });
    let screen = text(&draw(&mut app, 110, 16));
    assert!(screen.contains("one more thing"), "{screen}");
    assert!(screen.contains("following"), "{screen}");

    // Scrolled away: the view stays and says how to follow again.
    app.handle_key(key(KeyCode::Up));
    app.handle_event(Event::HeadlessRunEvent {
        run_id: "r1aa".into(),
        data: r#"{"type":"assistant","message":{"content":[{"type":"text","text":"later line"}]}}"#
            .into(),
    });
    let screen = text(&draw(&mut app, 110, 16));
    assert!(screen.contains("scrolled"), "{screen}");
    assert!(!screen.contains("later line"), "{screen}");
    press(&mut app, 'G');
    let screen = text(&draw(&mut app, 110, 16));
    assert!(screen.contains("later line"), "{screen}");

    // Lines of another run are ignored.
    app.handle_event(Event::HeadlessRunEvent {
        run_id: "other".into(),
        data: "intruder".into(),
    });
    assert!(!text(&draw(&mut app, 110, 16)).contains("intruder"));
}

#[test]
fn a_finished_run_reloads_its_log() {
    let mut app = open_panel();
    open_log(&mut app);
    // Still running: nothing to do.
    let still = runs().remove(0);
    assert!(
        app.handle_event(Event::HeadlessRunChanged(Box::new(still)))
            .is_empty()
    );
    // Ended: lines may have been missed, read the log once more.
    let mut done = runs().remove(0);
    done.status = HeadlessRunStatus::Completed;
    let fx = app.handle_event(Event::HeadlessRunChanged(Box::new(done)));
    assert_eq!(fx, vec![Effect::LoadHeadlessLog("r1aa".into())]);
    assert_eq!(
        app.headless.run("r1aa").unwrap().status,
        HeadlessRunStatus::Completed
    );
}

#[test]
fn search_and_status_filter() {
    let mut app = open_panel();
    press(&mut app, '/');
    for c in "shop".chars() {
        press(&mut app, c);
    }
    app.handle_key(key(KeyCode::Enter));
    let screen = text(&draw(&mut app, 110, 30));
    assert!(screen.contains("refactor the cart"), "{screen}");
    assert!(screen.contains("migrate the database"), "{screen}");
    assert!(!screen.contains("reply with the word ok"), "{screen}");
    assert!(screen.contains("2 of 4 runs"), "{screen}");
    // `s` walks the status chip: running first.
    press(&mut app, 's');
    let screen = text(&draw(&mut app, 110, 30));
    assert!(screen.contains("status: running"), "{screen}");
    assert!(screen.contains("1 of 4 runs"), "{screen}");
    // Esc in filter mode clears the search.
    press(&mut app, '/');
    app.handle_key(key(KeyCode::Esc));
    assert!(app.headless.query.is_empty());
}

#[test]
fn cancel_only_applies_to_running_runs() {
    let mut app = open_panel();
    assert_eq!(
        press(&mut app, 'x'),
        vec![Effect::CancelHeadlessRun("r1aa".into())]
    );
    press(&mut app, 'j');
    assert!(press(&mut app, 'x').is_empty());
    assert!(
        app.toasts.iter().any(|t| t.body.contains("not running")),
        "{:?}",
        app.toasts
    );
}

#[test]
fn a_new_run_goes_through_the_dialog_and_opens_its_log() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = open_panel();
    press(&mut app, 'n');
    let Some(Modal::NewAgent(form)) = &mut app.modal else {
        panic!("no dialog");
    };
    form.dir.set_text(dir.path().to_str().unwrap());
    // The prompt is required.
    app.handle_key(key(KeyCode::Enter));
    let Some(Modal::NewAgent(form)) = &app.modal else {
        panic!("dialog closed")
    };
    assert_eq!(form.error.as_deref(), Some("A prompt is required"));
    let screen = text(&draw(&mut app, 110, 30));
    assert!(screen.contains("New headless run"), "{screen}");
    assert!(!screen.contains("skip permission prompts"), "{screen}");
    assert!(!screen.contains("orchestrates"), "{screen}");

    for c in "say ok".chars() {
        press(&mut app, c);
    }
    let fx = app.handle_key(key(KeyCode::Enter));
    let [
        Effect::StartHeadlessRun(StartHeadlessRunPayload {
            prompt,
            project_dir,
            provider,
            model,
            ..
        }),
    ] = &fx[..]
    else {
        panic!("{fx:?}")
    };
    assert_eq!(prompt, "say ok");
    assert_eq!(project_dir, dir.path().to_str().unwrap());
    assert_eq!((*provider, model.as_str()), (ProviderId::Claude, "sonnet"));

    let new = run("r5ee", 0, HeadlessRunStatus::Running, project_dir, "say ok");
    let fx = app.handle_msg(Msg::HeadlessStarted(Ok(Box::new(new))));
    assert_eq!(fx, vec![Effect::LoadHeadlessLog("r5ee".into())]);
    assert!(matches!(app.modal, Some(Modal::Headless)));
    assert_eq!(app.headless.view.as_ref().unwrap().run_id, "r5ee");
    assert_eq!(app.headless.runs.len(), 5);
}

#[test]
fn a_failed_start_stays_in_the_dialog_and_esc_returns_to_the_panel() {
    let mut app = open_panel();
    press(&mut app, 'n');
    if let Some(Modal::NewAgent(form)) = &mut app.modal {
        form.submitting = true;
    }
    app.handle_msg(Msg::HeadlessStarted(Err("Claude CLI not found".into())));
    let Some(Modal::NewAgent(form)) = &app.modal else {
        panic!("dialog closed")
    };
    assert!(!form.submitting);
    assert_eq!(form.error.as_deref(), Some("Claude CLI not found"));
    app.handle_key(key(KeyCode::Esc));
    assert!(matches!(app.modal, Some(Modal::Headless)));
}

#[test]
fn esc_walks_back_log_then_list_then_closed() {
    let mut app = open_panel();
    open_log(&mut app);
    app.handle_key(key(KeyCode::Esc));
    assert!(app.headless.view.is_none());
    assert!(matches!(app.modal, Some(Modal::Headless)));
    app.handle_key(key(KeyCode::Esc));
    assert!(app.modal.is_none());
}

#[test]
fn the_palette_offers_headless_and_manager_commands() {
    let mut app = app();
    prefix(&mut app, 'p');
    let Some(Modal::Palette(p)) = &app.modal else {
        panic!("no palette")
    };
    let labels: Vec<_> = p.entries.iter().map(|e| e.label.as_str()).collect();
    for wanted in ["Headless Runs", "New Manager Agent", "MCP Server Status"] {
        assert!(labels.contains(&wanted), "{wanted} missing: {labels:?}");
    }
}

#[test]
fn manager_dialog_builds_a_manager_payload() {
    let mut app = app();
    app.handle_msg(Msg::McpStatus(Box::new(McpStatus {
        running: true,
        port: Some(4242),
        error: None,
        manager_workspace: Some("/home/me/.config/agent-studio/manager-workspace".into()),
    })));
    app.exec(crate::keymap::Command::NewManagerAgent);
    let screen = text(&draw(&mut app, 110, 30));
    assert!(screen.contains("New manager agent"), "{screen}");
    assert!(screen.contains("[x]"), "{screen}");
    assert!(!screen.contains("Provider"), "{screen}");
    assert!(!screen.contains("Resume"), "{screen}");
    let fx = app.handle_key(key(KeyCode::Enter));
    let [Effect::CreateAgent(payload)] = &fx[..] else {
        panic!("{fx:?}")
    };
    assert!(payload.is_manager);
    assert_eq!(payload.provider, ProviderId::Claude);
    assert!(
        payload.project_dir.is_empty(),
        "the daemon picks the workspace"
    );
    assert_eq!(payload.name, "Manager");
    assert!(payload.initial_prompt.contains("MCP tools"));
}

#[test]
fn manager_toggle_in_the_dialog_and_without_mcp() {
    let mut app = app();
    prefix(&mut app, 'n');
    // Walk to the Manager row and toggle it with Space.
    for _ in 0..5 {
        app.handle_key(key(KeyCode::Down));
    }
    press(&mut app, ' ');
    let Some(Modal::NewAgent(form)) = &app.modal else {
        panic!("no dialog")
    };
    assert!(form.manager, "{:?}", form.field);
    press(&mut app, ' ');
    let Some(Modal::NewAgent(form)) = &app.modal else {
        panic!("no dialog")
    };
    assert!(!form.manager);
    assert!(
        form.prompt.is_empty(),
        "the suggested prompt goes away with the toggle"
    );

    // MCP down: the toggle refuses and says why.
    app.handle_msg(Msg::McpStatus(Box::new(McpStatus {
        running: false,
        port: None,
        error: Some("address in use".into()),
        manager_workspace: None,
    })));
    press(&mut app, ' ');
    let Some(Modal::NewAgent(form)) = &app.modal else {
        panic!("no dialog")
    };
    assert!(!form.manager);
    assert!(form.error.as_deref().unwrap().contains("MCP"));
}

#[test]
fn mcp_status_in_the_help_overlay_and_as_a_toast() {
    let mut app = app();
    app.handle_msg(Msg::McpStatus(Box::new(McpStatus {
        running: true,
        port: Some(4242),
        error: None,
        manager_workspace: Some("/srv/manager-workspace".into()),
    })));
    let lines = crate::ui::dialogs::help_lines(&app);
    let all: String = lines
        .iter()
        .map(|l| {
            l.spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        all.contains("MCP server: running on 127.0.0.1:4242"),
        "{all}"
    );
    let fx = app.exec(crate::keymap::Command::McpStatus);
    assert_eq!(fx, vec![Effect::LoadMcpStatus]);
    assert!(app.toasts.iter().any(|t| t.body.contains("127.0.0.1:4242")));
}

#[test]
fn headless_notifications_become_toasts() {
    let mut app = app();
    app.handle_event(Event::Notification(as_proto::StudioNotification {
        id: "n1".into(),
        kind: as_proto::NotificationKind::HeadlessErrored,
        title: "Headless Run Error".into(),
        body: "Run r1aa failed: CLI exited with code 1".into(),
        agent_id: None,
        timestamp: "2026-10-22T14:30:00Z".into(),
    }));
    assert_eq!(app.toasts[0].kind, crate::app::ToastKind::Error);
}
