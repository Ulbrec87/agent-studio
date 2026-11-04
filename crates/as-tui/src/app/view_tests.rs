//! Views, grid navigation, broadcast, palette, keybinding reloads, persisted state.

use std::path::PathBuf;

use as_core::keybindings::KeybindingRule;
use as_core::{AgentState, AgentStatus, AppConfig, GridColumns, ProviderId, ViewMode};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::*;
use crate::grid::RUNNING_TAB;
use crate::sidebar::{SortKey, StatusChip};

/// No age limit: the agents of the fixtures are dated, the wall clock must not matter.
fn cfg() -> AppConfig {
    AppConfig {
        session_max_age_days: 0,
        ..AppConfig::default()
    }
}

fn agent(id: &str, dir: &str, status: AgentStatus, day: u32) -> AgentState {
    let mut a = AgentState::default();
    a.config.id = id.into();
    a.config.name = format!("agent-{id}");
    a.config.project_dir = dir.into();
    a.config.provider = ProviderId::Claude;
    a.config.model = "sonnet".into();
    a.config.created_at = format!("2026-11-{day:02}T00:00:00Z");
    a.status = status;
    a
}

/// Two projects: /work/api with a1 (running), a2 (running), a3 (idle); /work/web with w1 (running).
/// api is the more recent project, so it comes first in the sidebar and the tabs.
fn agents() -> Vec<AgentState> {
    vec![
        agent("a1", "/work/api", AgentStatus::Running, 10),
        agent("a2", "/work/api", AgentStatus::Running, 11),
        agent("a3", "/work/api", AgentStatus::Idle, 12),
        agent("w1", "/work/web", AgentStatus::Running, 4),
    ]
}

fn app_in(config: AppConfig, list: Vec<AgentState>) -> App {
    let mut app = App::new(config, list, ColorMode::TrueColor, PathBuf::from("/tmp"));
    app.on_resize(140, 40);
    app.start();
    app
}

fn new_app() -> App {
    app_in(cfg(), agents())
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn chr(c: char) -> KeyEvent {
    key(KeyCode::Char(c))
}

fn prefix(app: &mut App, c: char) -> Vec<Effect> {
    app.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL));
    app.handle_key(chr(c))
}

fn prefix_key(app: &mut App, code: KeyCode, mods: KeyModifiers) -> Vec<Effect> {
    app.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL));
    app.handle_key(KeyEvent::new(code, mods))
}

fn grid_app() -> App {
    let mut app = new_app();
    prefix(&mut app, 'g');
    assert_eq!(app.view, ViewMode::Grid);
    app
}

fn sent_inputs(fx: &[Effect]) -> Vec<(String, String)> {
    fx.iter()
        .filter_map(|e| match e {
            Effect::SendInput(id, text) => Some((id.clone(), text.clone())),
            _ => None,
        })
        .collect()
}

// ── views ───────────────────────────────────────────────────────────────────────────────

#[test]
fn prefix_g_and_backslash_toggle_chat_and_grid() {
    let mut app = new_app();
    assert_eq!(app.view, ViewMode::Chat);
    assert_eq!(app.focus, Focus::Terminal);
    prefix(&mut app, 'g');
    assert_eq!((app.view, app.focus), (ViewMode::Grid, Focus::Grid));
    prefix(&mut app, '\\');
    assert_eq!((app.view, app.focus), (ViewMode::Chat, Focus::Terminal));
}

#[test]
fn overview_is_a_placeholder_view_and_v_cycles() {
    let mut app = new_app();
    prefix(&mut app, 'O');
    assert_eq!(app.view, ViewMode::Overview);
    assert_eq!(app.focus, Focus::App);
    // Typing goes nowhere in the overview.
    assert!(app.handle_key(chr('z')).is_empty());
    prefix(&mut app, 'O');
    assert_eq!(app.view, ViewMode::Chat);
    prefix(&mut app, 'v');
    assert_eq!(app.view, ViewMode::Grid);
    prefix(&mut app, 'v');
    assert_eq!(app.view, ViewMode::Overview);
    prefix(&mut app, 'v');
    assert_eq!(app.view, ViewMode::Chat);
}

