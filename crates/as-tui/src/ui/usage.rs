//! Usage surfaces (Hydra `src/components/Usage/*`, `UsageDashboard`, `Overview/UsageCard`): the
//! header chip, the report overlay (`prefix u`) and the Overview card.

use as_core::usage::limits::{LimitSeverity, limit_severity};
use as_core::usage::report::UsageReportProvider;
use as_core::usage::types::{
    ProfileStatus, UsagePeriod, UsageProvider, UsageState, UsageSummary, indexing_percent,
};
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Bar, BarChart, BarGroup, Block, BorderType, Borders, Cell, Clear, LineGauge, Row, Table, Widget,
};
use unicode_width::UnicodeWidthStr;

use crate::app::App;
use crate::overview::format::{Clock, clip};
use crate::theme::Theme;
use crate::usage::format::{
    format_age, format_change, format_compact_tokens, format_exact_tokens, format_resets,
    format_updated, format_usd,
};
use crate::usage::model::{
    LimitGroup, LimitTile, TimedWindow, build_limit_groups, build_limit_tiles, chip_windows,
    is_stale, low_windows, newest_fetch, profile_problem,
};
use crate::usage::{
    REPORT_TABS, UsageOverlay, models_over, period_label, projects_over, report_tab_label,
};

// ── colors ──────────────────────────────────────────────────────────────────

pub fn provider_color(t: &Theme, p: UsageProvider) -> Color {
    match p {
        UsageProvider::Claude => t.claude,
        UsageProvider::Codex => t.codex,
    }
}

/// Green below 50 %, amber below 80 %, red from there (Hydra `limitSeverity`).
pub fn severity_color(t: &Theme, used_percent: f64) -> Color {
    match limit_severity(used_percent) {
        LimitSeverity::Ok => t.running,
        LimitSeverity::Warn => t.starting,
        LimitSeverity::High => t.error,
    }
}

fn muted(t: &Theme) -> Style {
    Style::default().fg(t.text_muted)
}
fn secondary(t: &Theme) -> Style {
    Style::default().fg(t.text_secondary)
}
fn bold(t: &Theme) -> Style {
    Style::default().fg(t.text).add_modifier(Modifier::BOLD)
}

fn resets_text(clock: &Clock, resets_at: Option<i64>) -> String {
    if clock.utc {
        format_resets(resets_at, clock.now, &chrono::Utc)
    } else {
        format_resets(resets_at, clock.now, &chrono::Local)
    }
}

fn width_of(spans: &[Span]) -> usize {
    spans.iter().map(|s| s.content.width()).sum()
}

// ── header chip ─────────────────────────────────────────────────────────────

/// The header's usage chip: `● 5h 42% · wk 18%` colored by severity (dimmed when the values are
/// older than 15 minutes), tokens today when no limits are known, plain `Usage` before that.
/// Empty when the daemon does not know usage.
pub fn chip_spans(app: &App) -> Vec<Span<'static>> {
    let t = &app.theme;
    let Some(state) = app.usage.state.as_ref() else {
        return Vec::new();
    };
    let now = app.overview.clock().now;
    let w = chip_windows(Some(state), app.config.usage.chip_provider);
    let mut spans: Vec<Span<'static>> = Vec::new();
    let meters: Vec<(&str, &TimedWindow)> = [("5h", w.session.as_ref()), ("wk", w.weekly.as_ref())]
        .into_iter()
        .filter_map(|(l, w)| w.map(|w| (l, w)))
        .collect();
    if let Some((_, first)) = meters.first() {
        spans.push(Span::styled(
            "\u{25CF} ",
            Style::default().fg(provider_color(t, first.window.provider)),
        ));
        for (i, (label, timed)) in meters.iter().enumerate() {
            if i > 0 {
                spans.push(Span::styled(" \u{b7} ", muted(t)));
            }
            let percent = timed.window.used_percent.round();
            let mut value = Style::default().fg(severity_color(t, timed.window.used_percent));
            if limit_severity(timed.window.used_percent) == LimitSeverity::High {
                value = value.add_modifier(Modifier::BOLD);
            }
            let mut name = muted(t);
            if is_stale(timed.at, now) {
                value = value.add_modifier(Modifier::DIM);
                name = name.add_modifier(Modifier::DIM);
            }
            spans.push(Span::styled(format!("{label} "), name));
            spans.push(Span::styled(format!("{percent:.0}%"), value));
        }
        return spans;
    }
    let today = app.usage.summary(UsagePeriod::Today, "");
    match today.filter(|s| s.total_tokens > 0) {
        Some(s) => {
            spans.push(Span::styled("\u{25CF} ", Style::default().fg(t.claude)));
            spans.push(Span::styled(
                format!("{} today", format_compact_tokens(s.total_tokens)),
                secondary(t),
            ));
        }
        None => spans.push(Span::styled("Usage", muted(t))),
    }
    spans
}

