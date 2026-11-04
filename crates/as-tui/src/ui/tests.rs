//! Render snapshots on ratatui's `TestBackend` (text only; colors are asserted separately).

use std::path::PathBuf;

use as_core::{AgentState, AgentStatus, AppConfig, ProviderId};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::{App, Focus, Modal, SubState, ToastKind};
use crate::theme::ColorMode;
use as_proto::Event;

fn agent(id: &str, name: &str, dir: &str, status: AgentStatus, day: u32) -> AgentState {
    let mut a = AgentState::default();
    a.config.id = id.into();
    a.config.name = name.into();
    a.config.project_dir = dir.into();
    a.config.provider = ProviderId::Claude;
    a.config.model = "sonnet".into();
    a.config.created_at = format!("2026-01-{day:02}T00:00:00Z");
    a.status = status;
    a
}

fn new_app(agents: Vec<AgentState>, w: u16, h: u16) -> App {
    let mut app = App::new(
        AppConfig::default(),
        agents,
        ColorMode::TrueColor,
        PathBuf::from("/tmp"),
    );
    app.on_resize(w, h);
    app.start();
    app
}

fn draw(app: &mut App, w: u16, h: u16) -> Terminal<TestBackend> {
    let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
    terminal.draw(|f| super::draw(f, app)).unwrap();
    terminal
}

fn text(terminal: &Terminal<TestBackend>) -> String {
    let buf = terminal.backend().buffer();
    let area = buf.area;
    (0..area.height)
        .map(|y| {
            let line: String = (0..area.width)
                .map(|x| buf[(x, y)].symbol().to_string())
                .collect();
            line.trim_end().to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn mixed_agents() -> Vec<AgentState> {
    let mut manager = agent("m1", "manager", "/work/hub", AgentStatus::Running, 1);
    manager.config.is_manager = true;
    let mut yolo = agent("a4", "tests", "/work/api", AgentStatus::Running, 5);
    yolo.config.yolo = true;
    vec![
        manager,
        agent("a1", "api-refactor", "/work/api", AgentStatus::Running, 3),
        yolo,
        agent("a5", "migrations", "/work/api", AgentStatus::Idle, 6),
        agent("w1", "ui-fix", "/work/web", AgentStatus::Errored, 2),
        agent("w2", "boot", "/work/web", AgentStatus::Starting, 4),
    ]
}

#[test]
fn empty_state() {
    let mut app = new_app(Vec::new(), 80, 16);
    let t = draw(&mut app, 80, 16);
    insta::assert_snapshot!(text(&t));
}

#[test]
fn sidebar_with_agents_in_several_states() {
    let mut app = new_app(mixed_agents(), 90, 18);
    app.handle_event(Event::AgentWaiting {
        agent_id: "a4".into(),
    });
    app.select("a1");
    app.terms.get_mut("a1").unwrap().sub = SubState::Live;
    app.terms
        .get_mut("a1")
        .unwrap()
        .emu
        .process(b"\x1b[1mHello\x1b[0m from the agent\r\n> ");
    let t = draw(&mut app, 90, 18);
    insta::assert_snapshot!(text(&t));
}

#[test]
fn status_glyphs_use_the_theme_colors() {
    let mut app = new_app(mixed_agents(), 90, 18);
    app.handle_event(Event::AgentWaiting {
        agent_id: "a4".into(),
    });
    let t = draw(&mut app, 90, 18);
    let buf = t.backend().buffer();
    let find = |glyph: &str| {
        let mut found = None;
        for y in 0..buf.area.height {
            for x in 0..28 {
                if buf[(x, y)].symbol() == glyph {
                    found = Some(buf[(x, y)].fg);
                }
            }
        }
        found.unwrap_or_else(|| panic!("no {glyph} in the sidebar"))
    };
    let theme = app.theme;
    assert_eq!(find("\u{25CF}"), theme.running);
    assert_eq!(find("\u{25D0}"), theme.waiting);
    assert_eq!(find("\u{25CB}"), theme.idle);
    assert_eq!(find("\u{2716}"), theme.error);
    assert_eq!(find("\u{25CC}"), theme.starting);
}

#[test]
fn new_agent_dialog() {
    let mut app = new_app(mixed_agents(), 100, 36);
    app.exec(crate::keymap::Command::NewAgent);
    if let Some(Modal::NewAgent(form)) = &mut app.modal {
        form.name.set_text("my-agent");
        form.dir.set_text("/tmp");
    }
    app.handle_key(key(KeyCode::Down));
    let t = draw(&mut app, 100, 36);
    insta::assert_snapshot!(text(&t));
}

#[test]
fn new_agent_dialog_codex_with_error() {
    let mut app = new_app(Vec::new(), 100, 36);
    app.exec(crate::keymap::Command::NewAgent);
    if let Some(Modal::NewAgent(form)) = &mut app.modal {
        form.provider = ProviderId::Codex;
        form.model_idx = Some(0);
        form.effort_idx = 3;
        form.yolo = true;
        form.error = Some("codex CLI not found on PATH".into());
    }
    let t = draw(&mut app, 100, 36);
    insta::assert_snapshot!(text(&t));
}

#[test]
fn quit_dialog() {
    let mut app = new_app(mixed_agents(), 90, 24);
    app.request_quit();
    assert!(matches!(app.modal, Some(Modal::Quit { .. })));
    let t = draw(&mut app, 90, 24);
    insta::assert_snapshot!(text(&t));
}

#[test]
fn toasts_help_and_prefix_indicator() {
    let mut app = new_app(mixed_agents(), 100, 30);
    app.toast(
        ToastKind::Warn,
        "Agent Finished",
        "api-refactor is waiting for input",
    );
    app.toast(
        ToastKind::Error,
        "Agent Error",
        "ui-fix encountered an error",
    );
    app.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL));
    let t = draw(&mut app, 100, 30);
    insta::assert_snapshot!("toasts_and_prefix", text(&t));
    app.handle_key(key(KeyCode::Char('?')));
    let t = draw(&mut app, 100, 30);
    insta::assert_snapshot!("help", text(&t));
}

#[test]
fn input_bar_and_scroll_mode() {
    let mut app = new_app(mixed_agents(), 90, 20);
    app.exec(crate::keymap::Command::ToggleInputBar);
    assert_eq!(app.focus, Focus::Input);
    app.input.insert_str("run the tests");
    let t = draw(&mut app, 90, 20);
    insta::assert_snapshot!("input_bar", text(&t));
}

#[test]
fn survives_tiny_screens() {
    for (w, h) in [(1, 1), (10, 3), (30, 8), (59, 10), (60, 5)] {
        let mut app = new_app(mixed_agents(), w, h);
        app.exec(crate::keymap::Command::NewAgent);
        draw(&mut app, w, h);
        app.modal = Some(Modal::Help);
        draw(&mut app, w, h);
        app.request_quit();
        draw(&mut app, w, h);
    }
}

#[test]
fn disconnected_banner_and_waiting_badge() {
    let mut app = new_app(mixed_agents(), 120, 20);
    app.handle_event(Event::AgentWaiting {
        agent_id: "a1".into(),
    });
    app.on_disconnected();
    let t = draw(&mut app, 120, 20);
    let first_line = text(&t).lines().next().unwrap().to_string();
    assert!(first_line.contains("reconnecting"), "{first_line}");
    assert!(first_line.contains("1 waiting"), "{first_line}");
    assert!(
        first_line.contains("4/6 running") || first_line.contains("3/6 running"),
        "{first_line}"
    );
}
