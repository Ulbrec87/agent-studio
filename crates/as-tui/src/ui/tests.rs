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
        app.modal = None;
        app.exec(crate::keymap::Command::ResumeSession);
        feed(&mut app, ProviderId::Claude, Ok(sample_sessions()));
        draw(&mut app, w, h);
        app.modal = None;
        app.exec(crate::keymap::Command::SetProfile);
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

// ── sessions picker, profiles (P4) ──────────────────────────────────────────

use as_core::sessions::{ClaudeProfile, SessionSummary};

const NOW_MS: i64 = 1_773_144_000_000; // 2026-03-10T12:00:00Z

fn session(
    id: &str,
    project: &str,
    prompt: &str,
    branch: Option<&str>,
    msgs: u32,
    modified: &str,
) -> SessionSummary {
    SessionSummary {
        session_id: id.into(),
        provider: ProviderId::Claude,
        project_path: project.into(),
        first_prompt: prompt.into(),
        message_count: msgs,
        modified_at: modified.into(),
        git_branch: branch.map(Into::into),
        ..Default::default()
    }
}

fn sample_sessions() -> Vec<SessionSummary> {
    vec![
        session(
            "s1",
            "/work/api",
            "Fix the login form validation and add tests for the empty password case",
            Some("fix/login"),
            42,
            "2026-03-10T11:55:00Z",
        ),
        session(
            "s2",
            "/work/web/some/very/deeply/nested/project/dir",
            "write docs\nfor billing",
            Some("main"),
            7,
            "2026-03-10T09:00:00Z",
        ),
        SessionSummary {
            hidden: true,
            ..session(
                "s3",
                "/work/api",
                "an old hidden one",
                None,
                3,
                "2026-03-08T09:00:00Z",
            )
        },
        session(
            "s4",
            "/work/old",
            "ancient refactor",
            Some("legacy"),
            120,
            "2026-01-02T09:00:00Z",
        ),
    ]
}

fn picker_app(w: u16, h: u16) -> App {
    let mut app = new_app(mixed_agents(), w, h);
    app.exec(crate::keymap::Command::ResumeSession);
    if let Some(Modal::Resume(p)) = &mut app.modal {
        p.now_ms = NOW_MS;
    }
    app
}

fn feed(app: &mut App, provider: ProviderId, result: Result<Vec<SessionSummary>, String>) {
    app.handle_msg(crate::app::Msg::Sessions { provider, result });
}

#[test]
fn session_picker_loading_then_list() {
    let mut app = picker_app(110, 26);
    let t = draw(&mut app, 110, 26);
    insta::assert_snapshot!("session_picker_loading", text(&t));
    feed(&mut app, ProviderId::Claude, Ok(sample_sessions()));
    let t = draw(&mut app, 110, 26);
    insta::assert_snapshot!("session_picker_list", text(&t));
}

#[test]
fn session_picker_toggles_and_filter() {
    let mut app = picker_app(110, 26);
    feed(&mut app, ProviderId::Claude, Ok(sample_sessions()));
    // all ages + hidden shown
    app.handle_key(key(KeyCode::Char('a')));
    app.handle_key(key(KeyCode::Char('s')));
    app.handle_key(key(KeyCode::Down));
    let t = draw(&mut app, 110, 26);
    insta::assert_snapshot!("session_picker_all_ages_hidden", text(&t));
    // fuzzy filter
    app.handle_key(key(KeyCode::Char('/')));
    for c in "lgn".chars() {
        app.handle_key(key(KeyCode::Char(c)));
    }
    let t = draw(&mut app, 110, 26);
    insta::assert_snapshot!("session_picker_filter", text(&t));
}

#[test]
fn session_picker_empty_and_error_states() {
    let mut app = picker_app(100, 20);
    feed(&mut app, ProviderId::Claude, Ok(Vec::new()));
    let t = draw(&mut app, 100, 20);
    assert!(text(&t).contains("No Claude sessions found."));
    feed(&mut app, ProviderId::Claude, Err("daemon exploded".into()));
    let t = draw(&mut app, 100, 20);
    assert!(text(&t).contains("daemon exploded"));
    // Filters hide everything: the hint says why.
    feed(&mut app, ProviderId::Claude, Ok(sample_sessions()));
    app.handle_key(key(KeyCode::Char('/')));
    for c in "zzzzqq".chars() {
        app.handle_key(key(KeyCode::Char(c)));
    }
    let t = draw(&mut app, 100, 20);
    assert!(text(&t).contains("No session matches the filters"));
}

