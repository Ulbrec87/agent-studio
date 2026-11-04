//! Overview view: render snapshots (wide, narrow, details drawer) and key handling, all on a
//! fixed clock and UTC so they do not depend on the machine.

use std::path::PathBuf;

use as_core::overview::{
    AgentDetails, EventKind, HistoryAgent, HistoryPoint, HistoryResponse, LastMessage,
    MeasuredSpan, OverviewEvent, OverviewSession, OverviewSnapshot, OverviewStatus as S,
    TokenCounts, WaitDirection, WaitLedgerData, WaitTransition, WaitingReason,
};
use as_core::{AgentState, AgentStatus, AppConfig, ProviderId, ViewMode};
use as_proto::Event;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::{App, Effect, Focus, Msg};
use crate::overview::format::Clock;
use crate::overview::{Card, SessionsMode};
use crate::theme::ColorMode;

/// 2026-10-22 14:30:00 UTC.
pub(super) const NOW: i64 = 1_792_679_400_000;
const MIN: i64 = 60_000;

fn agent(id: &str, name: &str, dir: &str, provider: ProviderId, status: AgentStatus) -> AgentState {
    let mut a = AgentState::default();
    a.config.id = id.into();
    a.config.name = name.into();
    a.config.project_dir = dir.into();
    a.config.provider = provider;
    a.config.model = "sonnet".into();
    a.config.created_at = "2026-10-01T00:00:00Z".into();
    a.status = status;
    a
}

fn agents() -> Vec<AgentState> {
    vec![
        agent(
            "a1",
            "api-refactor",
            "/work/shop",
            ProviderId::Claude,
            AgentStatus::Running,
        ),
        agent(
            "a2",
            "web-ui",
            "/work/web",
            ProviderId::Claude,
            AgentStatus::Running,
        ),
        agent(
            "a3",
            "docs",
            "/work/docs",
            ProviderId::Codex,
            AgentStatus::Running,
        ),
        agent(
            "a4",
            "migrations",
            "/work/shop",
            ProviderId::Claude,
            AgentStatus::Running,
        ),
        agent(
            "a5",
            "scratch",
            "/work/shop",
            ProviderId::Claude,
            AgentStatus::Running,
        ),
        agent(
            "a6",
            "release-notes",
            "/work/docs",
            ProviderId::Claude,
            AgentStatus::Idle,
        ),
        agent(
            "a7",
            "flaky-tests",
            "/work/web",
            ProviderId::Claude,
            AgentStatus::Errored,
        ),
        agent(
            "a8",
            "old-spike",
            "/work/web",
            ProviderId::Claude,
            AgentStatus::Idle,
        ),
    ]
}

fn session(id: &str, name: &str, dir: &str, status: S, since: i64) -> OverviewSession {
    let mut s = OverviewSession::basic(id, status);
    s.name = name.into();
    s.project_dir = dir.into();
    s.status_since = since;
    s.started_at = NOW - 3_600_000;
    s.pid = Some(4000 + id.trim_start_matches('a').parse::<u32>().unwrap_or(0));
    s.repo_name = Some(dir.rsplit('/').next().unwrap_or("").to_string());
    s.branch = Some("main".into());
    s
}

fn snapshot() -> OverviewSnapshot {
    let mut a1 = session(
        "a1",
        "api-refactor",
        "/work/shop",
        S::NeedsYou,
        NOW - 252_000,
    );
    a1.waiting_reason = Some(WaitingReason::Permission);
    a1.waiting_text = Some("Run: npm test -- --run".into());
    a1.branch = Some("feat/cart".into());
    let mut a2 = session("a2", "web-ui", "/work/web", S::NeedsYou, NOW - 95_000);
    a2.waiting_reason = Some(WaitingReason::Question);
    a2.waiting_text = Some("Which database should the new service use?".into());
    let mut a3 = session("a3", "docs", "/work/docs", S::Working, NOW - 10 * MIN);
    a3.provider = ProviderId::Codex;
    a3.model = "gpt-5.4".into();
    let mut a4 = session("a4", "migrations", "/work/shop", S::Working, NOW - 40 * MIN);
    a4.quiet_for_ms = Some(12 * MIN);
    a4.last_output_at = NOW - 12 * MIN;
    let a5 = session("a5", "scratch", "/work/shop", S::Idle, NOW - 30 * MIN);
    let mut a6 = session(
        "a6",
        "release-notes",
        "/work/docs",
        S::Finished,
        NOW - 3 * 3_600_000,
    );
    a6.ended_at = Some(NOW - 3 * 3_600_000);
    a6.exit_code = Some(0);
    let mut a7 = session(
        "a7",
        "flaky-tests",
        "/work/web",
        S::Failed,
        NOW - 3 * 3_600_000,
    );
    a7.ended_at = Some(NOW - 3 * 3_600_000);
    a7.exit_code = Some(1);
    let mut a8 = session(
        "a8",
        "old-spike",
        "/work/web",
        S::NotRunning,
        NOW - 2 * 86_400_000,
    );
    a8.ended_at = Some(NOW - 2 * 86_400_000);
    let sessions = vec![a1, a2, a3, a4, a5, a6, a7, a8];
    OverviewSnapshot {
        at: NOW,
        counts: as_core::overview::status::compute_counts(&sessions, NOW),
        sessions,
    }
}