// ── building blocks ─────────────────────────────────────────────────────────

/// How a plan-window row looks.
struct RowOpts<'a> {
    dimmed: bool,
    label_w: usize,
    /// Text after the reset time (`est. 6 min`).
    note: Option<&'a str>,
}

/// One plan window on one row: label, gauge, `42%`, reset time.
fn limit_row(
    t: &Theme,
    clock: &Clock,
    timed: &TimedWindow,
    opts: &RowOpts,
    area: Rect,
    buf: &mut Buffer,
) {
    let RowOpts {
        dimmed,
        label_w,
        note,
    } = *opts;
    if area.width < 12 || area.height == 0 {
        return;
    }
    let w = &timed.window;
    let dim = |s: Style| {
        if dimmed {
            s.add_modifier(Modifier::DIM)
        } else {
            s
        }
    };
    let percent = format!("{:>3.0}%", w.used_percent.round());
    let mut resets = resets_text(clock, w.resets_at);
    if let Some(note) = note {
        resets = if resets.is_empty() {
            note.to_string()
        } else {
            format!("{resets} \u{b7} {note}")
        };
    }
    let width = area.width as usize;
    // The reset time matters more than a long label: the label gives way before it is dropped.
    let wanted = resets.width() + 1;
    let (label_w, resets_w) = if width >= label_w + 1 + 10 + 5 + wanted {
        (label_w, wanted)
    } else if width >= 14 + 1 + 10 + 5 + wanted {
        (width - (1 + 10 + 5 + wanted), wanted)
    } else {
        (label_w.min(width.saturating_sub(1 + 5 + 4)), 0)
    };
    let right_w = 5 + resets_w;
    let gauge_w = width.saturating_sub(label_w + right_w + 1).max(4);
    let label = clip(&w.label, label_w);
    buf.set_string(
        area.x,
        area.y,
        format!("{label:<label_w$}"),
        dim(secondary(t)),
    );
    let gauge_area = Rect::new(area.x + label_w as u16 + 1, area.y, gauge_w as u16, 1);
    let color = severity_color(t, w.used_percent);
    LineGauge::default()
        .ratio((w.used_percent / 100.0).clamp(0.0, 1.0))
        .label("")
        .filled_symbol("\u{2501}")
        .unfilled_symbol("\u{2500}")
        .filled_style(dim(Style::default().fg(color)))
        .unfilled_style(Style::default().fg(t.border))
        .render(gauge_area, buf);
    let px = gauge_area.right();
    buf.set_string(
        px,
        area.y,
        &percent,
        dim(Style::default().fg(color).add_modifier(Modifier::BOLD)),
    );
    if resets_w > 0 {
        buf.set_string(
            px + 5,
            area.y,
            format!(" {}", clip(&resets, resets_w - 1)),
            dim(muted(t)),
        );
    }
}

/// `[Claude] Codex  Both`: the active one inverted.
fn segmented(t: &Theme, labels: &[&str], active: usize) -> Vec<Span<'static>> {
    let mut out = Vec::new();
    for (i, l) in labels.iter().enumerate() {
        if i > 0 {
            out.push(Span::raw(" "));
        }
        out.push(if i == active {
            Span::styled(
                format!(" {l} "),
                Style::default()
                    .fg(t.accent_text)
                    .bg(t.accent)
                    .add_modifier(Modifier::BOLD),
            )
        } else {
            Span::styled(format!(" {l} "), secondary(t))
        });
    }
    out
}

/// `left` ... `right` on one line.
fn spread(left: Vec<Span<'static>>, right: Vec<Span<'static>>, width: usize) -> Line<'static> {
    let (lw, rw) = (width_of(&left), width_of(&right));
    let mut out = left;
    out.push(Span::raw(" ".repeat(width.saturating_sub(lw + rw).max(1))));
    out.extend(right);
    Line::from(out)
}

