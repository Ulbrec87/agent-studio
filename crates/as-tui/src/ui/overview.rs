//! Overview view: a scrollable page of cards (Hydra `src/components/Overview/*`) drawn with
//! ratatui blocks. The page is painted on an off-screen buffer as tall as its content and the
//! visible window is copied to the screen, so every card can have its natural height.

use as_core::overview::buckets::{HOUR_MS, Segment, SegmentStatus};
use as_core::overview::grouping::BoardKey;
use as_core::overview::{LongestWait, OverviewSession, OverviewStatus, WaitFigure, WaitTotals};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Sparkline, Widget};
use unicode_width::UnicodeWidthStr;

use crate::app::App;
use crate::overview::format::{
    Clock, branch_of, clip, event_text, format_count, format_duration, last_wait_from,
    project_name, quiet_text, reason_label, status_fact, status_label, wrap,
};
use crate::overview::model::{self, CountKind, SessionRow};
use crate::overview::state::{Card, SessionsMode, WaitsTab};
use crate::theme::Theme;
use crate::ui::layout::Layout;

/// Main-area width from which the cards go into two columns (a 28-column sidebar makes it ~140
/// terminal columns).
pub const WIDE: u16 = 110;
/// Show the usage card slot (filled by a later phase).
pub const SHOW_USAGE_SLOT: bool = false;

const MAX_NEEDS: usize = 14;
const MAX_SESSIONS: usize = 15;
const MAX_EVENTS: usize = 12;
const MAX_TIMELINE_ROWS: usize = 8;
const CHART_ROWS: u16 = 6;

// ── colors ──────────────────────────────────────────────────────────────────

fn status_color(t: &Theme, s: OverviewStatus, stale: bool) -> Color {
    match s {
        OverviewStatus::NeedsYou => t.starting,
        OverviewStatus::Working => t.running,
        OverviewStatus::Idle if stale => t.text_muted,
        OverviewStatus::Idle => t.idle,
        OverviewStatus::Starting => t.accent,
        OverviewStatus::Finished => t.waiting,
        OverviewStatus::Failed => t.error,
        OverviewStatus::NotRunning => t.text_muted,
    }
}

fn dot(t: &Theme, s: &OverviewSession) -> Span<'static> {
    Span::styled(
        "\u{25CF} ",
        Style::default().fg(status_color(t, s.status, s.stale)),
    )
}

// ── tiny layout helpers ─────────────────────────────────────────────────────

fn width_of(spans: &[Span]) -> usize {
    spans.iter().map(|s| s.content.width()).sum()
}

/// `left` ... `right` on one line of `width` columns (the left side is clipped to fit).
fn spread(left: Vec<Span<'static>>, right: Vec<Span<'static>>, width: usize) -> Line<'static> {
    let rw = width_of(&right);
    let lw = width_of(&left);
    if lw + rw + 1 > width {
        let room = width.saturating_sub(rw + 1);
        let text: String = left.iter().map(|s| s.content.as_ref()).collect();
        let style = left.first().map(|s| s.style).unwrap_or_default();
        let mut out = vec![Span::styled(clip(&text, room), style)];
        out.push(Span::raw(" "));
        out.extend(right);
        return Line::from(out);
    }
    let mut out = left;
    out.push(Span::raw(" ".repeat(width - lw - rw)));
    out.extend(right);
    Line::from(out)
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

/// A card body: lines with an optional highlight and the line that must stay visible.
#[derive(Default)]
struct Body {
    rows: Vec<(Line<'static>, bool)>,
    anchor: Option<usize>,
}

impl Body {
    fn push(&mut self, line: Line<'static>) {
        self.rows.push((line, false));
    }
    fn push_hl(&mut self, line: Line<'static>, hl: bool) {
        self.rows.push((line, hl));
    }
    fn len(&self) -> usize {
        self.rows.len()
    }
    fn anchor_here(&mut self) {
        self.anchor = Some(self.rows.len().saturating_sub(1));
    }
}

fn paint(buf: &mut Buffer, area: Rect, body: &Body, t: &Theme, focused: bool) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let h = usize::from(area.height);
    let top = match body.anchor {
        Some(a) if a >= h => a + 1 - h,
        _ => 0,
    };
    for (i, (line, hl)) in body.rows.iter().skip(top).take(h).enumerate() {
        let row = Rect {
            y: area.y + u16::try_from(i).unwrap_or(0),
            height: 1,
            ..area
        };
        if *hl && focused {
            for x in row.x..row.right() {
                if let Some(c) = buf.cell_mut((x, row.y)) {
                    c.set_bg(t.surface_hover);
                }
            }
        }
        line.clone().render(row, buf);
    }
    // A hint that there is more.
    if body.rows.len() > top + h && h > 1 {
        let x = area.right().saturating_sub(1);
        if let Some(c) = buf.cell_mut((x, area.bottom() - 1)) {
            c.set_symbol("\u{25BE}");
            c.set_fg(t.text_muted);
        }
    }
    if top > 0
        && h > 1
        && let Some(c) = buf.cell_mut((area.right().saturating_sub(1), area.y))
    {
        c.set_symbol("\u{25B4}");
        c.set_fg(t.text_muted);
    }
}

// ── page layout ─────────────────────────────────────────────────────────────

struct Placed {
    card: Card,
    rect: Rect,
}

fn needs_waited(app: &App, clock: &Clock) -> Vec<as_core::overview::WaitBar> {
    model::last_hour_waits_for(&app.overview, clock)
        .map(|w| w.bars)
        .unwrap_or_default()
}

/// Inner height of a card (without borders).
fn natural_height(app: &App, card: Card, inner_w: u16, clock: &Clock) -> u16 {
    let o = &app.overview;
    let n = match card {
        Card::Counts => 3 * usize::from(counts_rows(inner_w)),
        Card::NeedsYou => {
            let list = o.needs_you();
            let bars = needs_waited(app, clock).len().min(3);
            let bar_lines = if bars > 0 { bars + 1 } else { 0 };
            let items: usize = list
                .iter()
                .map(|s| 2 + usize::from(s.waiting_text.is_some()))
                .sum();
            let base = if list.is_empty() { 3 } else { items };
            (base + bar_lines).min(MAX_NEEDS)
        }
        Card::LastHour => usize::from(CHART_ROWS) + 2,
        Card::Sessions => match o.mode {
            SessionsMode::Board => 1 + usize::from(model::BOARD_MAX as u16) * 2 + 2,
            _ => model::session_rows(o).len().clamp(2, MAX_SESSIONS),
        },
        Card::Events => {
            let n = o.events_newest_first().len() + usize::from(o.marker.is_some());
            n.clamp(2, MAX_EVENTS)
        }
        Card::Timeline => model::timeline(o, clock).len().clamp(1, MAX_TIMELINE_ROWS) + 3,
        Card::Waits => {
            if inner_w >= 90 {
                10
            } else {
                16
            }
        }
    };
    u16::try_from(n).unwrap_or(u16::MAX)
}

