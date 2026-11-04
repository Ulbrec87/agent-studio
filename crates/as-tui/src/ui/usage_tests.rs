//! Usage surfaces: header chip states, the report overlay (with and without costs), the Overview
//! card, and the effects that keep them fed. Fixed clock, UTC, text snapshots.

use as_core::usage::report::{UsageReportProvider, build_usage_report};
use as_core::usage::summary::summarize;
use as_core::usage::types::{
    IndexingState, ProfileStatus, TokenCounts, UsageBucket, UsagePeriod, UsageProvider,
    UsageScanProgress, UsageState,
};
use as_core::{AppConfig, ViewMode};
use as_proto::Event;
use chrono::{TimeZone, Utc};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::style::{Color, Modifier};

use super::overview_tests::{NOW, ch, draw, overview_app, press, text};
use crate::app::{App, Effect, Modal, Msg};
use crate::theme::ColorMode;
use crate::usage::model::fixtures::{codex, profile, usage_state, win};

const HOUR: i64 = 3_600_000;
const DAY: i64 = 24 * HOUR;

fn bucket(
    at: i64,
    provider: UsageProvider,
    model: &str,
    project: &str,
    input: u64,
    cost: f64,
) -> UsageBucket {
    UsageBucket {
        hour: at.div_euclid(HOUR) * HOUR,
        provider,
        model: model.into(),
        project_key: project.into(),
        tokens: TokenCounts {
            input,
            output: input / 10,
            ..Default::default()
        },
        messages: 3,
        cost_usd: cost,
        unpriced_messages: 0,
    }
}

/// Two weeks of Claude (two models, two projects) and a little Codex.
fn buckets() -> Vec<UsageBucket> {
    let mut out = Vec::new();
    for d in 0..14i64 {
        let at = NOW - d * DAY - 2 * HOUR;
        let load = 2_000_000 + (d % 5) * 900_000;
        out.push(bucket(
            at,
            UsageProvider::Claude,
            "claude-opus-5-5",
            "/work/shop",
            load as u64,
            3.0 + (d % 3) as f64,
        ));
        out.push(bucket(
            at - HOUR,
            UsageProvider::Claude,
            "claude-sonnet-4-6",
            "/work/web",
            (load / 4) as u64,
            0.8,
        ));
        if d % 3 == 0 {
            out.push(bucket(
                at - 2 * HOUR,
                UsageProvider::Codex,
                "gpt-5-codex",
                "/work/docs",
                300_000,
                0.5,
            ));
        }
    }
    out
}

fn now_utc() -> chrono::DateTime<Utc> {
    Utc.timestamp_millis_opt(NOW).unwrap()
}

/// Feed the app the state, the three summaries and the three reports.
fn load(app: &mut App, state: UsageState) {
    let all = buckets();
    app.usage.set_state(state);
    for p in [UsagePeriod::Today, UsagePeriod::Week, UsagePeriod::Month] {
        app.usage
            .set_summary(String::new(), summarize(&all, p, &now_utc(), None));
    }
    for p in [
        UsageReportProvider::Claude,
        UsageReportProvider::Codex,
        UsageReportProvider::All,
    ] {
        app.usage
            .set_report(build_usage_report(&all, 30, p, &now_utc()));
    }
}

fn small_app(w: u16, h: u16) -> App {
    let mut app = App::new(
        AppConfig::default(),
        Vec::new(),
        ColorMode::TrueColor,
        "/tmp".into(),
    );
    app.on_resize(w, h);
    app.start();
    app.overview.clock_override = Some(crate::overview::format::Clock {
        now: NOW,
        utc: true,
    });
    app
}

fn header(app: &mut App) -> String {
    let screen = app.screen;
    text(&draw(app, screen.width, screen.height))
        .lines()
        .next()
        .unwrap_or("")
        .to_string()
}

// ── the chip ────────────────────────────────────────────────────────────────

#[test]
fn the_chip_shows_the_highest_session_and_weekly() {
    let mut app = small_app(150, 20);
    load(&mut app, usage_state());
    let h = header(&mut app);
    assert!(h.contains("5h 29% \u{b7} wk 26%"), "{h}");
}