fn section(t: &Theme, title: &str, right: Vec<Span<'static>>, width: usize) -> Line<'static> {
    spread(vec![Span::styled(title.to_string(), bold(t))], right, width)
}

/// A bar chart of `values` (one bar per slot) filling `area`.
fn bars(t: &Theme, values: &[u64], color: Color, area: Rect, buf: &mut Buffer) {
    if values.is_empty() || area.width == 0 || area.height == 0 {
        return;
    }
    let n = values.len() as u16;
    let gap = u16::from(n <= 14 && area.width >= n * 3);
    let bar_w = (area.width.saturating_sub(gap * n.saturating_sub(1)) / n).clamp(1, 8);
    let bars: Vec<Bar> = values
        .iter()
        .map(|v| {
            Bar::default()
                .value(*v)
                .text_value(String::new())
                .style(Style::default().fg(color))
        })
        .collect();
    BarChart::default()
        .data(BarGroup::new(bars))
        .bar_width(bar_w)
        .bar_gap(gap)
        .max(values.iter().copied().max().unwrap_or(1).max(1))
        .style(Style::default().bg(t.surface_raised))
        .render(area, buf);
}

fn legend(t: &Theme, summary: &UsageSummary, now_width: usize) -> Line<'static> {
    let mut spans = Vec::new();
    for p in &summary.by_provider {
        spans.push(Span::styled(
            "\u{25CF} ",
            Style::default().fg(provider_color(t, p.provider)),
        ));
        spans.push(Span::styled(
            format!("{} ", p.provider.label()),
            secondary(t),
        ));
        spans.push(Span::styled(
            if p.total_tokens == 0 {
                "no usage".to_string()
            } else {
                format_compact_tokens(p.total_tokens)
            },
            bold(t),
        ));
        spans.push(Span::raw("   "));
    }
    let _ = now_width;
    Line::from(spans)
}

fn cost_text(summary: &UsageSummary) -> String {
    format!(
        "{}{} API-equivalent",
        if summary.cost_partial {
            "\u{2265}"
        } else {
            "~"
        },
        format_usd(summary.cost_usd)
    )
}

fn hero(t: &Theme, summary: &UsageSummary, show_cost: bool, with_change: bool) -> Line<'static> {
    let mut spans = vec![
        Span::styled(
            format_compact_tokens(summary.total_tokens),
            Style::default().fg(t.text).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" tokens", secondary(t)),
    ];
    if show_cost {
        spans.push(Span::styled(
            format!(" \u{b7} {}", cost_text(summary)),
            secondary(t),
        ));
    }
    if with_change && let Some(c) = format_change(summary.tokens_change_pct, summary.period, "") {
        spans.push(Span::styled(format!(" \u{b7} {c}"), muted(t)));
    }
    Line::from(spans)
}

fn indexing_line(t: &Theme, state: &UsageState) -> Option<Line<'static>> {
    if !state.indexing.indexing {
        return None;
    }
    let pct = indexing_percent(&state.indexing)
        .map(|p| format!(" {p}%"))
        .unwrap_or_default();
    Some(Line::styled(
        format!("Indexing usage\u{2026}{pct}"),
        Style::default().fg(t.starting),
    ))
}

/// The slot values of a summary as one bar each (tokens of both providers).
fn slot_values(summary: &UsageSummary) -> Vec<u64> {
    summary
        .slots
        .iter()
        .map(|s| s.tokens_by_provider.iter().sum())
        .collect()
}

/// Color of a chart: the provider with most tokens in the summary.
fn dominant_color(t: &Theme, summary: &UsageSummary) -> Color {
    let codex = summary.by_provider.get(1).map_or(0, |p| p.total_tokens);
    let claude = summary.by_provider.first().map_or(0, |p| p.total_tokens);
    if codex > claude { t.codex } else { t.claude }
}

fn axis_line(t: &Theme, summary: &UsageSummary, width: usize) -> Line<'static> {
    let first = summary
        .slots
        .first()
        .map_or("", |s| s.label.as_str())
        .to_string();
    let last = summary
        .slots
        .last()
        .map_or("", |s| s.label.as_str())
        .to_string();
    spread(
        vec![Span::styled(first, muted(t))],
        vec![Span::styled(last, muted(t))],
        width,
    )
}

// ── Overview card ───────────────────────────────────────────────────────────

const MAX_TILES: usize = 8;
const CARD_CHART_ROWS: u16 = 4;

/// The card is shown once the daemon has said anything about usage.
pub fn card_visible(app: &App) -> bool {
    app.usage.state.is_some()
}

fn card_tiles(app: &App) -> Vec<LimitTile> {
    app.usage
        .state
        .as_ref()
        .map(build_limit_tiles)
        .unwrap_or_default()
}