#[test]
fn session_picker_inside_the_new_agent_dialog_and_resume_row() {
    let mut app = new_app(mixed_agents(), 110, 40);
    app.exec(crate::keymap::Command::NewAgent);
    if let Some(Modal::NewAgent(form)) = &mut app.modal {
        form.field = crate::new_agent::Field::Resume;
    }
    app.handle_key(key(KeyCode::Char(' ')));
    if let Some(Modal::NewAgent(form)) = &mut app.modal
        && let Some(p) = &mut form.picker
    {
        p.now_ms = NOW_MS;
    }
    feed(&mut app, ProviderId::Claude, Ok(sample_sessions()));
    let t = draw(&mut app, 110, 40);
    insta::assert_snapshot!("session_picker_in_dialog", text(&t));
    app.handle_key(key(KeyCode::Enter));
    // The picked session is shown on the Resume row ("now": the fixture is stamped at pick time).
    if let Some(Modal::NewAgent(form)) = &mut app.modal
        && let Some(s) = &mut form.resume
    {
        s.modified_at = chrono::Utc::now().to_rfc3339();
    }
    let t = draw(&mut app, 110, 40);
    insta::assert_snapshot!("dialog_with_resumed_session", text(&t));
}

#[test]
fn new_agent_dialog_with_profiles() {
    let mut app = new_app(mixed_agents(), 100, 40);
    app.handle_msg(crate::app::Msg::Profiles(vec![
        ClaudeProfile {
            config_dir: "/h/.claude".into(),
            label: "default".into(),
            is_default: true,
        },
        ClaudeProfile {
            config_dir: "/h/.claude-work".into(),
            label: "work".into(),
            is_default: false,
        },
    ]));
    app.config
        .claude_profile_by_project
        .insert("/work/api".into(), "/h/.claude-work".into());
    app.exec(crate::keymap::Command::NewAgent);
    if let Some(Modal::NewAgent(form)) = &mut app.modal {
        form.dir.set_text("/work/api/sub");
    }
    // The daemon's answer arrives after the dialog opened: the mapping picks the profile.
    app.handle_msg(crate::app::Msg::Profiles(vec![
        ClaudeProfile {
            config_dir: "/h/.claude".into(),
            label: "default".into(),
            is_default: true,
        },
        ClaudeProfile {
            config_dir: "/h/.claude-work".into(),
            label: "work".into(),
            is_default: false,
        },
    ]));
    let t = draw(&mut app, 100, 40);
    let shown = text(&t);
    assert!(shown.contains("Profile"), "{shown}");
    assert!(shown.contains("work (/h/.claude-work)"), "{shown}");
    insta::assert_snapshot!("new_agent_dialog_profiles", shown);
}

#[test]
fn profile_chooser_and_sidebar_chip() {
    let mut app = new_app(mixed_agents(), 100, 24);
    app.handle_msg(crate::app::Msg::Profiles(vec![
        ClaudeProfile {
            config_dir: "/h/.claude".into(),
            label: "default".into(),
            is_default: true,
        },
        ClaudeProfile {
            config_dir: "/h/.claude-work".into(),
            label: "work".into(),
            is_default: false,
        },
    ]));
    app.config
        .claude_profile_by_project
        .insert("/work/api".into(), "/h/.claude-work".into());
    app.config
        .claude_profile_by_project
        .insert("/work/web".into(), String::new());
    let t = draw(&mut app, 100, 24);
    let shown = text(&t);
    assert!(shown.contains("/work/api [work]"), "{shown}");
    assert!(!shown.contains("/work/web ["), "{shown}");
    app.exec(crate::keymap::Command::SetProfile);
    let t = draw(&mut app, 100, 24);
    insta::assert_snapshot!("profile_chooser", text(&t));
}

// ── P5: grid, palette, help, overview ───────────────────────────────────────────────────

fn grid_agents(n: usize) -> Vec<AgentState> {
    (0..n)
        .map(|i| {
            let status = match i % 4 {
                0 | 1 => AgentStatus::Running,
                2 => AgentStatus::Idle,
                _ => AgentStatus::Errored,
            };
            let mut a = agent(
                &format!("t{i}"),
                &format!("task-{i}"),
                "/work/api",
                status,
                1 + u32::try_from(i).unwrap_or(0),
            );
            a.config.yolo = i == 1;
            a
        })
        .collect()
}