#[test]
fn the_saved_view_wins_unless_the_config_default_changed() {
    let state = |view, default| TuiState {
        view: Some(view),
        default_view: Some(default),
        ..TuiState::default()
    };
    let mut app = App::new(cfg(), agents(), ColorMode::TrueColor, PathBuf::from("/tmp"));
    app.restore_state(state(ViewMode::Grid, ViewMode::Chat));
    assert_eq!(app.view, ViewMode::Grid, "same default as when saved");

    let config = AppConfig {
        default_view_mode: ViewMode::Overview,
        ..cfg()
    };
    let mut app = App::new(
        config.clone(),
        agents(),
        ColorMode::TrueColor,
        PathBuf::from("/tmp"),
    );
    app.restore_state(state(ViewMode::Grid, ViewMode::Chat));
    assert_eq!(app.view, ViewMode::Overview, "the config default changed");

    // First run: no state at all.
    let mut app = App::new(
        config,
        agents(),
        ColorMode::TrueColor,
        PathBuf::from("/tmp"),
    );
    app.restore_state(TuiState::default());
    assert_eq!(app.view, ViewMode::Overview);
}

#[test]
fn starting_in_the_grid_focuses_the_tiles() {
    let config = AppConfig {
        default_view_mode: ViewMode::Grid,
        ..cfg()
    };
    let mut app = App::new(
        config,
        agents(),
        ColorMode::TrueColor,
        PathBuf::from("/tmp"),
    );
    app.restore_state(TuiState::default());
    let mut fx = app.on_resize(140, 40);
    fx.extend(app.start());
    assert_eq!((app.view, app.focus), (ViewMode::Grid, Focus::Grid));
    assert!(fx.contains(&Effect::Subscribe("a1".into())));
}

#[test]
fn state_snapshot_restores_view_selection_sidebar_and_grid() {
    let mut app = grid_app();
    prefix(&mut app, 'z');
    app.exec(Command::CycleSidebarSort);
    app.exec(Command::CycleSidebarStatus);
    app.sidebar_width = 36;
    let snap = app.state_snapshot();
    assert_eq!(snap.view, Some(ViewMode::Grid));
    assert_eq!(snap.selected_agent.as_deref(), Some("a1"));
    assert_eq!(snap.expanded_tile.as_deref(), Some("a1"));
    assert_eq!(snap.grid_tab.as_deref(), Some("/work/api"));
    assert_eq!(snap.sidebar_sort, SortKey::Recent);
    assert_eq!(snap.sidebar_status, StatusChip::Running);

    let mut again = App::new(cfg(), agents(), ColorMode::TrueColor, PathBuf::from("/tmp"));
    again.restore_state(snap.clone());
    again.on_resize(140, 40);
    again.start();
    assert_eq!(again.state_snapshot(), snap);
    assert_eq!(again.sidebar_width, 36);
}

#[test]
fn a_restored_selection_that_no_longer_exists_falls_back_to_the_first_agent() {
    let mut app = App::new(cfg(), agents(), ColorMode::TrueColor, PathBuf::from("/tmp"));
    app.restore_state(TuiState {
        selected_agent: Some("gone".into()),
        ..TuiState::default()
    });
    app.on_resize(140, 40);
    app.start();
    assert!(app.selected.is_some());
    assert_ne!(app.selected.as_deref(), Some("gone"));
}

// ── grid ────────────────────────────────────────────────────────────────────────────────

#[test]
fn the_grid_lists_the_agents_of_the_selected_project_and_subscribes_to_them() {
    let mut app = new_app();
    let fx = prefix(&mut app, 'g');
    assert_eq!(app.grid.tab, "/work/api");
    assert_eq!(app.grid.tiles, ["a1", "a2", "a3"]);
    assert_eq!(app.selected.as_deref(), Some("a1"));
    for id in ["a2", "a3"] {
        assert!(fx.contains(&Effect::Subscribe(id.into())), "{id}");
        assert!(app.terms.contains_key(id));
    }
    // Every terminal has the chat pane size: tiles show a window, they do not reflow the agents.
    let (cols, rows) = app.pane_size();
    for id in ["a1", "a2", "a3"] {
        assert!(
            fx.contains(&Effect::Resize(id.into(), cols, rows))
                || app.terms[id].sent_size == Some((cols, rows)),
            "{id}"
        );
        assert_eq!(app.terms[id].emu.size(), (cols, rows));
    }
}

#[test]
fn tabs_are_running_plus_one_per_project() {
    let app = grid_app();
    let tabs = app.grid_tabs();
    let labels: Vec<_> = tabs.iter().map(|t| (t.label.as_str(), t.count)).collect();
    assert_eq!(labels, [("Running", 3), ("api", 3), ("web", 1)]);
}