fn layout_cards(app: &App, width: u16, clock: &Clock) -> (Vec<Placed>, u16) {
    let mut placed = Vec::new();
    let mut y = 0u16;
    let height = |card: Card, w: u16| natural_height(app, card, w.saturating_sub(2), clock) + 2;
    if width >= WIDE {
        let half = width / 2;
        let rows: [&[Card]; 5] = [
            &[Card::Counts],
            &[Card::NeedsYou, Card::LastHour],
            &[Card::Sessions, Card::Events],
            &[Card::Timeline],
            &[Card::Waits],
        ];
        for row in rows {
            let h = row
                .iter()
                .map(|c| height(*c, if row.len() == 1 { width } else { half }))
                .max()
                .unwrap_or(0);
            let mut x = 0;
            for (i, card) in row.iter().enumerate() {
                let w = if row.len() == 1 {
                    width
                } else if i == 0 {
                    half
                } else {
                    width - half
                };
                placed.push(Placed {
                    card: *card,
                    rect: Rect::new(x, y, w, h),
                });
                x += w;
            }
            y += h;
        }
    } else {
        for card in [
            Card::NeedsYou,
            Card::Counts,
            Card::Sessions,
            Card::LastHour,
            Card::Events,
            Card::Timeline,
            Card::Waits,
        ] {
            let h = height(card, width);
            placed.push(Placed {
                card,
                rect: Rect::new(0, y, width, h),
            });
            y += h;
        }
    }
    (placed, y)
}

// ── entry point ─────────────────────────────────────────────────────────────

pub fn render(app: &mut App, l: &Layout, buf: &mut Buffer) {
    let t = app.theme;
    let base = Style::default().fg(t.text).bg(t.bg);
    for area in [l.title, l.terminal] {
        for y in area.y..area.bottom() {
            for x in area.x..area.right() {
                if let Some(c) = buf.cell_mut((x, y)) {
                    c.set_symbol(" ");
                    c.set_style(base);
                }
            }
        }
    }
    render_toolbar(app, l.title, buf);
    let view = l.terminal;
    if view.width < 20 || view.height < 3 {
        return;
    }
    let clock = app.overview.clock();
    if app.overview.snapshot.is_none() {
        let text = match &app.overview.error {
            Some(error) => format!(
                "The overview is not available: {error}. If Agent Studio was updated, restart the daemon (agent-studio stop)."
            ),
            None => "Loading overview\u{2026}".to_string(),
        };
        render_message(view, "", &text, &t, buf);
        return;
    }
    if app.overview.all_sessions().is_empty() {
        render_message(
            view,
            "No agents yet",
            "Start an agent (prefix n) and its status shows up here.",
            &t,
            buf,
        );
        return;
    }

    // Reserve a column for the scrollbar when the page is taller than the screen.
    let mut width = view.width;
    let (mut placed, mut total) = layout_cards(app, width, &clock);
    if total > view.height {
        width = view.width - 1;
        (placed, total) = layout_cards(app, width, &clock);
    }
    let focus = app.overview.focus;
    let cursor_now = focus_cursor(app);
    if let Some(p) = placed.iter().find(|p| p.card == focus)
        && app.overview.ensured != Some((focus, cursor_now))
    {
        // Bring the focused card into view once per move; paging may then leave it.
        let (top, bottom) = (p.rect.y, p.rect.bottom());
        if usize::from(top) < app.overview.scroll {
            app.overview.scroll = usize::from(top);
        } else if usize::from(bottom) > app.overview.scroll + usize::from(view.height) {
            app.overview.scroll = usize::from(bottom)
                .saturating_sub(usize::from(view.height))
                .min(usize::from(top));
        }
        app.overview.ensured = Some((focus, cursor_now));
    }
    let max_scroll = usize::from(total.saturating_sub(view.height));
    app.overview.scroll = app.overview.scroll.min(max_scroll);

    let page_area = Rect::new(0, 0, width, total);
    let mut page = Buffer::empty(page_area);
    for y in 0..total {
        for x in 0..width {
            if let Some(c) = page.cell_mut((x, y)) {
                c.set_style(base);
            }
        }
    }
    for p in &placed {
        render_card(app, p, &clock, &mut page);
    }
    let scroll = u16::try_from(app.overview.scroll).unwrap_or(0);
    for dy in 0..view.height.min(total.saturating_sub(scroll)) {
        for x in 0..width {
            if let (Some(src), Some(dst)) = (
                page.cell((x, dy + scroll)),
                buf.cell_mut((view.x + x, view.y + dy)),
            ) {
                *dst = src.clone();
            }
        }
    }
    if total > view.height {
        render_scrollbar(app, view, total, buf);
    }
    if app.overview.drawer.is_some() {
        render_drawer(app, l, &clock, buf);
    }
}

fn focus_cursor(app: &App) -> usize {
    let o = &app.overview;
    match o.focus {
        Card::NeedsYou => o.needs_cursor,
        Card::Sessions => o.sessions_cursor,
        Card::Events => o.events_cursor,
        Card::Timeline => o.timeline_scroll,
        Card::Waits => o.waits_scroll,
        _ => 0,
    }
}

fn render_scrollbar(app: &App, view: Rect, total: u16, buf: &mut Buffer) {
    let t = &app.theme;
    let x = view.right() - 1;
    let h = usize::from(view.height);
    let total = usize::from(total);
    let thumb = (h * h / total).max(1);
    let max = total - h;
    let pos = (app.overview.scroll * (h - thumb))
        .checked_div(max)
        .unwrap_or(0);
    for i in 0..h {
        let on = i >= pos && i < pos + thumb;
        if let Some(c) = buf.cell_mut((x, view.y + u16::try_from(i).unwrap_or(0))) {
            c.set_symbol(if on { "\u{2503}" } else { "\u{2502}" });
            c.set_fg(if on { t.text_secondary } else { t.border });
        }
    }
}