fn point(at: i64, statuses: &[(&str, S)]) -> HistoryPoint {
    let count = |st: S| statuses.iter().filter(|(_, x)| *x == st).count() as u32;
    HistoryPoint {
        at,
        needs_you: count(S::NeedsYou),
        working: count(S::Working),
        idle: count(S::Idle),
        total: statuses.len() as u32,
        statuses: statuses
            .iter()
            .map(|(k, v)| ((*k).to_string(), *v))
            .collect(),
    }
}

fn event(at: i64, id: &str, name: &str, from: Option<S>, to: Option<S>) -> OverviewEvent {
    OverviewEvent {
        id: format!("{id}@{at}"),
        at,
        kind: EventKind::Status,
        agent_id: id.into(),
        name: name.into(),
        project_dir: "/work/shop".into(),
        from,
        to,
        waiting_reason: None,
        waited_ms: None,
        gap_ms: None,
    }
}

fn history() -> HistoryResponse {
    let mut points = Vec::new();
    // 45 minutes of samples, one every 5 s up to now; a gap 20 to 15 minutes ago.
    let mut t = NOW - 45 * MIN;
    while t <= NOW {
        if !(NOW - 20 * MIN..NOW - 15 * MIN).contains(&t) {
            let mins = (NOW - t) / MIN;
            let a1 = if mins < 5 {
                S::NeedsYou
            } else if mins < 25 {
                S::Working
            } else {
                S::Idle
            };
            let a2 = if mins < 2 { S::NeedsYou } else { S::Working };
            points.push(point(
                t,
                &[
                    ("a1", a1),
                    ("a2", a2),
                    ("a3", S::Working),
                    ("a4", S::Working),
                    ("a5", S::Idle),
                    ("a6", S::Finished),
                    ("a7", S::Failed),
                ],
            ));
        }
        t += 5_000;
    }
    let mut wait_end = event(
        NOW - 40 * MIN,
        "a1",
        "api-refactor",
        Some(S::NeedsYou),
        Some(S::Working),
    );
    wait_end.waited_ms = Some(3 * MIN + 20_000);
    let mut enter = event(
        NOW - 252_000,
        "a1",
        "api-refactor",
        Some(S::Working),
        Some(S::NeedsYou),
    );
    enter.waiting_reason = Some(WaitingReason::Permission);
    let mut resumed = event(NOW - 15 * MIN, "", "", None, None);
    resumed.kind = EventKind::Resumed;
    resumed.gap_ms = Some(5 * MIN);
    resumed.id = "resumed".into();
    let mut started = event(NOW - 50 * MIN, "a3", "docs", None, Some(S::Working));
    started.kind = EventKind::Appeared;
    HistoryResponse {
        since: NOW - 45 * MIN,
        started_at: NOW - 45 * MIN,
        starts: vec![NOW - 45 * MIN],
        points,
        events: vec![
            started,
            wait_end,
            resumed,
            event(
                NOW - 10 * MIN,
                "a3",
                "docs",
                Some(S::Idle),
                Some(S::Working),
            ),
            enter,
            event(
                NOW - 95_000,
                "a2",
                "web-ui",
                Some(S::Working),
                Some(S::NeedsYou),
            ),
        ],
        agents: [
            ("a1", "api-refactor", "/work/shop"),
            ("a2", "web-ui", "/work/web"),
            ("a3", "docs", "/work/docs"),
            ("a4", "migrations", "/work/shop"),
            ("a5", "scratch", "/work/shop"),
            ("a6", "release-notes", "/work/docs"),
            ("a7", "flaky-tests", "/work/web"),
        ]
        .iter()
        .map(|(id, n, d)| {
            (
                (*id).to_string(),
                HistoryAgent {
                    name: (*n).into(),
                    project_dir: (*d).into(),
                },
            )
        })
        .collect(),
    }
}