#[test]
fn the_chip_is_colored_by_severity_in_the_provider_color() {
    let mut app = small_app(150, 20);
    let mut state = usage_state();
    state.claude = vec![{
        let mut p = profile("default");
        p.windows = vec![
            win(
                "default",
                as_core::usage::types::LimitWindowKind::Session,
                85.0,
                None,
            ),
            win(
                "default",
                as_core::usage::types::LimitWindowKind::Weekly,
                55.0,
                None,
            ),
        ];
        p
    }];
    load(&mut app, state);
    let t = draw(&mut app, 150, 20);
    let buf = t.backend().buffer();
    let row: String = (0..150).map(|x| buf[(x, 0)].symbol().to_string()).collect();
    let find = |needle: &str| {
        row.find(needle)
            .map(|i| row[..i].chars().count() as u16)
            .unwrap()
    };
    let high = &buf[(find("85%"), 0)];
    let warn = &buf[(find("55%"), 0)];
    assert_eq!(high.fg, app.theme.error);
    assert!(
        high.modifier.contains(Modifier::BOLD),
        "a high window is bold"
    );
    assert_eq!(warn.fg, app.theme.starting);
    // The dot before the meters is Claude's terracotta, in every theme.
    let dot = &buf[(find("\u{25CF} 5h"), 0)];
    assert_eq!(dot.fg, Color::Rgb(0xE0, 0x8B, 0x6D));
    let light = crate::theme::Theme::new(as_core::ThemeId::Light, ColorMode::TrueColor).claude;
    assert_eq!(light, Color::Rgb(0xD9, 0x77, 0x57));
}

#[test]
fn stale_values_are_dimmed() {
    let mut app = small_app(150, 20);
    let mut state = usage_state();
    state.claude[0].fetched_at = Some(NOW - 20 * 60_000);
    state.claude[1].fetched_at = Some(NOW - 20 * 60_000);
    load(&mut app, state);
    let t = draw(&mut app, 150, 20);
    let buf = t.backend().buffer();
    let row: String = (0..150).map(|x| buf[(x, 0)].symbol().to_string()).collect();
    let x = row
        .find("29%")
        .map(|i| row[..i].chars().count() as u16)
        .unwrap();
    assert!(buf[(x, 0)].modifier.contains(Modifier::DIM));
}

#[test]
fn without_limits_the_chip_shows_tokens_today() {
    let mut app = small_app(150, 20);
    let mut state = usage_state();
    state.claude = vec![{
        let mut p = profile("work");
        p.status = ProfileStatus::Expired;
        p.windows.clear();
        p.fetched_at = None;
        p
    }];
    load(&mut app, state);
    let today = app
        .usage
        .summary(UsagePeriod::Today, "")
        .unwrap()
        .total_tokens;
    assert!(today > 0);
    let h = header(&mut app);
    assert!(
        h.contains(&format!(
            "{} today",
            crate::usage::format::format_compact_tokens(today)
        )),
        "{h}"
    );
    assert!(!h.contains("5h"), "{h}");
}

#[test]
fn the_chip_is_a_plain_usage_label_before_any_data_and_absent_without_usage() {
    let mut app = small_app(150, 20);
    assert!(
        !header(&mut app).contains("Usage"),
        "an old daemon: no chip"
    );
    let mut state = usage_state();
    state.claude.clear();
    app.usage.set_state(state);
    assert!(header(&mut app).contains("Usage"));
}

#[test]
fn a_narrow_header_drops_the_chip_instead_of_overlapping() {
    let mut app = small_app(60, 20);
    load(&mut app, usage_state());
    assert!(!header(&mut app).contains("5h"));
}

// ── the report overlay ──────────────────────────────────────────────────────

fn open_report(app: &mut App) {
    // prefix u
    app.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL));
    app.handle_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::NONE));
    assert!(matches!(app.modal, Some(Modal::Usage(_))));
}

fn with_codex(mut state: UsageState) -> UsageState {
    state.codex = Some(codex(NOW - 6 * 60_000, 10.0));
    state
}

#[test]
fn the_report_overlay_with_costs() {
    let mut app = small_app(110, 60);
    load(&mut app, with_codex(usage_state()));
    open_report(&mut app);
    let screen = text(&draw(&mut app, 110, 60));
    insta::assert_snapshot!(screen);
}

#[test]
fn the_report_overlay_without_costs() {
    let mut app = small_app(110, 60);
    app.config.usage.show_cost = false;
    load(&mut app, with_codex(usage_state()));
    open_report(&mut app);
    let screen = text(&draw(&mut app, 110, 60));
    assert!(
        !screen.contains('$'),
        "no dollar amounts when costs are hidden:\n{screen}"
    );
    assert!(!screen.contains("API-equivalent"));
    assert!(screen.contains("Token trend"));
    insta::assert_snapshot!(screen);
}