fn render_message(area: Rect, title: &str, text: &str, t: &Theme, buf: &mut Buffer) {
    let lines = wrap(text, usize::from(area.width.saturating_sub(4)).max(10));
    let total = lines.len() + usize::from(!title.is_empty()) * 2;
    let mut y =
        area.y + u16::try_from(usize::from(area.height).saturating_sub(total) / 2).unwrap_or(0);
    if !title.is_empty() {
        let x = area.x
            + area
                .width
                .saturating_sub(u16::try_from(title.width()).unwrap_or(0))
                / 2;
        buf.set_string(x, y, title, bold(t));
        y += 2;
    }
    for line in lines {
        if y >= area.bottom() {
            break;
        }
        let x = area.x
            + area
                .width
                .saturating_sub(u16::try_from(line.width()).unwrap_or(0))
                / 2;
        buf.set_string(x, y, &line, secondary(t));
        y += 1;
    }
}

fn render_toolbar(app: &App, area: Rect, buf: &mut Buffer) {
    if area.height == 0 {
        return;
    }
    let t = &app.theme;
    let o = &app.overview;
    let sessions = o.sessions();
    let counts = as_core::overview::status::compute_counts(&sessions, o.clock().now);
    let mut left = vec![Span::styled(" Overview", secondary(t))];
    if o.snapshot.is_some() {
        left.push(Span::styled(
            format!(
                " \u{b7} {} agent{}",
                sessions.len(),
                if sessions.len() == 1 { "" } else { "s" }
            ),
            muted(t),
        ));
        if counts.needs_you > 0 {
            left.push(Span::styled(
                format!(" \u{b7} {} need you", counts.needs_you),
                Style::default().fg(t.starting).add_modifier(Modifier::BOLD),
            ));
        }
        if counts.working > 0 {
            left.push(Span::styled(
                format!(" \u{b7} {} working", counts.working),
                Style::default().fg(t.running),
            ));
        }
    }
    let project = o
        .effective_project()
        .map_or("All".to_string(), project_name);
    let right = vec![
        Span::styled("Project ", muted(t)),
        Span::styled(project, bold(t)),
        Span::styled(" (p)  Sessions ", muted(t)),
        Span::styled(o.mode.label(), bold(t)),
        Span::styled(" (m) ", muted(t)),
    ];
    spread(left, right, usize::from(area.width)).render(area, buf);
}

// ── cards ───────────────────────────────────────────────────────────────────