fn ledger() -> WaitLedgerData {
    let tr = |agent: &str, dir: &str, direction, at: i64| WaitTransition {
        agent_id: agent.into(),
        name: agent.into(),
        project_dir: dir.into(),
        at,
        direction,
    };
    WaitLedgerData {
        transitions: vec![
            tr("a1", "/work/shop", WaitDirection::In, NOW - 2 * 86_400_000),
            tr(
                "a1",
                "/work/shop",
                WaitDirection::Out,
                NOW - 2 * 86_400_000 + 7 * MIN,
            ),
            tr("a2", "/work/web", WaitDirection::In, NOW - 3 * 3_600_000),
            tr(
                "a2",
                "/work/web",
                WaitDirection::Out,
                NOW - 3 * 3_600_000 + 2 * MIN,
            ),
            tr("a1", "/work/shop", WaitDirection::In, NOW - 252_000),
        ],
        spans: vec![MeasuredSpan {
            start: NOW - 3 * 86_400_000,
            end: NOW,
        }],
        starts: vec![NOW - 3 * 86_400_000],
    }
}

pub(super) fn app_with(width: u16, height: u16) -> App {
    let mut app = App::new(
        AppConfig::default(),
        agents(),
        ColorMode::TrueColor,
        PathBuf::from("/tmp"),
    );
    app.on_resize(width, height);
    app.start();
    app.overview.clock_override = Some(Clock {
        now: NOW,
        utc: true,
    });
    app.handle_event(Event::OverviewSnapshot(Box::new(snapshot())));
    app.handle_msg(Msg::OverviewHistory(Box::new(history())));
    app.handle_msg(Msg::OverviewWaits(Box::new(ledger())));
    app
}

pub(super) fn overview_app(width: u16, height: u16) -> App {
    let mut app = app_with(width, height);
    app.set_view(ViewMode::Overview);
    app.overview.marker = None;
    app
}

pub(super) fn draw(app: &mut App, w: u16, h: u16) -> Terminal<TestBackend> {
    let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
    terminal.draw(|f| super::draw(f, app)).unwrap();
    terminal
}