#[test]
fn the_overlay_explains_problems_and_dims_last_values() {
    let mut app = small_app(110, 70);
    let mut state = usage_state();
    state.claude[0].status = ProfileStatus::Expired;
    state.claude[0].fetched_at = Some(NOW - 2 * HOUR);
    state.claude[1].status = ProfileStatus::KeychainDenied;
    state.claude[1].windows.clear();
    state.indexing = IndexingState {
        indexing: true,
        progress: Some(UsageScanProgress {
            files_done: 1,
            files_total: 3,
            bytes_done: 40,
            bytes_total: 100,
        }),
        last_scan_at: None,
    };
    load(&mut app, state);
    open_report(&mut app);
    let s = text(&draw(&mut app, 110, 70));
    assert!(s.contains("Sign-in expired"), "{s}");
    assert!(s.contains("Start a Claude agent in this profile"), "{s}");
    assert!(s.contains("Showing the last values from 2 h ago."), "{s}");
    assert!(
        s.contains("Agent Studio could not read this sign-in from the Keychain."),
        "{s}"
    );
    assert!(s.contains("Indexing usage\u{2026} 40%"), "{s}");
}

#[test]
fn low_filter_keeps_only_windows_near_their_limit() {
    let mut app = small_app(110, 60);
    let mut state = usage_state();
    state.claude = vec![{
        let mut p = profile("default");
        p.windows = vec![
            win(
                "default",
                as_core::usage::types::LimitWindowKind::Session,
                85.0,
                Some(NOW + 600_000),
            ),
            win(
                "default",
                as_core::usage::types::LimitWindowKind::Weekly,
                40.0,
                None,
            ),
        ];
        p
    }];
    load(&mut app, state);
    open_report(&mut app);
    ch(&mut app, 'f');
    let s = text(&draw(&mut app, 110, 60));
    assert!(s.contains("Current session (5h)"), "{s}");
    assert!(!s.contains("Weekly \u{b7} all models"), "{s}");
    ch(&mut app, 'f');
    let s = text(&draw(&mut app, 110, 60));
    assert!(s.contains("Weekly \u{b7} all models"), "{s}");
    // Nothing near a limit: a quiet line.
    load(&mut app, usage_state());
    ch(&mut app, 'f');
    assert!(text(&draw(&mut app, 110, 60)).contains("No plan window is near its limit."));
}

#[test]
fn the_overlay_keys_scroll_switch_provider_and_close() {
    let mut app = small_app(110, 24);
    load(&mut app, with_codex(usage_state()));
    open_report(&mut app);
    let Some(Modal::Usage(o)) = &app.modal else {
        panic!()
    };
    assert_eq!(
        (o.provider, o.trend, o.low_only),
        (UsageReportProvider::All, 0, false)
    );
    ch(&mut app, 'j');
    ch(&mut app, 'j');
    draw(&mut app, 110, 24);
    let Some(Modal::Usage(o)) = &app.modal else {
        panic!()
    };
    assert_eq!(o.scroll, 2);
    press(&mut app, KeyCode::Right);
    ch(&mut app, 'd');
    let Some(Modal::Usage(o)) = &app.modal else {
        panic!()
    };
    assert_eq!(
        (o.provider, o.trend_days()),
        (UsageReportProvider::Claude, 30)
    );
    ch(&mut app, '2');
    let Some(Modal::Usage(o)) = &app.modal else {
        panic!()
    };
    assert_eq!(o.provider, UsageReportProvider::Codex);
    // scrolling past the end sticks at the end instead of showing a blank page
    ch(&mut app, 'G');
    draw(&mut app, 110, 24);
    let Some(Modal::Usage(o)) = &app.modal else {
        panic!()
    };
    assert!(o.scroll < 400, "{}", o.scroll);
    press(&mut app, KeyCode::Esc);
    assert!(app.modal.is_none());
}

#[test]
fn the_provider_toggle_switches_the_report() {
    let mut app = small_app(110, 70);
    load(&mut app, with_codex(usage_state()));
    open_report(&mut app);
    ch(&mut app, '2');
    let codex = text(&draw(&mut app, 110, 70));
    assert!(codex.contains("gpt-5-codex"), "{codex}");
    assert!(!codex.contains("opus-5-5"), "{codex}");
    ch(&mut app, '1');
    let claude = text(&draw(&mut app, 110, 70));
    assert!(
        claude.contains("opus-5-5") && !claude.contains("gpt-5-codex"),
        "{claude}"
    );
}