fn render_card(app: &App, p: &Placed, clock: &Clock, page: &mut Buffer) {
    let t = &app.theme;
    let o = &app.overview;
    let focused = o.focus == p.card && app.focus == crate::app::Focus::Overview;
    let marked = o.focus == p.card;
    let mut title = format!(" {} ", p.card.title());
    match p.card {
        Card::NeedsYou if !o.needs_you().is_empty() => {
            title = format!(" Needs you ({}) ", o.needs_you().len());
        }
        Card::Sessions => title = format!(" Sessions \u{b7} {} ", o.mode.label()),
        Card::Waits => {
            title = format!(
                " Waits \u{b7} {} ",
                match o.waits_tab {
                    WaitsTab::Today => "longest today",
                    WaitsTab::Week => "longest in 7 days",
                }
            );
        }
        _ => {}
    }
    let border = if focused {
        Style::default().fg(t.accent)
    } else if marked {
        Style::default().fg(t.text_secondary)
    } else {
        Style::default().fg(t.border)
    };
    let title_style = if p.card == Card::NeedsYou && !o.needs_you().is_empty() {
        Style::default().fg(t.starting).add_modifier(Modifier::BOLD)
    } else if focused {
        Style::default().fg(t.accent).add_modifier(Modifier::BOLD)
    } else {
        secondary(t).add_modifier(Modifier::BOLD)
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(if focused {
            BorderType::Thick
        } else {
            BorderType::Rounded
        })
        .border_style(border)
        .title(Span::styled(title, title_style));
    let inner = block.inner(p.rect);
    block.render(p.rect, page);
    if inner.width < 4 || inner.height == 0 {
        return;
    }
    match p.card {
        Card::Counts => counts_card(app, inner, clock, page),
        Card::NeedsYou => needs_you_card(app, inner, clock, focused, page),
        Card::LastHour => last_hour_card(app, inner, clock, page),
        Card::Sessions => sessions_card(app, inner, clock, focused, page),
        Card::Events => events_card(app, inner, clock, focused, page),
        Card::Timeline => timeline_card(app, inner, clock, focused, page),
        Card::Waits => waits_card(app, inner, clock, page),
    }
}

fn kind_color(t: &Theme, kind: CountKind) -> Color {
    match kind {
        CountKind::Working => t.running,
        CountKind::Idle => t.idle,
        CountKind::Stale => t.text_muted,
        CountKind::NeedsYou => t.starting,
        CountKind::Finished => t.waiting,
        CountKind::Failed => t.error,
    }
}

/// Six counts in a row when there is room, else three per row.
fn counts_rows(inner_w: u16) -> u16 {
    if inner_w >= 84 { 1 } else { 2 }
}

fn counts_card(app: &App, inner: Rect, clock: &Clock, buf: &mut Buffer) {
    let t = &app.theme;
    let cells = model::count_cells(&app.overview, clock);
    let rows = counts_rows(inner.width);
    let per_row = u16::try_from(cells.len())
        .unwrap_or(6)
        .div_ceil(rows)
        .max(1);
    let cw = inner.width / per_row;
    for (i, cell) in cells.iter().enumerate() {
        let (row, col) = (
            u16::try_from(i).unwrap_or(0) / per_row,
            u16::try_from(i).unwrap_or(0) % per_row,
        );
        let x = inner.x + cw * col;
        let w = if col + 1 == per_row {
            inner.right() - x
        } else {
            cw
        };
        let color = kind_color(t, cell.kind);
        let top = inner.y + row * 3;
        let area = |dy: u16| Rect::new(x, top + dy, w.saturating_sub(1), 1);
        if top + 2 > inner.bottom() {
            break;
        }
        let label = Line::from(vec![
            Span::styled("\u{25CF} ", Style::default().fg(color)),
            Span::styled(cell.label, secondary(t)),
        ]);
        label.render(area(0), buf);
        if top + 1 < inner.bottom() {
            let value = Line::from(vec![
                Span::styled(
                    cell.value.to_string(),
                    Style::default()
                        .fg(if cell.value > 0 { color } else { t.text_muted })
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(format!("  {}", cell.detail), muted(t)),
            ]);
            value.render(area(1), buf);
        }
        if top + 2 < inner.bottom() && !cell.series.is_empty() {
            let max = cell.series.iter().flatten().fold(1.0_f64, |m, v| m.max(*v));
            let data: Vec<Option<u64>> = cell
                .series
                .iter()
                .map(|v| v.map(|v| (v * 100.0).round() as u64))
                .collect();
            Sparkline::default()
                .data(data)
                .max((max * 100.0).round() as u64)
                .style(Style::default().fg(color))
                .absent_value_symbol("\u{b7}")
                .absent_value_style(muted(t))
                .render(area(2), buf);
        }
    }
}

fn needs_you_card(app: &App, inner: Rect, clock: &Clock, focused: bool, buf: &mut Buffer) {
    let t = &app.theme;
    let o = &app.overview;
    let w = usize::from(inner.width);
    let list = o.needs_you();
    let mut body = Body::default();
    let multi = o
        .all_sessions()
        .iter()
        .map(|s| s.provider)
        .collect::<std::collections::HashSet<_>>()
        .len()
        > 1;
    if list.is_empty() {
        body.push(Line::from(vec![
            Span::styled("\u{2714} ", Style::default().fg(t.running)),
            Span::styled("Nothing needs you", bold(t)),
        ]));
        body.push(Line::styled(
            "When an agent stops to ask for something, it will show here.",
            muted(t),
        ));
        let events = o.events_newest_first();
        if let Some(last) = last_wait_from(&events, clock.now) {
            body.push(Line::from(vec![
                Span::styled("Last wait ", muted(t)),
                Span::styled(format_duration(last.waited_ms, false), bold(t)),
                Span::styled(
                    format!(
                        " \u{b7} {}, answered at {}",
                        last.name,
                        clock.minute(last.ended_at)
                    ),
                    secondary(t),
                ),
            ]));
        }
    }
    for (i, s) in list.iter().enumerate() {
        let hl = i == o.needs_cursor;
        let marker = if hl && focused { "\u{25B6} " } else { "  " };
        let name = Span::styled(clip(&s.name, 28), bold(t));
        let left = vec![
            Span::styled(marker, Style::default().fg(t.starting)),
            name,
            Span::styled(
                format!("  {}", reason_label(s.waiting_reason)),
                Style::default().fg(t.starting),
            ),
        ];
        let timer = vec![Span::styled(
            format_duration(clock.now - s.status_since, false),
            Style::default().fg(t.starting).add_modifier(Modifier::BOLD),
        )];
        body.push_hl(spread(left, timer, w), hl);
        let mut where_ = match &s.worktree_branch {
            Some(b) => format!("{} on {b}", project_name(&s.project_dir)),
            None => project_name(&s.project_dir),
        };
        if let Some(b) = branch_of(s).filter(|b| s.worktree_branch.as_deref() != Some(*b)) {
            where_.push_str(&format!(" \u{b7} {b}"));
        }
        if multi {
            where_.push_str(&format!(" \u{b7} {}", s.provider.label()));
        }
        body.push_hl(
            Line::styled(
                format!("  {}", clip(&where_, w.saturating_sub(2))),
                muted(t),
            ),
            hl,
        );
        if let Some(text) = &s.waiting_text {
            body.push_hl(
                Line::styled(
                    format!("  {}", clip(text, w.saturating_sub(2))),
                    Style::default().fg(t.text),
                ),
                hl,
            );
        }
        if hl {
            body.anchor_here();
        }
    }
    // "Waited on you": one bar per agent that waited in the last hour.
    let bars = needs_waited(app, clock);
    if !bars.is_empty() {
        body.push(Line::styled("Waited on you \u{b7} last hour", muted(t)));
        let longest = bars.iter().map(|b| b.ms).max().unwrap_or(1).max(1);
        let bar_w = (w / 3).clamp(6, 24);
        for b in bars.iter().take(3) {
            let filled = usize::try_from(b.ms * i64::try_from(bar_w).unwrap_or(1) / longest)
                .unwrap_or(0)
                .clamp(1, bar_w);
            let glyph = if b.open { "\u{2588}" } else { "\u{2592}" };
            body.push(Line::from(vec![
                Span::styled(glyph.repeat(filled), Style::default().fg(t.starting)),
                Span::styled(" ".repeat(bar_w - filled + 1), Style::default()),
                Span::styled(
                    format!("{}  {}", clip(&b.name, 18), format_duration(b.ms, true)),
                    secondary(t),
                ),
                Span::styled(
                    if b.open { "  waiting now" } else { "" },
                    Style::default().fg(t.starting),
                ),
            ]));
        }
    }
    paint(buf, inner, &body, t, focused);
}

fn last_hour_card(app: &App, inner: Rect, clock: &Clock, buf: &mut Buffer) {
    let t = &app.theme;
    let Some(data) = model::last_hour(&app.overview, clock) else {
        buf.set_string(inner.x, inner.y, "No history yet", muted(t));
        return;
    };
    let waited = format_duration(data.total_waited_ms, true);
    Line::from(vec![
        Span::styled("waited on you ", muted(t)),
        Span::styled(
            waited,
            Style::default().fg(t.starting).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            "  \u{b7} 5 min per bar  \u{b7} \u{2591} not measured",
            muted(t),
        ),
    ])
    .render(Rect { height: 1, ..inner }, buf);
    if inner.height < 4 {
        return;
    }
    let rows = inner.height - 2;
    let chart_top = inner.y + 1;
    let n = data.buckets.len().max(1);
    let col_w = (usize::from(inner.width) / n).clamp(2, 6);
    let bar_w = col_w - 1;
    let scale = data
        .buckets
        .iter()
        .map(|b| b.avg_total)
        .fold(1.0_f64, f64::max);
    let to_rows = |v: f64| (v / scale * f64::from(rows)).round() as u16;
    for (i, b) in data.buckets.iter().enumerate() {
        let x0 = inner.x + u16::try_from(i * col_w).unwrap_or(0);
        if x0 + u16::try_from(bar_w).unwrap_or(1) > inner.right() {
            break;
        }
        let measured = b.measured_ms > 0;
        let needs = if measured {
            to_rows(b.avg_needs_you)
        } else {
            0
        };
        let working = if measured { to_rows(b.avg_working) } else { 0 };
        let total = if measured {
            to_rows(b.avg_total).max(needs + working).min(rows)
        } else {
            0
        };
        for r in 0..rows {
            let y = chart_top + rows - 1 - r;
            let (glyph, color) = if !measured {
                ("\u{2591}", t.text_muted)
            } else if r < needs {
                ("\u{2588}", t.starting)
            } else if r < (needs + working).min(total) {
                ("\u{2588}", t.running)
            } else if r < total {
                ("\u{2588}", t.idle)
            } else {
                continue;
            };
            buf.set_string(x0, y, glyph.repeat(bar_w), Style::default().fg(color));
        }
        // A baseline for empty measured buckets keeps the axis readable.
        if measured && total == 0 {
            buf.set_string(x0, chart_top + rows - 1, "\u{2581}".repeat(bar_w), muted(t));
        }
        if i % 3 == 0 && col_w * 3 >= 6 {
            buf.set_string(
                x0,
                inner.bottom() - 1,
                clock.minute(b.start).chars().take(5).collect::<String>(),
                muted(t),
            );
        }
    }
}

fn session_name_line(
    t: &Theme,
    s: &OverviewSession,
    clock: &Clock,
    show_provider: bool,
    width: usize,
    cursor: bool,
    focused: bool,
) -> Line<'static> {
    let status = status_fact(s, clock);
    let quiet = quiet_text(s, clock.now);
    let model_txt = if show_provider {
        format!("{} {}", s.provider.label(), s.model)
    } else {
        s.model.clone()
    };
    let where_txt = {
        let mut w = project_name(&s.project_dir);
        if let Some(b) = branch_of(s) {
            w.push_str(&format!(" \u{b7} {b}"));
        }
        w
    };
    let mut right = vec![Span::styled(
        status.clone(),
        Style::default().fg(status_color(t, s.status, s.stale)),
    )];
    if let Some(q) = quiet {
        right.insert(
            0,
            Span::styled(format!("{q}  "), Style::default().fg(t.text_muted)),
        );
    }
    // marker + dot + the gap before the right side are fixed; the columns share what is left.
    let mut avail = width.saturating_sub(2 + 2 + 1 + width_of(&right));
    let mut where_w = 0;
    let mut model_w = 0;
    if avail >= 8 + 1 + 22 {
        where_w = 22;
        avail -= where_w + 1;
    }
    let wanted_model = if show_provider { 16 } else { 12 };
    if avail >= 8 + 1 + wanted_model {
        model_w = wanted_model;
        avail -= model_w + 1;
    }
    let name_w = avail.clamp(4, 40);
    let mut spans = vec![
        Span::styled(
            if cursor && focused { "\u{25B6} " } else { "  " },
            Style::default().fg(t.starting),
        ),
        dot(t, s),
        Span::styled(
            format!("{:<w$}", clip(&s.name, name_w), w = name_w),
            bold(t),
        ),
    ];
    if model_w > 0 {
        spans.push(Span::styled(
            format!(" {:<w$}", clip(&model_txt, model_w), w = model_w),
            secondary(t),
        ));
    }
    if where_w > 0 {
        spans.push(Span::styled(
            format!(" {:<w$}", clip(&where_txt, where_w), w = where_w),
            muted(t),
        ));
    }
    spread(spans, right, width)
}

fn sessions_card(app: &App, inner: Rect, clock: &Clock, focused: bool, buf: &mut Buffer) {
    let t = &app.theme;
    let o = &app.overview;
    let w = usize::from(inner.width);
    let multi = o
        .all_sessions()
        .iter()
        .map(|s| s.provider)
        .collect::<std::collections::HashSet<_>>()
        .len()
        > 1;
    if o.mode == SessionsMode::Board {
        sessions_board(app, inner, clock, focused, buf);
        return;
    }
    let rows = model::session_rows(o);
    let mut body = Body::default();
    if rows.is_empty() {
        body.push(Line::styled(
            "No sessions besides the ones that need you.",
            muted(t),
        ));
    }
    let mut selectable = 0usize;
    for row in &rows {
        match row {
            SessionRow::Group { label, count } => body.push(Line::from(vec![
                Span::styled(
                    label.to_uppercase(),
                    secondary(t).add_modifier(Modifier::BOLD),
                ),
                Span::styled(format!("  {count}"), muted(t)),
            ])),
            SessionRow::Session(s) => {
                let hl = selectable == o.sessions_cursor;
                body.push_hl(session_name_line(t, s, clock, multi, w, hl, focused), hl);
                if hl {
                    body.anchor_here();
                }
                selectable += 1;
            }
            SessionRow::ShowMore {
                hidden, expanded, ..
            } => {
                let hl = selectable == o.sessions_cursor;
                let text = if *expanded {
                    "  Show less".to_string()
                } else {
                    format!("  Show {hidden} more")
                };
                body.push_hl(Line::styled(text, Style::default().fg(t.accent)), hl);
                if hl {
                    body.anchor_here();
                }
                selectable += 1;
            }
        }
    }
    paint(buf, inner, &body, t, focused);
}

fn sessions_board(app: &App, inner: Rect, clock: &Clock, focused: bool, buf: &mut Buffer) {
    let t = &app.theme;
    let o = &app.overview;
    let columns = model::board(o);
    let n = u16::try_from(columns.len()).unwrap_or(4).max(1);
    let cw = inner.width / n;
    let mut index = 0usize;
    for (ci, col) in columns.iter().enumerate() {
        let x = inner.x + cw * u16::try_from(ci).unwrap_or(0);
        let w = if ci + 1 == columns.len() {
            inner.right() - x
        } else {
            cw
        };
        let area = Rect::new(x, inner.y, w.saturating_sub(1), inner.height);
        let color = match col.key {
            BoardKey::NeedsYou => t.starting,
            BoardKey::Working => t.running,
            BoardKey::Idle => t.idle,
            BoardKey::Ended => t.waiting,
        };
        let mut body = Body::default();
        body.push(Line::from(vec![
            Span::styled(
                col.label,
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!(" {}", col.total), muted(t)),
        ]));
        for s in &col.items {
            let hl = index == o.sessions_cursor;
            let marker = if hl && focused { "\u{25B6}" } else { " " };
            body.push_hl(
                Line::from(vec![
                    Span::styled(marker, Style::default().fg(t.starting)),
                    dot(t, s),
                    Span::styled(
                        clip(&s.name, usize::from(area.width).saturating_sub(3)),
                        bold(t),
                    ),
                ]),
                hl,
            );
            body.push_hl(
                Line::styled(
                    format!(
                        "   {}",
                        clip(
                            &status_fact(s, clock),
                            usize::from(area.width).saturating_sub(3)
                        )
                    ),
                    muted(t),
                ),
                hl,
            );
            if hl {
                body.anchor_here();
            }
            index += 1;
        }
        if col.hidden > 0 {
            body.push(Line::styled(format!("  and {} more", col.hidden), muted(t)));
        }
        if let Some(stale) = col.stale.filter(|n| *n > 0) {
            body.push(Line::styled(format!("  {stale} stale"), muted(t)));
        }
        if let Some(nr) = col.not_running.filter(|n| *n > 0) {
            body.push(Line::styled(
                format!("  {nr} not running, in the list"),
                muted(t),
            ));
        }
        paint(buf, area, &body, t, focused);
    }
}

fn event_dot(t: &Theme, e: &as_core::overview::OverviewEvent) -> Span<'static> {
    let color = e.to.map_or(t.text_muted, |s| status_color(t, s, false));
    Span::styled("\u{25CF} ", Style::default().fg(color))
}