/// Rows of the left column (plan limits).
fn limit_column_rows(tiles: &[LimitTile]) -> usize {
    let shown = tiles.len().min(MAX_TILES);
    let extra = usize::from(tiles.len() > MAX_TILES);
    let problems: usize = tiles
        .iter()
        .take(MAX_TILES)
        .map(|t| {
            t.problem
                .as_ref()
                .map_or(0, |p| 1 + usize::from(p.hint.is_some()))
        })
        .sum();
    // Problem tiles take their own lines instead of the one a window takes.
    let windows = tiles
        .iter()
        .take(MAX_TILES)
        .filter(|t| t.problem.is_none())
        .count();
    let _ = shown;
    1 + windows.max(usize::from(tiles.is_empty())) + problems + extra
}

fn token_column_rows(indexing: bool) -> usize {
    // title, hero, legend, chart, axis, + the indexing banner
    1 + 1 + 1 + usize::from(CARD_CHART_ROWS) + 1 + usize::from(indexing)
}

/// Inner height of the card at `inner_w` columns.
pub fn card_height(app: &App, inner_w: u16) -> u16 {
    let tiles = card_tiles(app);
    let left = limit_column_rows(&tiles);
    let right = token_column_rows(
        app.usage
            .state
            .as_ref()
            .is_some_and(|s| s.indexing.indexing),
    );
    let rows = if inner_w >= 96 {
        left.max(right)
    } else {
        left + right + 1
    };
    u16::try_from(rows).unwrap_or(u16::MAX)
}

pub fn card_title(app: &App) -> String {
    match app.usage.state.as_ref() {
        Some(s) => format!(
            " Usage & limits \u{b7} updated {} ",
            format_updated(newest_fetch(s), app.overview.clock().now)
        ),
        None => " Usage & limits ".to_string(),
    }
}

pub fn card_body(app: &App, inner: Rect, buf: &mut Buffer) {
    let t = &app.theme;
    let clock = app.overview.clock();
    let Some(state) = app.usage.state.as_ref() else {
        buf.set_string(inner.x, inner.y, "Loading usage\u{2026}", muted(t));
        return;
    };
    let tiles = card_tiles(app);
    let wide = inner.width >= 96;
    let left_w = if wide {
        inner.width * 11 / 20
    } else {
        inner.width
    };
    let left = Rect::new(
        inner.x,
        inner.y,
        left_w.saturating_sub(u16::from(wide) * 2),
        inner.height,
    );
    let left_rows = limit_column_rows(&tiles) as u16;
    let right = if wide {
        Rect::new(
            inner.x + left_w,
            inner.y,
            inner.width - left_w,
            inner.height,
        )
    } else {
        Rect::new(
            inner.x,
            inner.y + left_rows + 1,
            inner.width,
            inner.height.saturating_sub(left_rows + 1),
        )
    };
    limits_column(app, &tiles, &clock, left, buf);
    tokens_column(app, state, right, buf);
}

fn limits_column(app: &App, tiles: &[LimitTile], clock: &Clock, area: Rect, buf: &mut Buffer) {
    let t = &app.theme;
    let mut y = area.y;
    let bottom = area.bottom();
    let put = |y: &mut u16, line: Line<'static>, buf: &mut Buffer| {
        if *y < bottom {
            line.render(Rect::new(area.x, *y, area.width, 1), buf);
            *y += 1;
        }
    };
    put(&mut y, Line::styled("Plan limits", bold(t)), buf);
    if tiles.is_empty() {
        put(&mut y, Line::styled("No plan limits found.", muted(t)), buf);
        return;
    }
    let label_w = (area.width as usize * 2 / 5).clamp(14, 36);
    for tile in tiles.iter().take(MAX_TILES) {
        if let Some(problem) = &tile.problem {
            put(
                &mut y,
                Line::from(vec![
                    Span::styled(
                        "\u{25CF} ",
                        Style::default().fg(provider_color(t, tile.provider)),
                    ),
                    Span::styled(tile.label.clone(), secondary(t)),
                    Span::styled(
                        format!("  {}", problem.title),
                        Style::default().fg(t.starting),
                    ),
                ]),
                buf,
            );
            if let Some(hint) = problem.hint {
                put(
                    &mut y,
                    Line::styled(clip(&format!("  {hint}"), area.width as usize), muted(t)),
                    buf,
                );
            }
            continue;
        }
        let Some(timed) = &tile.timed else { continue };
        if y >= bottom {
            break;
        }
        let note = tile.estimated.then(|| {
            format!(
                "est. {}",
                timed
                    .at
                    .map_or(String::new(), |a| format_age(clock.now - a))
            )
        });
        let mut shown = timed.clone();
        shown.window.label = tile.label.clone();
        let dim = tile.dimmed || (tile.estimated && is_stale(tile.at, clock.now));
        limit_row(
            t,
            clock,
            &shown,
            &RowOpts {
                dimmed: dim,
                label_w,
                note: note.as_deref(),
            },
            Rect::new(area.x, y, area.width, 1),
            buf,
        );
        y += 1;
    }
    if tiles.len() > MAX_TILES {
        put(
            &mut y,
            Line::styled(
                format!("+{} more in the report (prefix u)", tiles.len() - MAX_TILES),
                muted(t),
            ),
            buf,
        );
    }
}