/// Give every attached terminal some Claude-like output.
fn feed_all(app: &mut App) {
    let ids: Vec<String> = app.terms.keys().cloned().collect();
    for id in ids {
        let name = app.agent(&id).map(|a| a.name.clone()).unwrap_or_default();
        let term = app.terms.get_mut(&id).unwrap();
        term.sub = SubState::Live;
        let mut out = String::new();
        for line in 0..40 {
            out.push_str(&format!("{name}: line {line}\r\n"));
        }
        out.push_str("\x1b[1m> run the tests\x1b[0m\r\n\x1b[2m  ? for shortcuts\x1b[0m");
        term.emu.process(out.as_bytes());
    }
}

/// An app without the age limit of the grid (the fixtures are dated, the clock must not matter).
fn timeless_app(agents: Vec<AgentState>, w: u16, h: u16) -> App {
    let config = AppConfig {
        session_max_age_days: 0,
        ..AppConfig::default()
    };
    let mut app = App::new(config, agents, ColorMode::TrueColor, PathBuf::from("/tmp"));
    app.on_resize(w, h);
    app.start();
    app
}

fn grid_screen(n: usize, w: u16, h: u16) -> (App, Terminal<TestBackend>) {
    let mut app = timeless_app(grid_agents(n), w, h);
    app.exec(crate::keymap::Command::ToggleView);
    feed_all(&mut app);
    let t = draw(&mut app, w, h);
    (app, t)
}

#[test]
fn grid_with_two_tiles() {
    let (_, t) = grid_screen(2, 130, 30);
    insta::assert_snapshot!(text(&t));
}

#[test]
fn grid_with_four_tiles() {
    let (_, t) = grid_screen(4, 130, 36);
    insta::assert_snapshot!(text(&t));
}

#[test]
fn grid_with_seven_tiles() {
    let (app, t) = grid_screen(7, 130, 36);
    assert_eq!(app.grid.visible.len(), 7);
    insta::assert_snapshot!(text(&t));
}

#[test]
fn grid_with_an_expanded_tile() {
    let (mut app, _) = grid_screen(4, 130, 30);
    app.handle_key(key(KeyCode::Char('l')));
    app.handle_key(key(KeyCode::Char('z')));
    feed_all(&mut app);
    let t = draw(&mut app, 130, 30);
    insta::assert_snapshot!(text(&t));
}

#[test]
fn grid_running_tab_shows_the_project_of_every_agent() {
    let mut list = grid_agents(3);
    let mut web = agent("w1", "web-ui", "/work/web", AgentStatus::Running, 20);
    web.started_at = Some("2026-11-04T10:00:00Z".into());
    list.push(web);
    let mut app = timeless_app(list, 130, 30);
    app.exec(crate::keymap::Command::ToggleView);
    app.handle_key(key(KeyCode::Char('[')));
    assert_eq!(app.grid.tab, crate::grid::RUNNING_TAB);
    feed_all(&mut app);
    let t = draw(&mut app, 130, 30);
    insta::assert_snapshot!(text(&t));
}

#[test]
fn grid_attached_tile_shows_the_typing_marker_and_the_cursor() {
    let (mut app, _) = grid_screen(2, 130, 30);
    app.handle_key(key(KeyCode::Enter));
    let t = draw(&mut app, 130, 30);
    let screen = text(&t);
    assert!(screen.contains("typing"));
    assert!(screen.contains("TILE"));
    let pos = t.backend().cursor_position();
    let tile = app.grid_layout().tiles[0].rect;
    assert!(tile.contains(pos), "cursor {pos:?} outside {tile:?}");
}

#[test]
fn grid_tile_shows_the_bottom_of_the_agent_screen() {
    // The agent's screen is much taller than the tile: the prompt at the bottom stays visible.
    let (_, t) = grid_screen(1, 130, 24);
    let screen = text(&t);
    assert!(screen.contains("> run the tests"), "{screen}");
    assert!(screen.contains("? for shortcuts"));
    assert!(
        !screen.contains("task-0: line 0\n"),
        "the top scrolls out of the window"
    );
}

#[test]
fn grid_empty_state_and_broadcast_bar() {
    let mut app = new_app(Vec::new(), 100, 24);
    app.exec(crate::keymap::Command::ToggleView);
    let t = draw(&mut app, 100, 24);
    assert!(text(&t).contains("No agents yet"));

    let (mut app, _) = grid_screen(3, 130, 30);
    app.handle_key(key(KeyCode::Char('b')));
    for c in "ship it".chars() {
        app.handle_key(key(KeyCode::Char(c)));
    }
    let t = draw(&mut app, 130, 30);
    insta::assert_snapshot!(text(&t));
}