#[test]
fn bracket_keys_switch_tabs_and_the_running_tab_shows_every_running_agent() {
    let mut app = grid_app();
    // Tabs: Running, api, web. The grid starts on api.
    app.handle_key(chr(']'));
    assert_eq!(app.grid.tab, "/work/web");
    assert_eq!(app.grid.tiles, ["w1"]);
    assert_eq!(
        app.selected.as_deref(),
        Some("w1"),
        "the selection follows the tab"
    );
    app.handle_key(chr(']'));
    assert_eq!(app.grid.tab, RUNNING_TAB, "wraps around");
    let mut running = app.grid.tiles.clone();
    running.sort();
    assert_eq!(running, ["a1", "a2", "w1"]);
    app.handle_key(chr('['));
    assert_eq!(app.grid.tab, "/work/web");
}

#[test]
fn hjkl_and_arrows_move_the_tile_focus() {
    let mut app = grid_app();
    assert_eq!(app.selected.as_deref(), Some("a1"));
    app.handle_key(chr('l'));
    assert_eq!(app.selected.as_deref(), Some("a2"));
    app.handle_key(key(KeyCode::Right));
    assert_eq!(app.selected.as_deref(), Some("a3"));
    app.handle_key(chr('l'));
    assert_eq!(app.selected.as_deref(), Some("a3"), "stays at the end");
    app.handle_key(chr('h'));
    assert_eq!(app.selected.as_deref(), Some("a2"));
    assert_eq!(app.focus, Focus::Grid, "navigation never attaches");
}

#[test]
fn enter_attaches_the_tile_and_keys_reach_that_agent_until_prefix_esc() {
    let mut app = grid_app();
    app.handle_key(chr('l'));
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(app.focus, Focus::Terminal);
    let fx = app.handle_key(chr('x'));
    assert!(fx.contains(&Effect::SendRaw("a2".into(), b"x".to_vec())));
    prefix_key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
    assert_eq!(app.focus, Focus::Grid);
    assert_eq!(app.view, ViewMode::Grid);
}

#[test]
fn escape_is_for_the_agent_while_attached() {
    let mut app = grid_app();
    app.handle_key(key(KeyCode::Enter));
    let fx = app.handle_key(key(KeyCode::Esc));
    assert!(fx.contains(&Effect::SendRaw("a1".into(), vec![0x1b])));
    assert_eq!(app.focus, Focus::Terminal);
}

#[test]
fn c_opens_the_focused_tile_in_the_chat_view() {
    let mut app = grid_app();
    app.handle_key(chr('l'));
    app.handle_key(chr('c'));
    assert_eq!((app.view, app.focus), (ViewMode::Chat, Focus::Terminal));
    assert_eq!(app.selected.as_deref(), Some("a2"));
}

#[test]
fn clicking_a_tile_header_opens_chat_and_a_body_attaches() {
    use ratatui::crossterm::event::{MouseEvent, MouseEventKind};
    let mut app = grid_app();
    let layout = app.grid_layout();
    let second = layout.tiles[1];
    let click = |x, y| MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: x,
        row: y,
        modifiers: KeyModifiers::NONE,
    };
    app.handle_mouse(click(second.rect.x + 3, second.rect.y + 3));
    assert_eq!((app.view, app.focus), (ViewMode::Grid, Focus::Terminal));
    assert_eq!(app.selected.as_deref(), Some("a2"));
    app.focus = Focus::Grid;
    let third = layout.tiles[2];
    app.handle_mouse(click(third.rect.x + 3, third.rect.y));
    assert_eq!(
        (app.view, app.selected.as_deref()),
        (ViewMode::Chat, Some("a3"))
    );
}

#[test]
fn clicking_a_tab_switches_it() {
    use ratatui::crossterm::event::{MouseEvent, MouseEventKind};
    let mut app = grid_app();
    let l = app.layout();
    // Running is the first tab at the left edge of the title row.
    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: l.title.x + 2,
        row: l.title.y,
        modifiers: KeyModifiers::NONE,
    });
    assert_eq!(app.grid.tab, RUNNING_TAB);
}