fn tokens_column(app: &App, state: &UsageState, area: Rect, buf: &mut Buffer) {
    let t = &app.theme;
    if area.height == 0 || area.width < 10 {
        return;
    }
    let period = app.usage.card_period.0;
    let project = app.overview.effective_project().unwrap_or("").to_string();
    let where_ = if project.is_empty() {
        String::new()
    } else {
        format!(
            " \u{b7} {}",
            crate::overview::format::project_name(&project)
        )
    };
    let mut y = area.y;
    let line = |y: &mut u16, l: Line<'static>, buf: &mut Buffer| {
        if *y < area.bottom() {
            l.render(Rect::new(area.x, *y, area.width, 1), buf);
            *y += 1;
        }
    };
    let title = format!(
        "Tokens \u{b7} {}{where_}",
        period_label(period).to_lowercase()
    );
    line(
        &mut y,
        section(
            t,
            &title,
            vec![Span::styled("t period", muted(t))],
            area.width as usize,
        ),
        buf,
    );
    if let Some(l) = indexing_line(t, state) {
        line(&mut y, l, buf);
    }
    match app.usage.summary(period, &project) {
        None => line(&mut y, Line::styled("Loading\u{2026}", muted(t)), buf),
        Some(s) => {
            line(&mut y, hero(t, s, app.config.usage.show_cost, true), buf);
            line(&mut y, legend(t, s, area.width as usize), buf);
            let chart = Rect::new(
                area.x,
                y,
                area.width,
                CARD_CHART_ROWS.min(area.bottom().saturating_sub(y)),
            );
            bars(t, &slot_values(s), dominant_color(t, s), chart, buf);
            y += chart.height;
            line(&mut y, axis_line(t, s, area.width as usize), buf);
        }
    }
}

// ── report overlay ──────────────────────────────────────────────────────────

/// Rows the off-screen page can have (the report is far shorter).
const PAGE_ROWS: u16 = 400;
const TREND_ROWS: u16 = 7;
const TABLE_ROWS: usize = 8;

struct Page {
    buf: Buffer,
    y: u16,
    width: u16,
}

impl Page {
    fn new(width: u16) -> Self {
        Self {
            buf: Buffer::empty(Rect::new(0, 0, width, PAGE_ROWS)),
            y: 0,
            width,
        }
    }
    fn line(&mut self, line: Line<'static>) {
        if self.y < PAGE_ROWS {
            line.render(Rect::new(0, self.y, self.width, 1), &mut self.buf);
            self.y += 1;
        }
    }
    fn text(&mut self, text: &str, style: Style) {
        self.line(Line::styled(clip(text, self.width as usize), style));
    }
    fn gap(&mut self) {
        self.y = (self.y + 1).min(PAGE_ROWS);
    }
    fn rect(&mut self, rows: u16) -> Rect {
        let r = Rect::new(
            0,
            self.y.min(PAGE_ROWS - 1),
            self.width,
            rows.min(PAGE_ROWS.saturating_sub(self.y)),
        );
        self.y = (self.y + rows).min(PAGE_ROWS);
        r
    }
}