fn events_card(app: &App, inner: Rect, clock: &Clock, focused: bool, buf: &mut Buffer) {
    let t = &app.theme;
    let o = &app.overview;
    let w = usize::from(inner.width);
    let events = o.events_newest_first();
    let mut body = Body::default();
    if o.history.is_none() {
        body.push(Line::styled("No history yet", muted(t)));
    } else if events.is_empty() {
        body.push(Line::styled("Nothing has happened yet.", muted(t)));
    }
    let marker_at = o.marker.map(|m| m.index);
    for (i, e) in events.iter().enumerate() {
        if marker_at == Some(i)
            && let Some(m) = &o.marker
        {
            let text = format!(" new since {} ", clock.minute(m.at));
            let fill = w.saturating_sub(text.width() + 2);
            body.push(Line::styled(
                format!("\u{2500}\u{2500}{text}{}", "\u{2500}".repeat(fill)),
                Style::default().fg(t.accent),
            ));
        }
        let hl = i == o.events_cursor;
        let when = Span::styled(format!("{} ", clock.time_of_day(e.at)), muted(t));
        let line = if e.agent_id.is_empty() {
            Line::from(vec![
                Span::styled(
                    if hl && focused { "\u{25B6} " } else { "  " },
                    Style::default().fg(t.starting),
                ),
                when,
                Span::styled(clip(&event_text(e), w.saturating_sub(12)), muted(t)),
            ])
        } else {
            let head = 2 + 9;
            let rest = w.saturating_sub(head + 2);
            let name = clip(&e.name, (rest / 3).max(8));
            let text = clip(&event_text(e), rest.saturating_sub(name.width() + 1));
            Line::from(vec![
                Span::styled(
                    if hl && focused { "\u{25B6} " } else { "  " },
                    Style::default().fg(t.starting),
                ),
                when,
                event_dot(t, e),
                Span::styled(format!("{name} "), bold(t)),
                Span::styled(text, secondary(t)),
            ])
        };
        body.push_hl(line, hl);
        if hl {
            body.anchor_here();
        }
    }
    if marker_at == Some(events.len())
        && !events.is_empty()
        && let Some(m) = &o.marker
    {
        body.push(Line::styled(
            format!(
                "\u{2500}\u{2500} new since {} \u{2500}\u{2500}",
                clock.minute(m.at)
            ),
            Style::default().fg(t.accent),
        ));
    }
    paint(buf, inner, &body, t, focused);
}