#[test]
fn z_expands_the_focused_tile_and_collapses_it_again() {
    let mut app = grid_app();
    app.handle_key(chr('l'));
    app.handle_key(chr('z'));
    assert_eq!(app.grid.expanded.as_deref(), Some("a2"));
    let layout = app.grid_layout();
    assert_eq!(layout.tiles.len(), 1);
    assert_eq!(layout.tiles[0].rect, app.layout().terminal);
    assert_eq!(app.grid.visible, ["a2"]);
    prefix(&mut app, 'z');
    assert_eq!(app.grid.expanded, None);
    assert_eq!(app.grid.visible.len(), 3);
}

#[test]
fn an_expanded_tile_that_disappears_is_collapsed() {
    let mut app = grid_app();
    app.handle_key(chr('z'));
    app.handle_msg(Msg::Agents(
        agents().into_iter().filter(|a| a.id != "a1").collect(),
    ));
    assert_eq!(app.grid.expanded, None);
    assert_eq!(app.grid.tiles, ["a2", "a3"]);
    assert_eq!(app.selected.as_deref(), Some("a2"));
}

#[test]
fn angle_brackets_reorder_tiles_and_the_order_is_remembered() {
    let mut app = grid_app();
    app.handle_key(chr('>'));
    assert_eq!(app.grid.tiles, ["a2", "a1", "a3"]);
    assert_eq!(
        app.selected.as_deref(),
        Some("a1"),
        "the focus moves with the tile"
    );
    app.handle_key(chr('>'));
    assert_eq!(app.grid.tiles, ["a2", "a3", "a1"]);
    app.handle_key(chr('<'));
    assert_eq!(app.grid.tiles, ["a2", "a1", "a3"]);
    assert_eq!(
        app.state_snapshot().tile_order["/work/api"],
        ["a2", "a1", "a3"]
    );
    // Shift+Left with the prefix does the same.
    prefix_key(&mut app, KeyCode::Left, KeyModifiers::SHIFT);
    assert_eq!(app.grid.tiles, ["a1", "a2", "a3"]);
    // New agents are appended after the user's order.
    let mut list = agents();
    list.push(agent("a4", "/work/api", AgentStatus::Idle, 5));
    app.handle_msg(Msg::Agents(list));
    assert_eq!(app.grid.tiles, ["a1", "a2", "a3", "a4"]);
}

#[test]
fn the_running_tab_cannot_be_reordered() {
    let mut app = grid_app();
    app.handle_key(chr('['));
    assert_eq!(app.grid.tab, RUNNING_TAB);
    let before = app.grid.tiles.clone();
    app.handle_key(chr('>'));
    assert_eq!(app.grid.tiles, before);
    assert!(
        app.toasts
            .iter()
            .any(|t| t.body.contains("cannot be reordered"))
    );
}

#[test]
fn u_shows_only_running_agents_of_a_project() {
    let mut app = grid_app();
    assert_eq!(app.grid.tiles.len(), 3);
    app.handle_key(chr('u'));
    assert_eq!(app.grid.tiles, ["a1", "a2"]);
    assert!(app.state_snapshot().grid_only_running);
    app.handle_key(chr('u'));
    assert_eq!(app.grid.tiles.len(), 3);
}

#[test]
fn shift_c_cycles_the_columns_and_asks_the_daemon_to_store_them() {
    let mut app = grid_app();
    let fx = app.handle_key(chr('C'));
    assert!(fx.contains(&Effect::SetGridColumns(GridColumns::Two)));
    assert_eq!(app.config.grid_columns, GridColumns::Two);
    let fx = app.handle_key(chr('C'));
    assert!(fx.contains(&Effect::SetGridColumns(GridColumns::Three)));
    let fx = app.handle_key(chr('C'));
    assert!(fx.contains(&Effect::SetGridColumns(GridColumns::Auto)));
}

#[test]
fn the_sidebar_selection_moves_the_grid_to_that_project() {
    let mut app = grid_app();
    assert_eq!(app.grid.tab, "/work/api");
    app.exec(Command::FocusSidebar);
    assert_eq!(app.focus, Focus::App);
    app.select("w1");
    assert_eq!(app.grid.tab, "/work/web");
    assert_eq!(app.grid.tiles, ["w1"]);
    // Tab/Esc from the sidebar return to tile navigation in the grid.
    app.handle_key(key(KeyCode::Esc));
    assert_eq!(app.focus, Focus::Grid);
}