fn limits_section(app: &App, overlay: &UsageOverlay, state: &UsageState, page: &mut Page) {
    let t = &app.theme;
    let clock = app.overview.clock();
    let w = page.width as usize;
    page.line(section(
        t,
        "Plan limits",
        segmented(t, &["All", "Low"], usize::from(overlay.low_only)),
        w,
    ));
    let groups = build_limit_groups(state);
    if groups.is_empty() {
        page.text("No plan limits yet.", muted(t));
        return;
    }
    let low = low_windows(&groups, &app.config.usage);
    let visible: Vec<&LimitGroup> = if overlay.low_only {
        groups
            .iter()
            .filter(|g| {
                g.windows
                    .iter()
                    .any(|t| low.contains(&(g.key.clone(), t.window.id.clone())))
            })
            .collect()
    } else {
        groups.iter().collect()
    };
    if visible.is_empty() {
        page.text("No plan window is near its limit.", muted(t));
        return;
    }
    let label_w = 24usize;
    for g in visible {
        let profile = g.profile.as_ref();
        let problem = profile.and_then(profile_problem);
        let dimmed = profile.is_some_and(|p| p.status != ProfileStatus::Ok);
        let mut head = vec![
            Span::styled(
                "\u{25CF} ",
                Style::default().fg(provider_color(t, g.provider)),
            ),
            Span::styled(g.name.clone(), bold(t)),
        ];
        if !g.subtitle.is_empty() {
            head.push(Span::styled(format!(" {}", g.subtitle), secondary(t)));
        }
        let mut source = Vec::new();
        if let (UsageProvider::Codex, Some(at)) = (g.provider, g.codex_at) {
            source.push(Span::styled(
                format!("from session log \u{b7} {} ", format_age(clock.now - at)),
                muted(t),
            ));
            source.push(Span::styled(
                " ESTIMATED ",
                Style::default().fg(t.starting).add_modifier(Modifier::BOLD),
            ));
        } else if profile.is_some_and(|p| p.status == ProfileStatus::Ok) {
            source.push(Span::styled("live", Style::default().fg(t.running)));
        }
        page.line(spread(head, source, w));
        if !overlay.low_only
            && let Some(p) = &problem
        {
            page.line(Line::styled(
                format!("  {}", p.title),
                Style::default().fg(t.starting),
            ));
            if let Some(h) = p.hint {
                page.line(Line::styled(format!("  {h}"), muted(t)));
            }
        }
        for timed in &g.windows {
            if overlay.low_only && !low.contains(&(g.key.clone(), timed.window.id.clone())) {
                continue;
            }
            let r = page.rect(1);
            limit_row(
                t,
                &clock,
                timed,
                &RowOpts {
                    dimmed,
                    label_w,
                    note: None,
                },
                Rect::new(2, r.y, r.width.saturating_sub(2), 1),
                &mut page.buf,
            );
        }
        if !overlay.low_only
            && dimmed
            && let Some(p) = profile
            && !p.windows.is_empty()
            && let Some(at) = p.fetched_at
        {
            page.text(
                &format!(
                    "  Showing the last values from {} ago.",
                    format_age(clock.now - at)
                ),
                muted(t),
            );
        }
    }
}

fn period_cell(app: &App, period: UsagePeriod, provider: Option<UsageProvider>) -> String {
    match app.usage.summary(period, "") {
        None => "\u{2026}".to_string(),
        Some(s) => {
            let tokens = match provider {
                Some(p) => s.by_provider.get(p.index()).map_or(0, |p| p.total_tokens),
                None => s.total_tokens,
            };
            if tokens == 0 {
                "\u{2013}".to_string()
            } else {
                format_compact_tokens(tokens)
            }
        }
    }
}

fn tokens_section(app: &App, state: &UsageState, page: &mut Page) {
    let t = &app.theme;
    let show_cost = app.config.usage.show_cost;
    let w = page.width as usize;
    page.line(section(t, "Tokens", Vec::new(), w));
    if let Some(l) = indexing_line(t, state) {
        page.line(l);
    }
    let periods = [UsagePeriod::Today, UsagePeriod::Week, UsagePeriod::Month];
    let mut rows: Vec<Row> = Vec::new();
    for p in [UsageProvider::Claude, UsageProvider::Codex] {
        rows.push(Row::new(vec![
            Cell::from(Line::from(vec![
                Span::styled("\u{25CF} ", Style::default().fg(provider_color(t, p))),
                Span::styled(p.label(), secondary(t)),
            ])),
            Cell::from(Line::from(period_cell(app, periods[0], Some(p))).right_aligned()),
            Cell::from(Line::from(period_cell(app, periods[1], Some(p))).right_aligned()),
            Cell::from(Line::from(period_cell(app, periods[2], Some(p))).right_aligned()),
        ]));
    }
    rows.push(
        Row::new(vec![
            Cell::from("Total"),
            Cell::from(Line::from(period_cell(app, periods[0], None)).right_aligned()),
            Cell::from(Line::from(period_cell(app, periods[1], None)).right_aligned()),
            Cell::from(Line::from(period_cell(app, periods[2], None)).right_aligned()),
        ])
        .style(bold(t)),
    );
    if show_cost {
        let cost = |p: UsagePeriod| {
            app.usage
                .summary(p, "")
                .map_or("\u{2026}".to_string(), |s| {
                    format!(
                        "{}{}",
                        if s.cost_partial { "\u{2265}" } else { "~" },
                        format_usd(s.cost_usd)
                    )
                })
        };
        rows.push(
            Row::new(vec![
                Cell::from("API-equivalent"),
                Cell::from(Line::from(cost(periods[0])).right_aligned()),
                Cell::from(Line::from(cost(periods[1])).right_aligned()),
                Cell::from(Line::from(cost(periods[2])).right_aligned()),
            ])
            .style(secondary(t)),
        );
    }
    let height = rows.len() as u16 + 1;
    let area = page.rect(height);
    Table::new(
        rows,
        [
            Constraint::Length(18),
            Constraint::Length(11),
            Constraint::Length(11),
            Constraint::Length(11),
        ],
    )
    .header(
        Row::new(vec![
            Cell::from(""),
            Cell::from(Line::from("Today").right_aligned()),
            Cell::from(Line::from("Week").right_aligned()),
            Cell::from(Line::from("Month").right_aligned()),
        ])
        .style(muted(t)),
    )
    .render(
        Rect::new(2, area.y, area.width.saturating_sub(2), area.height),
        &mut page.buf,
    );
    // Changes against the previous period.
    let changes: Vec<String> = periods
        .iter()
        .filter_map(|p| {
            app.usage
                .summary(*p, "")
                .and_then(|s| format_change(s.tokens_change_pct, *p, ""))
        })
        .collect();
    if !changes.is_empty() {
        page.text(&format!("  {}", changes.join(" \u{b7} ")), muted(t));
    }
    if let Some(s) = app.usage.summary(UsagePeriod::Today, "") {
        page.text(
            &format!("  {} messages today", format_exact_tokens(s.messages)),
            muted(t),
        );
    }
}