pub(super) fn text(t: &Terminal<TestBackend>) -> String {
    let buf = t.backend().buffer();
    (0..buf.area.height)
        .map(|y| {
            let line: String = (0..buf.area.width)
                .map(|x| buf[(x, y)].symbol().to_string())
                .collect();
            line.trim_end().to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub(super) fn press(app: &mut App, code: KeyCode) -> Vec<Effect> {
    app.handle_key(KeyEvent::new(code, KeyModifiers::NONE))
}
pub(super) fn ch(app: &mut App, c: char) -> Vec<Effect> {
    press(app, KeyCode::Char(c))
}

// ── snapshots ───────────────────────────────────────────────────────────────

#[test]
fn wide_layout_has_two_columns() {
    let mut app = overview_app(170, 64);
    app.overview.scroll = 0;
    let t = draw(&mut app, 170, 64);
    insta::assert_snapshot!(text(&t));
}

#[test]
fn narrow_layout_is_one_column() {
    let mut app = overview_app(86, 90);
    let t = draw(&mut app, 86, 90);
    insta::assert_snapshot!(text(&t));
}

#[test]
fn the_board_layout() {
    let mut app = overview_app(170, 64);
    ch(&mut app, 'm');
    ch(&mut app, 'm');
    assert_eq!(app.overview.mode, SessionsMode::Board);
    let screen = text(&draw(&mut app, 170, 64));
    assert!(screen.contains("Sessions \u{b7} Board"), "{screen}");
    assert!(screen.contains("Working 2"), "{screen}");
    assert!(screen.contains("Needs you 2"), "{screen}");
    assert!(screen.contains("Idle 1"), "{screen}");
    assert!(screen.contains("1 not running,"), "{screen}");
}

#[test]
fn the_details_drawer() {
    let mut app = overview_app(170, 50);
    app.overview.focus = Card::NeedsYou;
    press(&mut app, KeyCode::Char('d'));
    app.handle_msg(Msg::AgentDetails {
        agent_id: "a1".into(),
        details: Box::new(AgentDetails {
            last_message: LastMessage::Text {
                text: "I changed the cart reducer and now I need to run the test suite to check that nothing else broke.\n\nShall I go ahead?".into(),
                truncated: false,
            },
            tokens: Some(TokenCounts { input: 18_200, cached: Some(15_000), output: 412 }),
        }),
    });
    let t = draw(&mut app, 170, 50);
    insta::assert_snapshot!(text(&t));
}

#[test]
fn an_empty_overview_says_so() {
    let mut app = App::new(
        AppConfig::default(),
        Vec::new(),
        ColorMode::TrueColor,
        PathBuf::from("/tmp"),
    );
    app.on_resize(100, 24);
    app.start();
    app.handle_event(Event::OverviewSnapshot(Box::default()));
    app.set_view(ViewMode::Overview);
    let screen = text(&draw(&mut app, 100, 24));
    assert!(screen.contains("No agents yet"), "{screen}");
}

#[test]
fn needs_you_shows_the_all_clear_with_the_last_wait() {
    let mut app = overview_app(150, 40);
    let mut snap = snapshot();
    snap.sessions.retain(|s| s.status != S::NeedsYou);
    snap.at += 1;
    app.handle_event(Event::OverviewSnapshot(Box::new(snap)));
    let mut ended = event(
        NOW - 30_000,
        "a1",
        "api-refactor",
        Some(S::NeedsYou),
        Some(S::Working),
    );
    ended.waited_ms = Some(252_000);
    app.handle_event(Event::OverviewEvents(vec![ended]));
    let screen = text(&draw(&mut app, 150, 40));
    assert!(screen.contains("Nothing needs you"), "{screen}");
    assert!(screen.contains("Last wait 4m 12s"), "{screen}");
    assert!(
        screen.contains("api-refactor, answered at 14:29"),
        "{screen}"
    );
}

#[test]
fn the_new_since_marker_is_drawn_in_the_events_card() {
    let mut app = app_with(150, 50);
    app.overview.last_seen = Some(NOW - 12 * MIN);
    app.set_view(ViewMode::Overview);
    assert_eq!(app.overview.marker.map(|m| m.count), Some(3));
    let screen = text(&draw(&mut app, 150, 50));
    assert!(screen.contains("new since 14:18"), "{screen}");
}

// ── header badge, title and counts ──────────────────────────────────────────

#[test]
fn the_overview_tab_shows_an_amber_badge_with_the_needs_you_count_in_every_view() {
    let mut app = app_with(120, 30);
    for view in [ViewMode::Chat, ViewMode::Grid, ViewMode::Overview] {
        app.set_view(view);
        let t = draw(&mut app, 120, 30);
        let header = text(&t).lines().next().unwrap().to_string();
        assert!(header.contains("Overview  2"), "{view:?}: {header}");
        let buf = t.backend().buffer();
        let amber = app.theme.starting;
        let badge =
            (0..buf.area.width).any(|x| buf[(x, 0)].symbol() == "2" && buf[(x, 0)].bg == amber);
        assert!(badge, "{view:?}: the badge is amber");
    }
}

#[test]
fn the_title_and_waiting_count_come_from_the_overview() {
    let mut app = App::new(
        AppConfig::default(),
        agents(),
        ColorMode::TrueColor,
        PathBuf::from("/tmp"),
    );
    app.on_resize(100, 24);
    app.start();
    assert_eq!(app.title(), "Agent Studio");
    // Before the Overview reported: the 5 s silence rule.
    app.handle_event(Event::AgentWaiting {
        agent_id: "a1".into(),
    });
    assert_eq!(app.waiting_count(), 1);
    // Once it did, Claude agents count only when the registry says needs-you; a quiet Codex agent
    // still counts (the only signal it has).
    app.handle_event(Event::OverviewSnapshot(Box::new(snapshot())));
    assert_eq!(
        app.waiting_count(),
        2,
        "a1 and a2 need you; the quiet flag of a1 is not counted twice"
    );
    app.handle_event(Event::AgentWaiting {
        agent_id: "a3".into(),
    });
    assert_eq!(app.waiting_count(), 3);
    assert_eq!(app.title(), "(3) Agent Studio");
    let mut quiet = snapshot();
    quiet.at += 1;
    quiet.sessions.retain(|s| s.status != S::NeedsYou);
    app.handle_event(Event::OverviewSnapshot(Box::new(quiet)));
    assert_eq!(app.waiting_count(), 1);
}

// ── keys ────────────────────────────────────────────────────────────────────

#[test]
fn enter_on_needs_you_jumps_to_the_longest_waiter_in_the_chat_terminal() {
    let mut app = overview_app(150, 40);
    assert_eq!(
        (app.view, app.focus, app.overview.focus),
        (ViewMode::Overview, Focus::Overview, Card::NeedsYou)
    );
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.view, ViewMode::Chat);
    assert_eq!(app.focus, Focus::Terminal);
    assert_eq!(
        app.selected.as_deref(),
        Some("a1"),
        "a1 waited 4m12s, a2 1m35s"
    );
    // The second one with j.
    app.set_view(ViewMode::Overview);
    ch(&mut app, 'j');
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.selected.as_deref(), Some("a2"));
}

#[test]
fn tab_moves_between_cards_and_the_focused_card_is_highlighted() {
    let mut app = overview_app(170, 64);
    let order: Vec<Card> = (0..6)
        .map(|_| {
            press(&mut app, KeyCode::Tab);
            app.overview.focus
        })
        .collect();
    assert_eq!(
        order,
        [
            Card::Sessions,
            Card::Events,
            Card::LastHour,
            Card::Timeline,
            Card::Waits,
            Card::NeedsYou
        ]
    );
    press(&mut app, KeyCode::BackTab);
    assert_eq!(app.overview.focus, Card::Waits);
    app.overview.focus = Card::Sessions;
    let t = draw(&mut app, 170, 64);
    let buf = t.backend().buffer();
    let accent = app.theme.accent;
    let thick = (0..buf.area.height).any(|y| {
        (0..buf.area.width).any(|x| buf[(x, y)].symbol() == "\u{250F}" && buf[(x, y)].fg == accent)
    });
    assert!(thick, "the focused card has a thick accent border");
}

#[test]
fn j_and_k_move_within_the_sessions_card_and_enter_opens_the_drawer() {
    let mut app = overview_app(170, 64);
    app.overview.focus = Card::Sessions;
    ch(&mut app, 'j');
    ch(&mut app, 'j');
    assert_eq!(app.overview.sessions_cursor, 2);
    ch(&mut app, 'k');
    assert_eq!(app.overview.sessions_cursor, 1);
    let fx = press(&mut app, KeyCode::Enter);
    // Working group, longest in the status first: a4 (40m), a3 (10m): the cursor is on a3.
    assert_eq!(fx, vec![Effect::LoadAgentDetails("a3".into())]);
    assert_eq!(app.overview.drawer.as_ref().unwrap().agent_id, "a3");
    // Keys go to the drawer: j scrolls, Esc closes, Enter jumps.
    ch(&mut app, 'j');
    assert_eq!(app.overview.drawer.as_ref().unwrap().scroll, 1);
    press(&mut app, KeyCode::Esc);
    assert!(app.overview.drawer.is_none());
    press(&mut app, KeyCode::Enter);
    press(&mut app, KeyCode::Enter);
    assert_eq!(
        (app.view, app.selected.as_deref()),
        (ViewMode::Chat, Some("a3"))
    );
    assert!(
        app.overview.drawer.is_none(),
        "leaving the Overview closes the drawer"
    );
}

#[test]
fn m_cycles_the_sessions_mode_and_p_the_project_filter() {
    let mut app = overview_app(170, 64);
    ch(&mut app, 'm');
    assert_eq!(
        (app.overview.mode, app.overview.focus),
        (SessionsMode::Repos, Card::Sessions)
    );
    ch(&mut app, 'm');
    ch(&mut app, 'm');
    assert_eq!(app.overview.mode, SessionsMode::List);
    ch(&mut app, 'p');
    assert_eq!(app.overview.effective_project(), Some("/work/docs"));
    let screen = text(&draw(&mut app, 170, 64));
    assert!(screen.contains("Project docs (p)"), "{screen}");
    assert!(
        screen.contains("of 2 agents"),
        "only the docs agents are counted: {screen}"
    );
    ch(&mut app, 'p');
    ch(&mut app, 'p');
    ch(&mut app, 'p');
    assert_eq!(app.overview.effective_project(), None);
}

#[test]
fn enter_on_show_more_expands_the_ended_group() {
    let mut app = overview_app(170, 64);
    let mut snap = snapshot();
    snap.at += 1;
    for i in 0..12 {
        let mut s = session(
            &format!("n{i}"),
            &format!("old-{i:02}"),
            "/work/web",
            S::NotRunning,
            NOW - 3_600_000 * (i + 5),
        );
        s.ended_at = Some(s.status_since);
        snap.sessions.push(s);
    }
    app.handle_event(Event::OverviewSnapshot(Box::new(snap)));
    app.overview.focus = Card::Sessions;
    ch(&mut app, 'G');
    let rows = crate::overview::model::selectable_sessions(&app.overview);
    assert!(matches!(
        rows.last(),
        Some(crate::overview::model::SessionRow::ShowMore {
            expanded: false,
            ..
        })
    ));
    press(&mut app, KeyCode::Enter);
    let rows = crate::overview::model::selectable_sessions(&app.overview);
    assert!(matches!(
        rows.last(),
        Some(crate::overview::model::SessionRow::ShowMore { expanded: true, .. })
    ));
    assert!(app.overview.drawer.is_none());
}

#[test]
fn t_switches_the_waits_card_between_today_and_seven_days() {
    let mut app = overview_app(170, 64);
    let screen = text(&draw(&mut app, 170, 64));
    assert!(screen.contains("longest today"), "{screen}");
    ch(&mut app, 't');
    let screen = text(&draw(&mut app, 170, 64));
    assert!(screen.contains("longest in 7 days"), "{screen}");
}

#[test]
fn pushed_events_that_move_an_agent_in_or_out_of_needs_you_reload_the_ledger() {
    let mut app = overview_app(150, 40);
    let mut e = event(NOW, "a2", "web-ui", Some(S::NeedsYou), Some(S::Working));
    e.waited_ms = Some(MIN);
    assert_eq!(
        app.handle_event(Event::OverviewEvents(vec![e])),
        vec![Effect::LoadOverviewWaits]
    );
    let idle = event(NOW + 1, "a3", "docs", Some(S::Working), Some(S::Idle));
    assert!(
        app.handle_event(Event::OverviewEvents(vec![idle]))
            .is_empty()
    );
    // Outside the Overview nothing is fetched (it refreshes on entering).
    app.set_view(ViewMode::Chat);
    let e2 = event(NOW + 2, "a2", "web-ui", Some(S::Working), Some(S::NeedsYou));
    assert!(app.handle_event(Event::OverviewEvents(vec![e2])).is_empty());
}

#[test]
fn the_view_asks_for_history_and_waits_when_opened_and_every_30_seconds() {
    let mut app = app_with(150, 40);
    assert!(
        app.poll_effects(std::time::Instant::now()).is_empty(),
        "nothing while elsewhere"
    );
    app.set_view(ViewMode::Overview);
    let fx = app.poll_effects(std::time::Instant::now());
    assert!(
        fx.iter()
            .any(|e| matches!(e, Effect::LoadOverviewHistory(_))),
        "{fx:?}"
    );
    assert!(fx.contains(&Effect::LoadOverviewWaits));
    // The load is in flight: not asked again.
    assert!(app.poll_effects(std::time::Instant::now()).is_empty());
}

#[test]
fn the_daemon_being_old_is_reported_in_the_view() {
    let mut app = App::new(
        AppConfig::default(),
        agents(),
        ColorMode::TrueColor,
        PathBuf::from("/tmp"),
    );
    app.on_resize(120, 30);
    app.start();
    app.set_view(ViewMode::Overview);
    app.handle_msg(Msg::OverviewFailed("unknown variant `get_overview`".into()));
    let screen = text(&draw(&mut app, 120, 30));
    assert!(screen.contains("The overview is not available"), "{screen}");
    assert!(screen.contains("restart the daemon"), "{screen}");
}

#[test]
fn a_small_screen_does_not_panic() {
    for (w, h) in [(30, 8), (61, 12), (20, 5), (140, 6)] {
        let mut app = overview_app(w, h);
        draw(&mut app, w, h);
        ch(&mut app, 'j');
        press(&mut app, KeyCode::PageDown);
        press(&mut app, KeyCode::Tab);
        draw(&mut app, w, h);
    }
}