#[test]
fn many_tiles_are_paged_and_never_exceed_the_subscription_cap() {
    let list: Vec<AgentState> = (0..14)
        .map(|i| {
            agent(
                &format!("t{i:02}"),
                "/work/big",
                AgentStatus::Running,
                1 + (i % 20),
            )
        })
        .collect();
    let mut app = app_in(cfg(), list);
    prefix(&mut app, 'g');
    assert!(app.grid.visible.len() <= MAX_LIVE_TERMS);
    assert!(app.terms.len() <= MAX_LIVE_TERMS);
    // Moving to the last tile scrolls the grid and keeps the cap.
    for _ in 0..14 {
        app.handle_key(chr('j'));
        app.handle_key(chr('l'));
    }
    let last = app.grid.tiles.last().cloned().unwrap();
    assert_eq!(app.selected.as_deref(), Some(last.as_str()));
    assert!(app.grid.visible.contains(&last));
    assert!(app.terms.len() <= MAX_LIVE_TERMS);
}

// ── broadcast ───────────────────────────────────────────────────────────────────────────

fn type_text(app: &mut App, text: &str) {
    for c in text.chars() {
        app.handle_key(chr(c));
    }
}

#[test]
fn broadcast_sends_to_the_running_agents_of_the_tab_and_confirms_the_count() {
    let mut app = grid_app();
    prefix(&mut app, 'b');
    assert_eq!(app.focus, Focus::Broadcast);
    assert!(
        app.layout().input.is_some(),
        "the bar takes rows from the tiles"
    );
    type_text(&mut app, "run the tests");
    let fx = app.handle_key(key(KeyCode::Enter));
    assert_eq!(
        sent_inputs(&fx),
        [
            ("a1".to_string(), "run the tests".to_string()),
            ("a2".to_string(), "run the tests".to_string())
        ],
        "the idle agent a3 is left alone"
    );
    assert!(
        app.toasts
            .iter()
            .any(|t| t.title == "Broadcast" && t.body == "Sent to 2 agents")
    );
    assert!(app.broadcast.is_empty());
    assert_eq!(app.focus, Focus::Broadcast, "the bar stays open");
    app.handle_key(key(KeyCode::Esc));
    assert_eq!(app.focus, Focus::Grid);
    assert!(app.layout().input.is_none());
}

#[test]
fn broadcast_on_the_running_tab_reaches_every_project() {
    let mut app = grid_app();
    app.handle_key(chr('['));
    app.handle_key(chr('b'));
    type_text(&mut app, "hi");
    let fx = app.handle_key(key(KeyCode::Enter));
    let mut ids: Vec<_> = sent_inputs(&fx).into_iter().map(|(id, _)| id).collect();
    ids.sort();
    assert_eq!(ids, ["a1", "a2", "w1"]);
    assert!(app.toasts.iter().any(|t| t.body == "Sent to 3 agents"));
}

#[test]
fn broadcast_supports_multiple_lines_and_ignores_empty_input() {
    let mut app = grid_app();
    app.handle_key(chr('b'));
    assert!(
        app.handle_key(key(KeyCode::Enter)).is_empty(),
        "nothing to send"
    );
    type_text(&mut app, "one");
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT));
    type_text(&mut app, "two");
    assert_eq!(app.broadcast.line_count(), 2);
    let fx = app.handle_key(key(KeyCode::Enter));
    assert_eq!(sent_inputs(&fx)[0].1, "one\ntwo");
}

#[test]
fn broadcast_without_running_agents_warns() {
    let list = vec![agent("i1", "/work/idle", AgentStatus::Idle, 1)];
    let mut app = app_in(cfg(), list);
    prefix(&mut app, 'g');
    app.handle_key(chr('b'));
    type_text(&mut app, "hi");
    let fx = app.handle_key(key(KeyCode::Enter));
    assert!(sent_inputs(&fx).is_empty());
    assert!(
        app.toasts
            .iter()
            .any(|t| t.body.contains("No running agent"))
    );
}

#[test]
fn prefix_b_in_the_chat_view_opens_the_grid_with_the_bar() {
    let mut app = new_app();
    prefix(&mut app, 'b');
    assert_eq!((app.view, app.focus), (ViewMode::Grid, Focus::Broadcast));
}

// ── palette ─────────────────────────────────────────────────────────────────────────────

#[test]
fn the_palette_opens_with_prefix_p_and_colon() {
    let mut app = new_app();
    prefix(&mut app, 'p');
    assert!(matches!(app.modal, Some(Modal::Palette(_))));
    app.handle_key(key(KeyCode::Esc));
    assert!(app.modal.is_none());
    prefix(&mut app, ':');
    assert!(matches!(app.modal, Some(Modal::Palette(_))));
}