fn report_section(app: &App, overlay: &UsageOverlay, page: &mut Page) {
    let t = &app.theme;
    let show_cost = app.config.usage.show_cost;
    let w = page.width as usize;
    let active = REPORT_TABS
        .iter()
        .position(|p| *p == overlay.provider)
        .unwrap_or(0);
    let labels: Vec<&str> = REPORT_TABS.iter().map(|p| report_tab_label(*p)).collect();
    let mut right = segmented(t, &labels, active);
    right.push(Span::raw("  "));
    right.extend(segmented(t, &["7d", "30d"], overlay.trend));
    page.line(section(t, "Report", right, w));
    let Some(report) = app.usage.reports.get(&overlay.provider) else {
        page.text(
            if app.usage.error.is_some() {
                "The report is not available."
            } else {
                "Loading\u{2026}"
            },
            muted(t),
        );
        return;
    };
    let days = overlay.trend_days();
    let window: Vec<_> = report.daily.iter().rev().take(days).rev().collect();
    let value = |d: &as_core::usage::report::UsageReportDay| {
        if show_cost {
            (d.cost_usd * 100.0).round() as u64
        } else {
            d.total_tokens
        }
    };
    let values: Vec<u64> = window.iter().map(|d| value(d)).collect();
    let color = match overlay.provider {
        UsageReportProvider::Claude => t.claude,
        UsageReportProvider::Codex => t.codex,
        UsageReportProvider::All => t.accent,
    };
    let total_tokens: u64 = window.iter().map(|d| d.total_tokens).sum();
    let total_cost: f64 = window.iter().map(|d| d.cost_usd).sum();
    let partial = window.iter().any(|d| d.cost_partial);
    let active_days = window.iter().filter(|d| d.total_tokens > 0).count();
    let noun = if show_cost { "Cost" } else { "Token" };
    page.text(
        &format!("{noun} trend \u{b7} last {days} days"),
        secondary(t),
    );
    let chart = page.rect(TREND_ROWS);
    bars(
        t,
        &values,
        color,
        Rect::new(2, chart.y, chart.width.saturating_sub(4), chart.height),
        &mut page.buf,
    );
    let first = window.first().map_or("", |d| d.date.as_str()).to_string();
    let last = window.last().map_or("", |d| d.date.as_str()).to_string();
    page.line(spread(
        vec![Span::raw("  "), Span::styled(first, muted(t))],
        vec![Span::styled(last, muted(t)), Span::raw("  ")],
        w,
    ));
    let mut facts = vec![format!("{} tokens", format_compact_tokens(total_tokens))];
    if show_cost {
        facts.push(format!(
            "{}{}",
            if partial { "\u{2265}" } else { "~" },
            format_usd(total_cost)
        ));
    }
    facts.push(format!(
        "{active_days} active day{}",
        if active_days == 1 { "" } else { "s" }
    ));
    if days > 0 {
        let avg = total_tokens / days as u64;
        facts.push(format!("avg {}/day", format_compact_tokens(avg)));
    }
    page.text(&format!("  {}", facts.join(" \u{b7} ")), secondary(t));
    page.gap();
    named_table(
        app,
        page,
        &format!("Models \u{b7} last {days} days"),
        "Model",
        &models_over(report, days),
        show_cost,
    );
    page.gap();
    named_table(
        app,
        page,
        &format!("Projects \u{b7} last {days} days"),
        "Project",
        &projects_over(report, days),
        show_cost,
    );
}