fn segment_at(segments: &[Segment], at: i64) -> Option<SegmentStatus> {
    segments
        .iter()
        .find(|s| s.start <= at && at < s.end)
        .map(|s| s.status)
}

fn timeline_card(app: &App, inner: Rect, clock: &Clock, focused: bool, buf: &mut Buffer) {
    let t = &app.theme;
    let o = &app.overview;
    let rows = model::timeline(o, clock);
    if o.history.is_none() || rows.is_empty() {
        buf.set_string(
            inner.x,
            inner.y,
            if o.history.is_none() {
                "No history yet"
            } else {
                "No activity in the last hour."
            },
            muted(t),
        );
        return;
    }
    let label_w = rows
        .iter()
        .map(|r| r.name.width())
        .max()
        .unwrap_or(8)
        .clamp(8, 18);
    let track_w = usize::from(inner.width).saturating_sub(label_w + 2).max(8);
    let visible = usize::from(inner.height).saturating_sub(2).max(1);
    let top = o
        .timeline_scroll
        .saturating_sub(visible.saturating_sub(1))
        .min(rows.len().saturating_sub(1));
    let start = clock.now - HOUR_MS;
    for (i, row) in rows.iter().skip(top).take(visible).enumerate() {
        let y = inner.y + u16::try_from(i).unwrap_or(0);
        let hl = top + i == o.timeline_scroll && focused;
        buf.set_string(
            inner.x,
            y,
            format!(
                "{}{:<w$}",
                if hl { "\u{25B6}" } else { " " },
                clip(&row.name, label_w),
                w = label_w
            ),
            if hl { bold(t) } else { secondary(t) },
        );
        for k in 0..track_w {
            let at = start
                + (HOUR_MS * (2 * i64::try_from(k).unwrap_or(0) + 1))
                    / (2 * i64::try_from(track_w).unwrap_or(1));
            let (glyph, color) = match segment_at(&row.segments, at) {
                Some(SegmentStatus::Status(s)) => ("\u{2588}", status_color(t, s, false)),
                Some(SegmentStatus::Unmeasured) => ("\u{2591}", t.text_muted),
                None => continue,
            };
            buf.set_string(
                inner.x + u16::try_from(label_w + 2 + k).unwrap_or(0),
                y,
                glyph,
                Style::default().fg(color),
            );
        }
    }
    // Axis and legend.
    let axis_y = inner.y + u16::try_from(visible).unwrap_or(1);
    if axis_y < inner.bottom() {
        let x0 = inner.x + u16::try_from(label_w + 2).unwrap_or(0);
        buf.set_string(x0, axis_y, "\u{2212}60m", muted(t));
        let mid = x0 + u16::try_from(track_w / 2).unwrap_or(0);
        buf.set_string(mid.saturating_sub(2), axis_y, "\u{2212}30m", muted(t));
        buf.set_string(
            x0 + u16::try_from(track_w.saturating_sub(3)).unwrap_or(0),
            axis_y,
            "now",
            muted(t),
        );
    }
    if axis_y + 1 < inner.bottom() {
        let mut spans = Vec::new();
        for (label, color) in [
            ("needs you", t.starting),
            ("working", t.running),
            ("idle", t.idle),
            ("finished", t.waiting),
            ("failed", t.error),
        ] {
            spans.push(Span::styled("\u{2588} ", Style::default().fg(color)));
            spans.push(Span::styled(format!("{label}  "), muted(t)));
        }
        spans.push(Span::styled("\u{2591} not measured", muted(t)));
        Line::from(spans).render(Rect::new(inner.x, axis_y + 1, inner.width, 1), buf);
    }
}