#[test]
fn the_palette_switches_agents_and_views_and_runs_commands() {
    let mut app = new_app();
    prefix(&mut app, 'p');
    type_text(&mut app, "agent-w1");
    app.handle_key(key(KeyCode::Enter));
    assert!(app.modal.is_none());
    assert_eq!(app.selected.as_deref(), Some("w1"));
    assert_eq!(app.focus, Focus::Terminal);

    prefix(&mut app, 'p');
    type_text(&mut app, "view grid");
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(app.view, ViewMode::Grid);

    prefix(&mut app, 'p');
    type_text(&mut app, "new agent");
    app.handle_key(key(KeyCode::Enter));
    assert!(matches!(app.modal, Some(Modal::NewAgent(_))));
}

#[test]
fn palette_switching_to_an_agent_in_the_grid_keeps_the_grid() {
    let mut app = grid_app();
    prefix(&mut app, 'p');
    type_text(&mut app, "agent-w1");
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(app.view, ViewMode::Grid);
    assert_eq!(app.grid.tab, "/work/web");
    assert_eq!(app.focus, Focus::Grid);
}

#[test]
fn keys_typed_in_the_palette_do_not_trigger_bindings() {
    let mut app = new_app();
    prefix(&mut app, 'p');
    app.handle_key(chr('q'));
    app.handle_key(chr('?'));
    assert!(matches!(app.modal, Some(Modal::Palette(_))));
    assert!(!app.quit);
}

// ── keybindings file ────────────────────────────────────────────────────────────────────

#[test]
fn user_bindings_replace_defaults_and_direct_chords_work_everywhere() {
    let mut app = new_app();
    app.apply_keybindings(
        Some(r#"[{"command":"toggle-view","keys":"alt+g"},{"command":"new-agent","keys":"prefix N"}]"#),
        true,
    );
    assert!(app.toasts.iter().any(|t| t.body.contains("2 overrides")));
    // The old key is gone, the new ones work.
    assert!(prefix(&mut app, 'n').is_empty());
    assert!(app.modal.is_none());
    prefix(&mut app, 'N');
    assert!(matches!(app.modal, Some(Modal::NewAgent(_))));
    app.modal = None;
    app.handle_key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::ALT));
    assert_eq!(app.view, ViewMode::Grid);
    // Alt+g never reaches the agent, also with the terminal focused in the grid.
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(app.focus, Focus::Terminal);
    let fx = app.handle_key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::ALT));
    assert!(fx.iter().all(|e| !matches!(e, Effect::SendRaw(..))));
    assert_eq!(app.view, ViewMode::Chat);
}

#[test]
fn problems_in_the_file_become_toasts_and_the_defaults_stay() {
    let mut app = new_app();
    app.apply_keybindings(
        Some(
            r#"[{"command":"bogus","keys":"x"},{"command":"quit","keys":"prefix nope"},
                {"command":"new-agent","keys":"prefix r"}]"#,
        ),
        true,
    );
    let bodies: Vec<_> = app.toasts.iter().map(|t| t.body.clone()).collect();
    assert!(
        bodies
            .iter()
            .any(|b| b.contains("unknown command \"bogus\"")),
        "{bodies:?}"
    );
    assert!(
        bodies.iter().any(|b| b.contains("invalid keys")),
        "{bodies:?}"
    );
    assert!(
        bodies.iter().any(|b| b.contains("rename-agent")),
        "{bodies:?}"
    );
    // `prefix q` still quits (invalid override ignored).
    prefix(&mut app, 'q');
    assert!(app.quit || matches!(app.modal, Some(Modal::Quit { .. })));
}