// ── the Overview card ───────────────────────────────────────────────────────

/// The usage card of the Overview, cut out of the screen (the sidebar and the next card left out).
fn card_text(app: &mut App, w: u16, h: u16) -> String {
    let s = text(&draw(app, w, h));
    let lines: Vec<&str> = s.lines().collect();
    let start = lines
        .iter()
        .position(|l| l.contains("Usage & limits"))
        .expect("a usage card");
    let col = lines[start]
        .chars()
        .position(|c| c == '\u{256D}' || c == '\u{250F}')
        .expect("the card's corner");
    let end = lines
        .iter()
        .skip(start)
        .position(|l| matches!(l.chars().nth(col), Some('\u{2570}' | '\u{2517}')))
        .map_or(lines.len() - 1, |i| start + i);
    lines[start..=end]
        .iter()
        .map(|l| {
            let cut: String = l.chars().skip(col).collect();
            // The scrollbar column of the page sits after the card.
            cut.trim_end_matches('\u{2503}').trim_end().to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn the_overview_has_a_usage_card_on_top() {
    let mut app = overview_app(170, 64);
    load(&mut app, with_codex(usage_state()));
    let card = card_text(&mut app, 170, 64);
    insta::assert_snapshot!(card);
    let full = text(&draw(&mut app, 170, 64));
    assert!(
        full.lines().position(|l| l.contains("Usage & limits"))
            < full.lines().position(|l| l.contains("Counts"))
    );
}

#[test]
fn the_overview_usage_card_in_one_column_and_without_costs() {
    let mut app = overview_app(86, 90);
    app.config.usage.show_cost = false;
    load(&mut app, usage_state());
    let card = card_text(&mut app, 86, 90);
    assert!(!card.contains('$'), "{card}");
    insta::assert_snapshot!(card);
}

#[test]
fn there_is_no_usage_card_without_usage_state() {
    let mut app = overview_app(170, 64);
    assert!(!text(&draw(&mut app, 170, 64)).contains("Usage & limits"));
    // and Tab skips it
    for _ in 0..8 {
        press(&mut app, KeyCode::Tab);
        assert_ne!(app.overview.focus, crate::overview::Card::Usage);
    }
}

#[test]
fn t_changes_the_card_period_when_the_card_has_focus_and_enter_opens_the_report() {
    let mut app = overview_app(170, 64);
    load(&mut app, usage_state());
    app.overview.focus = crate::overview::Card::Usage;
    assert_eq!(app.usage.card_period.0, UsagePeriod::Week);
    ch(&mut app, 't');
    assert_eq!(app.usage.card_period.0, UsagePeriod::Month);
    assert!(card_text(&mut app, 170, 64).contains("Tokens \u{b7} month"));
    press(&mut app, KeyCode::Enter);
    assert!(matches!(app.modal, Some(Modal::Usage(_))));
    press(&mut app, KeyCode::Esc);
    ch(&mut app, 'u');
    assert!(matches!(app.modal, Some(Modal::Usage(_))));
}

// ── what is fetched, and when ───────────────────────────────────────────────

fn loads(fx: &[Effect]) -> Vec<String> {
    fx.iter()
        .filter_map(|e| match e {
            Effect::LoadUsageSummary { period, project } => {
                Some(format!("summary {period:?} {project:?}"))
            }
            Effect::LoadUsageReport { provider, days } => {
                Some(format!("report {provider:?} {days}"))
            }
            _ => None,
        })
        .collect()
}

#[test]
fn start_asks_for_the_usage_state() {
    let mut app = App::new(
        AppConfig::default(),
        Vec::new(),
        ColorMode::TrueColor,
        "/tmp".into(),
    );
    app.on_resize(100, 30);
    assert!(app.start().contains(&Effect::LoadUsageState));
}

#[test]
fn the_overlay_fetches_everything_once_and_reports_visibility() {
    let mut app = small_app(110, 40);
    app.usage.set_state(usage_state());
    let first = app.usage_effects();
    assert_eq!(
        loads(&first),
        ["summary Today \"\""],
        "the chip's fallback only"
    );
    assert!(app.usage_effects().is_empty(), "nothing is asked twice");
    open_report(&mut app);
    let fx = app.usage_effects();
    let mut got = loads(&fx);
    got.sort();
    assert_eq!(
        got,
        ["report All 30", "summary Month \"\"", "summary Week \"\""],
        "Today is already asked"
    );
    assert!(fx.contains(&Effect::UsageVisible(true)));
    assert!(app.usage_effects().is_empty());
    // another provider: another report
    ch(&mut app, '2');
    assert_eq!(loads(&app.usage_effects()), ["report Codex 30"]);
    press(&mut app, KeyCode::Esc);
    assert_eq!(app.usage_effects(), [Effect::UsageVisible(false)]);
}

#[test]
fn the_overview_reports_visibility_and_asks_for_the_card_period_of_the_project() {
    let mut app = small_app(170, 50);
    app.usage.set_state(usage_state());
    app.handle_msg(Msg::Overview(Box::default()));
    let fx = app.set_view(ViewMode::Overview);
    let _ = fx;
    let fx = app.usage_effects();
    assert!(fx.contains(&Effect::UsageVisible(true)));
    assert!(
        loads(&fx).contains(&"summary Week \"\"".to_string()),
        "{fx:?}"
    );
    let fx = app.set_view(ViewMode::Chat);
    let _ = fx;
    assert!(app.usage_effects().contains(&Effect::UsageVisible(false)));
}

#[test]
fn new_data_refetches_what_is_on_screen_and_keeps_the_old_values_meanwhile() {
    let mut app = small_app(110, 40);
    load(&mut app, usage_state());
    open_report(&mut app);
    // everything already loaded at version 1
    app.usage_effects();
    assert!(loads(&app.usage_effects()).is_empty());
    let mut next = usage_state();
    next.data_version = 2;
    app.handle_event(Event::UsageChanged(Box::new(next)));
    let mut got = loads(&app.usage_effects());
    got.sort();
    assert_eq!(got.len(), 4, "{got:?}");
    assert!(
        app.usage.summary(UsagePeriod::Today, "").is_some(),
        "still shown"
    );
}

#[test]
fn refresh_asks_the_daemon_once_until_it_answers() {
    let mut app = small_app(110, 40);
    app.usage.set_state(usage_state());
    open_report(&mut app);
    assert_eq!(ch(&mut app, 'r'), [Effect::RefreshUsage]);
    assert!(ch(&mut app, 'r').is_empty());
    app.handle_msg(Msg::UsageState(Box::new(usage_state())));
    assert!(!app.usage.refreshing);
}

#[test]
fn a_daemon_without_usage_gets_a_hint_instead_of_an_overlay() {
    let mut app = small_app(110, 40);
    app.handle_msg(Msg::UsageFailed("unknown request".into()));
    app.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL));
    app.handle_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::NONE));
    assert!(app.modal.is_none());
    assert!(app.toasts.iter().any(|t| t.title == "Usage"));
}