fn weekday_label(day: &str) -> String {
    chrono::NaiveDate::parse_from_str(day, "%Y-%m-%d")
        .map_or_else(|_| day.to_string(), |d| d.format("%a %d").to_string())
}

fn figure_line(t: &Theme, label: &str, f: &WaitFigure) -> Line<'static> {
    let waits = format!("{} wait{}", f.count, if f.count == 1 { "" } else { "s" });
    Line::from(vec![
        Span::styled(format!("{label:<8}"), secondary(t)),
        Span::styled(
            format!("{:<8}", format_duration(f.ms, false)),
            Style::default()
                .fg(if f.ms > 0 { t.starting } else { t.text_muted })
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("{waits}  measured {}", format_duration(f.measured_ms, true)),
            muted(t),
        ),
    ])
}

fn longest_lines(t: &Theme, list: &[LongestWait], width: usize) -> Vec<Line<'static>> {
    if list.is_empty() {
        return vec![Line::styled("Nobody waited yet.", muted(t))];
    }
    list.iter()
        .enumerate()
        .map(|(i, l)| {
            let mut spans = vec![
                Span::styled(format!("{}. ", i + 1), muted(t)),
                Span::styled(clip(&l.name, width.saturating_sub(26).max(8)), bold(t)),
                Span::styled(
                    format!(
                        "  {} \u{b7} {}",
                        format_duration(l.ms, true),
                        plural(l.count, "wait")
                    ),
                    secondary(t),
                ),
            ];
            if l.waiting_now {
                spans.push(Span::styled(
                    "  waiting now",
                    Style::default().fg(t.starting),
                ));
            }
            Line::from(spans)
        })
        .collect()
}

fn plural(n: u32, word: &str) -> String {
    format!("{n} {word}{}", if n == 1 { "" } else { "s" })
}

fn by_day_lines(t: &Theme, totals: &WaitTotals, width: usize) -> Vec<Line<'static>> {
    let longest = totals
        .by_day
        .iter()
        .map(|d| d.figure.ms)
        .max()
        .unwrap_or(0)
        .max(1);
    let bar_w = width.saturating_sub(24).clamp(6, 24);
    totals
        .by_day
        .iter()
        .map(|d| {
            let label = Span::styled(format!("{:<7}", weekday_label(&d.day)), muted(t));
            if !d.measured {
                return Line::from(vec![
                    label,
                    Span::styled("\u{2591}".repeat(bar_w), muted(t)),
                    Span::styled("  not measured", muted(t)),
                ]);
            }
            let filled = usize::try_from(d.figure.ms * i64::try_from(bar_w).unwrap_or(1) / longest)
                .unwrap_or(0);
            let filled = if d.figure.ms > 0 {
                filled.clamp(1, bar_w)
            } else {
                0
            };
            Line::from(vec![
                label,
                Span::styled("\u{2588}".repeat(filled), Style::default().fg(t.starting)),
                Span::styled("\u{b7}".repeat(bar_w - filled), muted(t)),
                Span::styled(
                    format!(
                        "  {}{}",
                        format_duration(d.figure.ms, true),
                        if d.figure.open_ms > 0 { " (open)" } else { "" }
                    ),
                    secondary(t),
                ),
            ])
        })
        .collect()
}

fn waits_card(app: &App, inner: Rect, clock: &Clock, buf: &mut Buffer) {
    let t = &app.theme;
    let o = &app.overview;
    let Some(totals) = model::wait_totals_for(o, clock) else {
        buf.set_string(inner.x, inner.y, "No wait history yet", muted(t));
        return;
    };
    let longest = match o.waits_tab {
        WaitsTab::Today => &totals.longest.today,
        WaitsTab::Week => &totals.longest.week,
    };
    let wide = inner.width >= 90;
    let (left_w, right_w) = if wide {
        (inner.width / 2, inner.width - inner.width / 2)
    } else {
        (inner.width, inner.width)
    };
    let mut left = Body::default();
    left.push(figure_line(t, "Today", &totals.today));
    left.push(figure_line(t, "7 days", &totals.week));
    left.push(Line::styled("By day", muted(t)));
    for line in by_day_lines(t, &totals, usize::from(left_w)) {
        left.push(line);
    }
    let mut right = Body::default();
    right.push(Line::from(vec![
        Span::styled(
            "Longest waiters ",
            secondary(t).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            match o.waits_tab {
                WaitsTab::Today => "today (t: 7 days)",
                WaitsTab::Week => "7 days (t: today)",
            },
            muted(t),
        ),
    ]));
    for line in longest_lines(t, longest, usize::from(right_w)) {
        right.push(line);
    }
    if wide {
        paint(
            buf,
            Rect {
                width: left_w - 1,
                ..inner
            },
            &left,
            t,
            false,
        );
        paint(
            buf,
            Rect {
                x: inner.x + left_w,
                width: right_w,
                ..inner
            },
            &right,
            t,
            false,
        );
    } else {
        left.push(Line::raw(""));
        left.rows.extend(right.rows);
        paint(buf, inner, &left, t, false);
    }
}

// ── details drawer ──────────────────────────────────────────────────────────

fn tokens_text(tokens: &as_core::overview::TokenCounts) -> String {
    let cached = tokens
        .cached
        .map(|c| format!(" ({} cached)", format_count(c)))
        .unwrap_or_default();
    format!(
        "{} in{cached} \u{b7} {} out",
        format_count(tokens.input),
        format_count(tokens.output)
    )
}