#[test]
fn grid_survives_tiny_screens() {
    for (w, h) in [(1, 1), (20, 5), (60, 8), (61, 12), (90, 14)] {
        let (mut app, _) = grid_screen(5, 130, 40);
        app.on_resize(w, h);
        app.handle_key(key(KeyCode::Char('b')));
        app.handle_key(key(KeyCode::Char('z')));
        let _ = draw(&mut app, w, h);
    }
}

#[test]
fn command_palette_snapshots() {
    let mut app = new_app(mixed_agents(), 110, 32);
    app.exec(crate::keymap::Command::CommandPalette);
    let t = draw(&mut app, 110, 32);
    insta::assert_snapshot!("palette_all", text(&t));
    for c in "grid".chars() {
        app.handle_key(key(KeyCode::Char(c)));
    }
    let t = draw(&mut app, 110, 32);
    insta::assert_snapshot!("palette_filtered", text(&t));
}

#[test]
fn help_overlay_is_grouped_by_category_and_scrolls() {
    let mut app = new_app(mixed_agents(), 110, 36);
    app.exec(crate::keymap::Command::Help);
    let t = draw(&mut app, 110, 36);
    let first = text(&t);
    assert!(
        first.contains("Navigation") && first.contains("Ctrl+A g"),
        "{first}"
    );
    insta::assert_snapshot!("help_overlay_top", first);
    for _ in 0..30 {
        app.handle_key(key(KeyCode::Char('j')));
    }
    let t = draw(&mut app, 110, 36);
    let scrolled = text(&t);
    assert_ne!(first, scrolled);
    insta::assert_snapshot!("help_overlay_scrolled", scrolled);
    // Far past the end it stops at the last line.
    app.handle_key(key(KeyCode::Char('G')));
    let _ = draw(&mut app, 110, 36);
    let end = app.help_scroll;
    let _ = draw(&mut app, 110, 36);
    assert_eq!(app.help_scroll, end);
}

#[test]
fn help_overlay_shows_user_overrides_and_unbound_commands() {
    let mut app = new_app(mixed_agents(), 110, 60);
    app.apply_keybindings(
        Some(r#"[{"command":"new-agent","keys":"alt+n"},{"command":"quit","keys":"none"}]"#),
        false,
    );
    app.exec(crate::keymap::Command::Help);
    let screen = text(&draw(&mut app, 110, 60));
    assert!(screen.contains("Alt+n"), "{screen}");
    app.handle_key(key(KeyCode::Char('G')));
    let screen = text(&draw(&mut app, 110, 60));
    assert!(screen.contains("(unbound)"), "{screen}");
}

#[test]
fn header_tabs_and_the_overview_waiting_for_the_daemon() {
    let mut app = new_app(mixed_agents(), 100, 20);
    let header = text(&draw(&mut app, 100, 20));
    assert!(
        header
            .lines()
            .next()
            .unwrap()
            .contains("Chat  Grid  Overview")
    );
    app.exec(crate::keymap::Command::ShowOverview);
    let screen = text(&draw(&mut app, 100, 20));
    assert!(screen.contains("Loading overview"), "{screen}");
}

#[test]
fn sidebar_chips_and_sort_label() {
    let mut app = new_app(mixed_agents(), 100, 24);
    app.exec(crate::keymap::Command::CycleSidebarStatus);
    app.exec(crate::keymap::Command::CycleSidebarSort);
    let t = draw(&mut app, 100, 24);
    let screen = text(&t);
    assert!(screen.contains("All Run Wait Idle Err"), "{screen}");
    assert!(screen.contains("\u{2195}recent"));
    // The active chip is highlighted.
    let buf = t.backend().buffer();
    let accent = app.theme.accent;
    let hit = (0..buf.area.height)
        .any(|y| (0..28).any(|x| buf[(x, y)].symbol() == "R" && buf[(x, y)].bg == accent));
    assert!(hit, "the Run chip is highlighted");
}

// ── P10: footer hints follow the bindings, toasts wrap, the update banner ───

fn footer_hints(app: &App) -> (&'static str, String) {
    super::footer::mode_and_hints(app)
}

#[test]
fn footer_hints_show_the_default_prefix_keys() {
    let mut app = new_app(mixed_agents(), 100, 30);
    let (mode, hints) = footer_hints(&app);
    assert_eq!(mode, "TERMINAL");
    assert!(hints.contains("Ctrl+A ? help"), "{hints}");
    assert!(hints.contains("Ctrl+A q quit"), "{hints}");
    app.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL));
    let (mode, hints) = footer_hints(&app);
    assert_eq!(mode, "PREFIX");
    assert!(
        hints.starts_with("n new \u{b7} g grid \u{b7} p palette"),
        "{hints}"
    );
    assert!(hints.contains("j/k agents"), "{hints}");
    assert!(hints.contains("1-9 jump"), "{hints}");
}