#[test]
fn a_usage_limit_notification_is_a_warning_toast() {
    let mut app = small_app(110, 40);
    app.handle_event(Event::Notification(as_proto::StudioNotification {
        id: "n".into(),
        kind: as_proto::NotificationKind::UsageLimit,
        title: "Claude session limit: 85% used".into(),
        body: "Resets in 42 min".into(),
        agent_id: None,
        timestamp: "2026-10-22T14:30:00Z".into(),
    }));
    assert_eq!(app.toasts.last().unwrap().kind, crate::app::ToastKind::Warn);
}

#[test]
fn palette_commands_refresh_and_toggle_settings() {
    let mut app = small_app(110, 40);
    app.usage.set_state(usage_state());
    let entries = crate::palette::build_entries(&app.keymap, &app.agents, app.view);
    let labels: Vec<_> = entries.iter().map(|e| e.label.as_str()).collect();
    for l in [
        "Usage Report",
        "Refresh Usage",
        "Show / Hide Usage Costs",
        "Usage Alerts On / Off",
    ] {
        assert!(labels.contains(&l), "{l}");
    }
    let report = entries.iter().find(|e| e.label == "Usage Report").unwrap();
    assert_eq!(report.hint, "Ctrl+A u");
    let fx = app.run_palette_action(crate::palette::PaletteAction::Command(
        crate::keymap::Command::ToggleUsageCosts,
    ));
    assert_eq!(
        fx,
        [Effect::PatchConfig(
            serde_json::json!({"usage": {"showCost": false}})
        )]
    );
    let fx = app.run_palette_action(crate::palette::PaletteAction::Command(
        crate::keymap::Command::ToggleUsageAlerts,
    ));
    assert_eq!(
        fx,
        [Effect::PatchConfig(
            serde_json::json!({"usage": {"alertsEnabled": true}})
        )]
    );
}