fn render_drawer(app: &App, l: &Layout, clock: &Clock, buf: &mut Buffer) {
    let t = &app.theme;
    let o = &app.overview;
    let Some(drawer) = &o.drawer else { return };
    let full = Rect {
        y: l.title.y,
        height: l.title.height + l.terminal.height,
        ..l.terminal
    };
    let width = (full.width * 45 / 100).clamp(40, 72).min(full.width);
    let area = Rect::new(full.right() - width, full.y, width, full.height);
    Clear.render(area, buf);
    let session = o
        .all_sessions()
        .iter()
        .find(|s| s.agent_id == drawer.agent_id);
    let name = session
        .map(|s| s.name.clone())
        .or_else(|| {
            o.history
                .as_ref()
                .and_then(|h| h.agents.get(&drawer.agent_id))
                .map(|a| a.name.clone())
        })
        .unwrap_or_else(|| drawer.agent_id.clone());
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Thick)
        .border_style(Style::default().fg(t.accent))
        .style(Style::default().bg(t.bg).fg(t.text))
        .title(Span::styled(
            format!(" {} ", clip(&name, usize::from(width).saturating_sub(8))),
            bold(t),
        ))
        .title_bottom(Span::styled(
            " Enter jump \u{b7} j/k scroll \u{b7} Esc close ",
            muted(t),
        ));
    let inner = block.inner(area);
    block.render(area, buf);
    let w = usize::from(inner.width);
    let mut body = Body::default();
    let kv = |k: &str, v: String| {
        Line::from(vec![
            Span::styled(format!("{k:<9}"), muted(t)),
            Span::styled(clip(&v, w.saturating_sub(9)), Style::default().fg(t.text)),
        ])
    };
    match session {
        Some(s) => {
            body.push(Line::from(vec![
                Span::styled(format!("{:<9}", "Status"), muted(t)),
                Span::styled(
                    "\u{25CF} ",
                    Style::default().fg(status_color(t, s.status, s.stale)),
                ),
                Span::styled(status_label(s), bold(t)),
                Span::styled(
                    format!(
                        " since {} ({})",
                        clock.minute(s.status_since),
                        format_duration(clock.now - s.status_since, true)
                    ),
                    secondary(t),
                ),
            ]));
            if s.status == OverviewStatus::NeedsYou {
                body.push(Line::from(vec![
                    Span::styled(format!("{:<9}", "Waiting"), muted(t)),
                    Span::styled(
                        reason_label(s.waiting_reason),
                        Style::default().fg(t.starting),
                    ),
                ]));
            }
            if let Some(text) = &s.waiting_text {
                body.push(kv("Asks", text.clone()));
            }
            if matches!(
                s.status,
                OverviewStatus::Finished | OverviewStatus::Failed | OverviewStatus::NotRunning
            ) {
                body.push(kv("Ended", status_fact(s, clock)));
            }
            body.push(kv(
                "Agent",
                format!("{} \u{b7} {}", s.provider.label(), s.model),
            ));
            body.push(kv("Folder", s.project_dir.clone()));
            let repo = match (&s.repo_name, branch_of(s), &s.commit) {
                (Some(r), Some(b), _) => format!("{r} \u{b7} {b}"),
                (Some(r), None, Some(c)) => format!("{r} \u{b7} detached at {c}"),
                (Some(r), None, None) => r.clone(),
                (None, Some(b), _) => b.to_string(),
                _ => "\u{2014}".to_string(),
            };
            body.push(kv("Repo", repo));
            body.push(kv(
                "Started",
                if s.started_at > 0 {
                    clock.clock(s.started_at)
                } else {
                    "never".to_string()
                },
            ));
            body.push(kv(
                "PID",
                s.pid.map_or("\u{2014}".to_string(), |p| p.to_string()),
            ));
        }
        None => body.push(Line::styled("This agent is gone.", muted(t))),
    }
    if let Some(details) = &drawer.details {
        body.push(kv(
            "Tokens",
            details
                .tokens
                .as_ref()
                .map_or("\u{2014}".to_string(), tokens_text),
        ));
    } else {
        body.push(kv("Tokens", "reading\u{2026}".into()));
    }
    // Waits of this agent in the last hour.
    if let Some(waited) = model::last_hour_waits_for(o, clock)
        && let Some(bar) = waited.bars.iter().find(|b| b.agent_id == drawer.agent_id)
    {
        body.push(kv(
            "Waits",
            format!(
                "{} \u{b7} {} in the last hour",
                plural(bar.count, "wait"),
                format_duration(bar.ms, false)
            ),
        ));
    }
    body.push(Line::raw(""));
    body.push(Line::styled(
        "Last message",
        secondary(t).add_modifier(Modifier::BOLD),
    ));
    match drawer.details.as_ref().map(|d| &d.last_message) {
        Some(as_core::overview::LastMessage::Text { text, truncated }) => {
            if *truncated {
                body.push(Line::styled("\u{2026} (the start is cut)", muted(t)));
            }
            for line in wrap(text, w.saturating_sub(1)) {
                body.push(Line::styled(line, Style::default().fg(t.text)));
            }
        }
        Some(as_core::overview::LastMessage::Unavailable { unavailable }) => {
            for line in wrap(unavailable, w.saturating_sub(1)) {
                body.push(Line::styled(line, muted(t)));
            }
        }
        None => body.push(Line::styled("reading\u{2026}", muted(t))),
    }
    body.push(Line::raw(""));
    body.push(Line::styled(
        "Events",
        secondary(t).add_modifier(Modifier::BOLD),
    ));
    let own = model::events_of_agent(o, &drawer.agent_id);
    if own.is_empty() {
        body.push(Line::styled("None yet.", muted(t)));
    }
    for e in own.iter().take(8) {
        body.push(Line::from(vec![
            Span::styled(format!("{} ", clock.time_of_day(e.at)), muted(t)),
            event_dot(t, e),
            Span::styled(clip(&event_text(e), w.saturating_sub(12)), secondary(t)),
        ]));
    }
    body.push(Line::raw(""));
    body.push(Line::styled(
        "Timeline \u{b7} last hour",
        secondary(t).add_modifier(Modifier::BOLD),
    ));
    if let Some(row) = model::timeline(o, clock)
        .into_iter()
        .find(|r| r.agent_id == drawer.agent_id)
    {
        let track_w = w.max(8);
        let start = clock.now - HOUR_MS;
        let spans: Vec<Span<'static>> = (0..track_w)
            .map(|k| {
                let at = start
                    + HOUR_MS * (2 * i64::try_from(k).unwrap_or(0) + 1)
                        / (2 * i64::try_from(track_w).unwrap_or(1));
                match segment_at(&row.segments, at) {
                    Some(SegmentStatus::Status(s)) => {
                        Span::styled("\u{2588}", Style::default().fg(status_color(t, s, false)))
                    }
                    Some(SegmentStatus::Unmeasured) => Span::styled("\u{2591}", muted(t)),
                    None => Span::raw(" "),
                }
            })
            .collect();
        body.push(Line::from(spans));
    } else {
        body.push(Line::styled("No activity in the last hour.", muted(t)));
    }
    // Scrolling: the drawer scroll is a line offset (clamped).
    let max = body.len().saturating_sub(usize::from(inner.height));
    let skip = drawer.scroll.min(max);
    body.rows.drain(..skip);
    paint(buf, inner, &body, t, false);
}