#[test]
fn many_problems_are_summarised() {
    let mut app = new_app();
    let rules: Vec<String> = (0..6)
        .map(|i| format!(r#"{{"command":"nope{i}","keys":"x"}}"#))
        .collect();
    app.apply_keybindings(Some(&format!("[{}]", rules.join(","))), true);
    assert_eq!(app.toasts.len(), 4);
    assert!(app.toasts.last().is_some_and(|t| t.body.contains("3 more")));
}

#[test]
fn deleting_the_file_restores_the_defaults() {
    let mut app = new_app();
    app.apply_keybindings(
        Some(r#"[{"command":"new-agent","keys":"prefix N"}]"#),
        false,
    );
    assert!(prefix(&mut app, 'n').is_empty());
    app.apply_keybindings(None, false);
    prefix(&mut app, 'n');
    assert!(matches!(app.modal, Some(Modal::NewAgent(_))));
}

#[test]
fn a_changed_prefix_key_keeps_the_user_bindings() {
    let mut app = new_app();
    app.apply_keybindings(
        Some(r#"[{"command":"new-agent","keys":"prefix N"}]"#),
        false,
    );
    let mut config = app.config.clone();
    config.prefix_key = "ctrl+b".into();
    app.handle_msg(Msg::Config(Box::new(config)));
    assert_eq!(app.keymap.prefix, crate::keymap::KeyChord::ctrl('b'));
    assert_eq!(app.keymap.hint_for("new-agent"), Some("Ctrl+B N"));
}

#[test]
fn open_keybindings_file_suspends_the_tui_with_the_editor() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("keybindings.json");
    let mut app = new_app();
    app.set_keybindings_path(path.clone());
    let fx = app.exec(Command::OpenKeybindingsFile);
    assert!(path.exists(), "the file is created with the defaults first");
    assert!(
        matches!(&fx[..], [Effect::Suspend(cmd)] if cmd.args.last().map(String::as_str) == path.to_str())
    );
    // Without a path: an error toast, no effect.
    let mut app = new_app();
    assert!(app.exec(Command::OpenKeybindingsFile).is_empty());
    assert!(app.toasts.iter().any(|t| t.kind == ToastKind::Error));
}

// ── sidebar filter / sort ───────────────────────────────────────────────────────────────

#[test]
fn status_chip_and_sort_commands_cycle_and_filter_the_sidebar() {
    let mut app = new_app();
    app.exec(Command::FocusSidebar);
    app.handle_key(chr('f'));
    assert_eq!(app.sidebar.status, StatusChip::Running);
    let shown: Vec<_> = app
        .sidebar_rows()
        .iter()
        .filter_map(|r| match r {
            Row::Agent(i) => Some(app.agents[*i].id.clone()),
            Row::Header { .. } => None,
        })
        .collect();
    assert_eq!(shown, ["a1", "a2", "w1"]);
    app.handle_key(chr('S'));
    assert_eq!(app.sidebar.sort, SortKey::Recent);
}

#[test]
fn slash_in_the_prefix_table_focuses_the_search_and_filters_by_name_project_and_model() {
    let mut app = new_app();
    prefix(&mut app, '/');
    assert_eq!((app.focus, app.sidebar.filtering), (Focus::App, true));
    type_text(&mut app, "web");
    assert_eq!(app.sidebar_rows().len(), 2, "one header and w1");
    app.handle_key(key(KeyCode::Enter));
    assert!(!app.sidebar.filtering);
    assert_eq!(app.sidebar.filter, "web");
}

#[test]
fn waiting_chip_uses_the_waiting_set() {
    let mut app = new_app();
    app.handle_event(Event::AgentWaiting {
        agent_id: "a2".into(),
    });
    app.sidebar.status = StatusChip::Waiting;
    let rows = app.sidebar_rows();
    assert_eq!(rows.len(), 2);
    assert!(matches!(rows[1], Row::Agent(i) if app.agents[i].id == "a2"));
}

#[test]
fn keymap_default_rules_are_the_ones_of_the_catalog() {
    let rules: Vec<KeybindingRule> = Vec::new();
    let (km, problems) = Keymap::from_user_rules("ctrl+a", &rules);
    assert!(problems.is_empty());
    assert_eq!(km.hint_for("toggle-view"), Some("Ctrl+A g"));
}

#[test]
fn the_running_tab_stays_selected_when_its_agents_stop() {
    let mut app = grid_app();
    app.handle_key(chr('['));
    assert_eq!(app.grid.tab, RUNNING_TAB);
    for id in ["a1", "a2", "w1"] {
        app.handle_event(Event::AgentStatus {
            agent_id: id.into(),
            status: AgentStatus::Errored,
            session_id: None,
            model: "sonnet".into(),
        });
    }
    assert_eq!(app.grid.tab, RUNNING_TAB);
    assert!(app.grid.tiles.is_empty());
    // Choosing an agent in the sidebar still moves the grid to its project.
    app.select("a3");
    assert_eq!(app.grid.tab, "/work/api");
}