#[test]
fn footer_hints_follow_user_overrides_and_unbinding() {
    let mut app = new_app(mixed_agents(), 100, 30);
    app.apply_keybindings(
        Some(
            r#"[
            {"command": "new-agent", "keys": "prefix w"},
            {"command": "quit", "keys": "none"},
            {"command": "show-shortcuts", "keys": "prefix h"}
        ]"#,
        ),
        false,
    );
    let (_, hints) = footer_hints(&app);
    assert!(hints.contains("Ctrl+A h help"), "{hints}");
    assert!(hints.contains("Ctrl+A w new"), "{hints}");
    assert!(
        !hints.contains("quit"),
        "an unbound command has no hint: {hints}"
    );
    app.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL));
    let (_, hints) = footer_hints(&app);
    assert!(hints.contains("w new"), "{hints}");
    assert!(!hints.contains("n new"), "{hints}");
    assert!(hints.contains("h help"), "{hints}");
    assert!(!hints.contains("q quit"), "{hints}");
    // The label follows a changed prefix key too.
    let mut app = new_app(mixed_agents(), 100, 30);
    app.config.prefix_key = "ctrl+b".into();
    app.apply_keybindings(None, false);
    let (_, hints) = footer_hints(&app);
    assert!(hints.contains("Ctrl+B ? help"), "{hints}");
}

#[test]
fn footer_hints_of_the_grid_and_sidebar_follow_overrides() {
    let mut app = new_app(mixed_agents(), 100, 30);
    app.apply_keybindings(
        Some(r#"[{"command": "new-agent", "keys": "app N"}, {"command": "grid-open-chat", "keys": "grid x"}]"#),
        false,
    );
    app.focus = Focus::App;
    let (mode, hints) = footer_hints(&app);
    assert_eq!(mode, "APP");
    assert!(hints.contains("N new"), "{hints}");
    app.focus = Focus::Grid;
    let (mode, hints) = footer_hints(&app);
    assert_eq!(mode, "GRID");
    assert!(hints.contains("x chat"), "{hints}");
    assert!(hints.contains("hjkl move"), "{hints}");
}

#[test]
fn a_long_toast_wraps_instead_of_being_cut() {
    let mut app = new_app(mixed_agents(), 100, 30);
    app.toast(
        ToastKind::Warn,
        "Keybindings",
        "ctrl+a w is bound to both new-agent and quit; keeping new-agent. THE-END-MARKER",
    );
    let t = draw(&mut app, 100, 30);
    let screen = text(&t);
    assert!(screen.contains("THE-END-MARKER"), "{screen}");
    assert!(screen.contains("Keybindings"), "{screen}");
    // The old fixed-height toast showed one body line.
    let lines = screen.lines().filter(|l| l.contains('\u{2502}')).count();
    assert!(lines >= 3, "{screen}");
    // Many long toasts never draw outside the screen.
    for _ in 0..6 {
        app.toast(ToastKind::Error, "Error", &"very long text ".repeat(30));
    }
    let _ = draw(&mut app, 100, 30);
    app.on_resize(50, 12);
    let _ = draw(&mut app, 50, 12);
}

#[test]
fn the_update_banner_is_drawn_until_dismissed() {
    use as_core::updates::{CheckStatus, InstallMethod, UpdateInfo, UpdateState};
    let mut app = new_app(mixed_agents(), 100, 30);
    app.handle_event(Event::UpdateAvailable(Box::new(UpdateState {
        enabled: true,
        current_version: "0.1.0".into(),
        status: CheckStatus::Available,
        checked_at_ms: Some(1),
        latest: Some(UpdateInfo {
            version: "0.2.0".into(),
            tag: "v0.2.0".into(),
            url: "https://example.test/r".into(),
            notes: String::new(),
            asset_url: None,
        }),
        install_method: InstallMethod::Homebrew,
        hint: "Run `brew upgrade agent-studio`".into(),
    })));
    let screen = text(&draw(&mut app, 100, 30));
    assert!(screen.contains("Update available: 0.2.0"), "{screen}");
    assert!(screen.contains("brew upgrade agent-studio"), "{screen}");
    app.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL));
    app.handle_key(key(KeyCode::Esc));
    let screen = text(&draw(&mut app, 100, 30));
    assert!(!screen.contains("Update available"), "{screen}");
}