fn named_table(
    app: &App,
    page: &mut Page,
    title: &str,
    head: &str,
    rows: &[(String, u64, f64)],
    show_cost: bool,
) {
    let t = &app.theme;
    page.line(section(t, title, Vec::new(), page.width as usize));
    if rows.iter().all(|r| r.1 == 0) {
        page.text("  Nothing in this window.", muted(t));
        return;
    }
    let shown: Vec<Row> = rows
        .iter()
        .filter(|r| r.1 > 0)
        .take(TABLE_ROWS)
        .map(|(name, tokens, cost)| {
            let mut cells = vec![
                Cell::from(clip(name.strip_prefix("claude-").unwrap_or(name), 40)),
                Cell::from(Line::from(format_compact_tokens(*tokens)).right_aligned()),
            ];
            if show_cost {
                cells.push(Cell::from(Line::from(format_usd(*cost)).right_aligned()));
            }
            Row::new(cells)
        })
        .collect();
    let mut header = vec![
        Cell::from(head.to_string()),
        Cell::from(Line::from("Tokens").right_aligned()),
    ];
    let mut widths = vec![Constraint::Min(16), Constraint::Length(10)];
    if show_cost {
        header.push(Cell::from(Line::from("Cost").right_aligned()));
        widths.push(Constraint::Length(11));
    }
    let area = page.rect(shown.len() as u16 + 1);
    Table::new(shown, widths)
        .header(Row::new(header).style(muted(t)))
        .render(
            Rect::new(2, area.y, area.width.saturating_sub(2), area.height),
            &mut page.buf,
        );
    let more = rows
        .iter()
        .filter(|r| r.1 > 0)
        .count()
        .saturating_sub(TABLE_ROWS);
    if more > 0 {
        page.text(&format!("  +{more} more"), muted(t));
    }
}

/// The report overlay: plan limits with reset times, tokens per period and provider, the daily
/// trend and the per-model and per-project tables. `j/k` scroll.
pub fn render_overlay(app: &mut App, overlay: &mut UsageOverlay, screen: Rect, buf: &mut Buffer) {
    let t = app.theme;
    let width = screen.width.min(104);
    let height = screen.height.saturating_sub(2).max(8).min(screen.height);
    let area = crate::ui::dialogs::centered(screen, width, height);
    Clear.render(area, buf);
    let updated = app
        .usage
        .state
        .as_ref()
        .map(|s| {
            format!(
                " updated {} ",
                format_updated(newest_fetch(s), app.overview.clock().now)
            )
        })
        .unwrap_or_default();
    let refresh = if app.usage.refreshing {
        " refreshing\u{2026} "
    } else {
        ""
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(t.accent))
        .title(Span::styled(" Usage ", bold(&t)))
        .title(
            Line::from(vec![
                Span::styled(updated, muted(&t)),
                Span::styled(refresh, Style::default().fg(t.starting)),
            ])
            .right_aligned(),
        )
        .style(Style::default().fg(t.text).bg(t.surface_raised));
    let inner = block.inner(area);
    block.render(area, buf);
    if inner.width < 40 || inner.height < 3 {
        return;
    }
    let content = Rect::new(inner.x + 1, inner.y, inner.width - 2, inner.height);
    let Some(state) = app.usage.state.clone() else {
        buf.set_string(content.x, content.y, "Loading usage\u{2026}", muted(&t));
        return;
    };
    let mut page = Page::new(content.width);
    limits_section(app, &*overlay, &state, &mut page);
    page.gap();
    tokens_section(app, &state, &mut page);
    page.gap();
    report_section(app, &*overlay, &mut page);
    let total = page.y;
    let max_scroll = total.saturating_sub(content.height);
    let scroll = u16::try_from(overlay.scroll)
        .unwrap_or(u16::MAX)
        .min(max_scroll);
    overlay.scroll = usize::from(scroll);
    for dy in 0..content.height.min(total.saturating_sub(scroll)) {
        for x in 0..content.width {
            if let (Some(src), Some(dst)) = (
                page.buf.cell((x, dy + scroll)),
                buf.cell_mut((content.x + x, content.y + dy)),
            ) {
                *dst = src.clone();
            }
        }
    }
    if total > content.height {
        let more = if scroll < max_scroll {
            "\u{25BC} more"
        } else {
            "\u{25B2} top: k"
        };
        let x = content.right().saturating_sub(more.width() as u16);
        buf.set_string(x, area.bottom().saturating_sub(1), more, muted(&t));
    }
}
