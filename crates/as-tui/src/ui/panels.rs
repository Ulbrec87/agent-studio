//! Drawing of the P9 panels: git, files (+ search popup), settings, open-in and the free
//! terminals. All state lives in `crate::panels`.

use as_core::PanelDisplayMode;
use as_core::git::GitStatusKind;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Widget};

use crate::app::{App, SubState};
use crate::panels::diff_view::{DiffView, RowKind, SplitRow};
use crate::panels::git::{GitTab, marker};
use crate::panels::settings::{ImportStage, Kind, Section};
use crate::theme::Theme;
use crate::ui::dialogs::{centered, frame};
use crate::ui::sidebar::fit_left;

/// Panel rectangle: centered 90 % (overlay) or the right 60 % of the content area (split).
pub fn panel_area(mode: PanelDisplayMode, screen: Rect) -> Rect {
    match mode {
        PanelDisplayMode::Overlay => centered(
            screen,
            (screen.width * 9 / 10).max(40),
            (screen.height * 9 / 10).max(12),
        ),
        PanelDisplayMode::Split => {
            let w = (screen.width * 6 / 10).max(40).min(screen.width);
            Rect::new(
                screen.right() - w,
                screen.y + 1,
                w,
                screen.height.saturating_sub(2),
            )
        }
    }
}

fn hint(theme: &Theme, text: &str, area: Rect, buf: &mut Buffer) {
    if area.height == 0 {
        return;
    }
    buf.set_stringn(
        area.x,
        area.y,
        text,
        usize::from(area.width),
        Style::default()
            .fg(theme.text_muted)
            .bg(theme.surface_raised),
    );
}

fn tab_line<'a>(theme: &Theme, labels: &[&'a str], active: usize) -> Line<'a> {
    let mut spans = Vec::new();
    for (i, l) in labels.iter().enumerate() {
        let style = if i == active {
            Style::default()
                .fg(theme.accent_text)
                .bg(theme.accent)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.text_secondary)
        };
        spans.push(Span::styled(format!(" {} {l} ", i + 1), style));
        spans.push(Span::raw(" "));
    }
    Line::from(spans)
}

fn row_style(theme: &Theme, selected: bool, focused: bool) -> Style {
    let base = Style::default().fg(theme.text).bg(theme.surface_raised);
    if selected && focused {
        Style::default().fg(theme.accent_text).bg(theme.accent)
    } else if selected {
        Style::default().fg(theme.text).bg(theme.surface_hover)
    } else {
        base
    }
}

fn fill_row(buf: &mut Buffer, x: u16, y: u16, width: u16, style: Style) {
    for xx in x..x + width {
        if let Some(c) = buf.cell_mut((xx, y)) {
            c.set_symbol(" ");
            c.set_style(style);
        }
    }
}

// ── diff ────────────────────────────────────────────────────────────────────

use crate::highlight::StyledSpan;

type Spans = [StyledSpan];

/// Minimum width of the whole git panel body for the side-by-side layout.
pub const SPLIT_MIN_WIDTH: u16 = 100;

/// Draw `text` at `(x, y)` in at most `width` cells: tabs become four spaces, a line that does not
/// fit ends in `…` when `ellipsis` is set. `spans` (optional) color the text over `base`.
fn put_code(
    buf: &mut Buffer,
    (x, y): (u16, u16),
    width: usize,
    text: &str,
    base: Style,
    spans: Option<&Spans>,
    ellipsis: bool,
) {
    use unicode_width::UnicodeWidthChar;
    let mut cells: Vec<(char, Style)> = Vec::new();
    let mut push = |c: char, st: Style| {
        if c == '\t' {
            cells.extend(std::iter::repeat_n((' ', st), 4));
        } else if !c.is_control() {
            cells.push((c, st));
        }
    };
    match spans {
        Some(spans) if !spans.is_empty() => {
            for (st, t) in spans {
                for c in t.chars() {
                    push(c, base.patch(*st));
                }
            }
        }
        _ => text.chars().for_each(|c| push(c, base)),
    }
    let total: usize = cells.iter().map(|(c, _)| c.width().unwrap_or(0)).sum();
    let budget = if ellipsis && total > width {
        width.saturating_sub(1)
    } else {
        width
    };
    let mut used = 0;
    let mut cx = x;
    for (c, st) in &cells {
        let w = c.width().unwrap_or(0);
        if used + w > budget {
            break;
        }
        if let Some(cell) = buf.cell_mut((cx, y)) {
            cell.set_char(*c);
            cell.set_style(*st);
        }
        // Wide characters occupy the following cell too.
        for k in 1..w {
            if let Some(cell) = buf.cell_mut((cx + k as u16, y)) {
                cell.set_symbol("");
                cell.set_style(*st);
            }
        }
        cx += w as u16;
        used += w;
    }
    if ellipsis
        && total > width
        && width > 0
        && used < width
        && let Some(cell) = buf.cell_mut((cx, y))
    {
        cell.set_char('\u{2026}');
        cell.set_style(base);
    }
}

/// Colors of an added / removed / context diff line: `(row style, sign style, sign)`.
fn diff_line_style(theme: &Theme, kind: RowKind, highlighted: bool) -> (Style, Style, char) {
    let plain = Style::default().fg(theme.text).bg(theme.surface_raised);
    match kind {
        RowKind::Add => {
            let bg = theme.diff_add_bg;
            let fg = if highlighted {
                theme.text
            } else {
                Color::Green
            };
            (
                Style::default().fg(fg).bg(bg),
                Style::default().fg(Color::Green).bg(bg),
                '+',
            )
        }
        RowKind::Del => {
            let bg = theme.diff_del_bg;
            let fg = if highlighted { theme.text } else { Color::Red };
            (
                Style::default().fg(fg).bg(bg),
                Style::default().fg(Color::Red).bg(bg),
                '-',
            )
        }
        _ => (plain, plain.fg(theme.text_muted), ' '),
    }
}

fn header_style(theme: &Theme, kind: RowKind) -> Style {
    let bg = theme.surface_raised;
    match kind {
        RowKind::File => Style::default()
            .fg(theme.text)
            .bg(bg)
            .add_modifier(Modifier::BOLD),
        RowKind::Hunk => Style::default().fg(theme.accent).bg(bg),
        _ => Style::default().fg(theme.text_muted).bg(bg),
    }
}

/// Draw the diff; `true` when highlighting is not finished (draw again).
pub fn render_diff(theme: &Theme, view: &mut DiffView, area: Rect, buf: &mut Buffer) -> bool {
    if area.width == 0 || area.height == 0 {
        return false;
    }
    let done = view.highlight_visible(theme, usize::from(area.height));
    if view.split_active {
        render_split(theme, view, area, buf);
    } else {
        render_unified(theme, view, area, buf);
    }
    !done
}

fn render_unified(theme: &Theme, view: &DiffView, area: Rect, buf: &mut Buffer) {
    let g = view.gutter();
    for (i, (idx, row)) in view
        .rows
        .iter()
        .enumerate()
        .skip(view.scroll)
        .take(usize::from(area.height))
        .enumerate()
    {
        let y = area.y + i as u16;
        match row.kind {
            RowKind::File | RowKind::Hunk | RowKind::Note | RowKind::Meta => {
                let style = header_style(theme, row.kind);
                fill_row(buf, area.x, y, area.width, style);
                put_code(
                    buf,
                    (area.x, y),
                    usize::from(area.width),
                    &row.text,
                    style,
                    None,
                    false,
                );
            }
            kind => {
                let spans = view.spans_of(idx);
                let (style, sign_style, sign) = diff_line_style(theme, kind, spans.is_some());
                fill_row(buf, area.x, y, area.width, style);
                let n = |v: Option<u32>| v.map_or_else(|| " ".repeat(g), |v| format!("{v:>g$}"));
                let prefix = format!("{} {} {sign} ", n(row.old_no), n(row.new_no));
                let pw = prefix.chars().count();
                buf.set_stringn(
                    area.x,
                    y,
                    &prefix,
                    usize::from(area.width),
                    style.fg(theme.text_muted),
                );
                if let Some(c) = buf.cell_mut((area.x + (pw as u16).saturating_sub(2), y)) {
                    c.set_style(sign_style);
                }
                put_code(
                    buf,
                    (area.x + pw as u16, y),
                    usize::from(area.width).saturating_sub(pw),
                    &row.text,
                    style,
                    spans,
                    false,
                );
            }
        }
    }
}

/// One half of a side-by-side row. `number` is the line number shown in the gutter.
#[allow(clippy::too_many_arguments)]
fn render_cell(
    theme: &Theme,
    view: &DiffView,
    idx: Option<usize>,
    side_new: bool,
    gutter: usize,
    x: u16,
    y: u16,
    width: u16,
    buf: &mut Buffer,
) {
    let Some(idx) = idx else {
        fill_row(buf, x, y, width, Style::default().bg(theme.bg));
        return;
    };
    let row = &view.rows[idx];
    let spans = view.spans_of(idx);
    // A context row is drawn on both sides; only added/removed rows get the tint.
    let (style, sign_style, sign) = diff_line_style(theme, row.kind, spans.is_some());
    fill_row(buf, x, y, width, style);
    let no = if side_new { row.new_no } else { row.old_no };
    let prefix = format!(
        "{} {sign} ",
        no.map_or_else(|| " ".repeat(gutter), |v| format!("{v:>gutter$}"))
    );
    let pw = prefix.chars().count();
    buf.set_stringn(
        x,
        y,
        &prefix,
        usize::from(width),
        style.fg(theme.text_muted),
    );
    if let Some(c) = buf.cell_mut((x + (pw as u16).saturating_sub(2), y)) {
        c.set_style(sign_style);
    }
    put_code(
        buf,
        (x + pw as u16, y),
        usize::from(width).saturating_sub(pw),
        &row.text,
        style,
        spans,
        true,
    );
}

fn render_split(theme: &Theme, view: &DiffView, area: Rect, buf: &mut Buffer) {
    let g = view.gutter();
    let left_w = (area.width.saturating_sub(1)) / 2;
    let right_w = area.width.saturating_sub(1) - left_w;
    let right_x = area.x + left_w + 1;
    for (i, srow) in view
        .split
        .rows
        .iter()
        .skip(view.scroll)
        .take(usize::from(area.height))
        .enumerate()
    {
        let y = area.y + i as u16;
        match *srow {
            SplitRow::Full(idx) => {
                let row = &view.rows[idx];
                let style = header_style(theme, row.kind);
                fill_row(buf, area.x, y, area.width, style);
                put_code(
                    buf,
                    (area.x, y),
                    usize::from(area.width),
                    &row.text,
                    style,
                    None,
                    true,
                );
            }
            SplitRow::Pair { left, right } => {
                render_cell(theme, view, left, false, g, area.x, y, left_w, buf);
                buf.set_string(
                    area.x + left_w,
                    y,
                    "\u{2502}",
                    Style::default().fg(theme.border).bg(theme.surface_raised),
                );
                render_cell(theme, view, right, true, g, right_x, y, right_w, buf);
            }
        }
    }
}

// ── git ─────────────────────────────────────────────────────────────────────

pub fn render_git(app: &mut App, screen: Rect, buf: &mut Buffer) -> Option<(u16, u16)> {
    let theme = app.theme;
    let area = panel_area(app.config.git_panel_display_mode, screen);
    let ui = &app.git;
    let title = if ui.status.is_repo {
        let mut t = format!(
            "Git \u{00B7} {}",
            if ui.status.detached {
                "(detached)"
            } else {
                &ui.status.branch
            }
        );
        if ui.status.ahead > 0 {
            t.push_str(&format!(" \u{2191}{}", ui.status.ahead));
        }
        if ui.status.behind > 0 {
            t.push_str(&format!(" \u{2193}{}", ui.status.behind));
        }
        if ui.stats.files > 0 {
            t.push_str(&format!(
                " \u{00B7} {} files +{} -{}",
                ui.stats.files, ui.stats.additions, ui.stats.deletions
            ));
        }
        t
    } else {
        "Git".to_string()
    };
    let inner = frame(&theme, &title, area, buf);
    if inner.height < 4 {
        return None;
    }
    let labels: Vec<&str> = GitTab::ALL.iter().map(|t| t.label()).collect();
    let idx = GitTab::ALL.iter().position(|t| *t == ui.tab).unwrap_or(0);
    tab_line(&theme, &labels, idx).render(Rect { height: 1, ..inner }, buf);
    let body = Rect::new(
        inner.x,
        inner.y + 2,
        inner.width,
        inner.height.saturating_sub(4),
    );
    let status_row = Rect::new(inner.x, inner.bottom() - 2, inner.width, 1);
    let hint_row = Rect::new(inner.x, inner.bottom() - 1, inner.width, 1);

    let mut cursor = None;
    if let Some(err) = &ui.error
        && ui.tab != GitTab::Pr
    {
        buf.set_stringn(
            body.x + 1,
            body.y + 1,
            err,
            usize::from(body.width.saturating_sub(2)),
            Style::default().fg(theme.error).bg(theme.surface_raised),
        );
        hint(&theme, "r retry \u{00B7} Esc close", hint_row, buf);
        return None;
    }
    if !ui.loaded {
        buf.set_string(
            body.x + 1,
            body.y,
            "Loading\u{2026}",
            Style::default()
                .fg(theme.text_muted)
                .bg(theme.surface_raised),
        );
    }
    // The side-by-side layout needs a wide panel; narrower ones fall back to the unified view.
    let wants_split = ui.layout == as_core::DiffLayout::Split;
    let split_fits = body.width >= SPLIT_MIN_WIDTH;
    app.git.diff.set_split(wants_split && split_fits);
    app.git.pr.diff.set_split(wants_split && split_fits);
    let ui = &app.git;
    let mut more = false;
    match ui.tab {
        GitTab::Changes => {
            let list_w = (body.width / 3).clamp(22, 48).min(body.width);
            let list = Rect::new(body.x, body.y, list_w, body.height);
            let diff = Rect::new(
                body.x + list_w + 1,
                body.y,
                body.width.saturating_sub(list_w + 1),
                body.height,
            );
            let top = ui
                .cursor
                .saturating_sub(usize::from(list.height).saturating_sub(1));
            for (i, f) in ui
                .status
                .files
                .iter()
                .enumerate()
                .skip(top)
                .take(usize::from(list.height))
            {
                let y = list.y + (i - top) as u16;
                let sel = i == ui.cursor;
                let st = row_style(&theme, sel, !ui.diff_focus);
                fill_row(buf, list.x, y, list.width, st);
                let color = match f.kind {
                    GitStatusKind::Added | GitStatusKind::Untracked => Color::Green,
                    GitStatusKind::Deleted | GitStatusKind::Unmerged => Color::Red,
                    _ => Color::Yellow,
                };
                let m = marker(f);
                let mstyle = if sel && !ui.diff_focus {
                    st
                } else {
                    st.fg(color)
                };
                buf.set_string(list.x + 1, y, &m, mstyle);
                let name = fit_left(&f.path, usize::from(list.width.saturating_sub(5)));
                buf.set_stringn(
                    list.x + 4,
                    y,
                    name,
                    usize::from(list.width.saturating_sub(4)),
                    st,
                );
            }
            if ui.status.files.is_empty() && ui.loaded {
                buf.set_string(
                    list.x + 1,
                    list.y,
                    "No changes",
                    Style::default()
                        .fg(theme.text_muted)
                        .bg(theme.surface_raised),
                );
            }
            for y in body.y..body.bottom() {
                buf.set_string(
                    list.right(),
                    y,
                    "\u{2502}",
                    Style::default().fg(theme.border).bg(theme.surface_raised),
                );
            }
            let h = usize::from(diff.height);
            app.git.diff_height = h;
            more = render_diff(&theme, &mut app.git.diff, diff, buf);
        }
        GitTab::Branches => {
            let cur = ui.branch_cursor;
            let top = cur.saturating_sub(usize::from(body.height).saturating_sub(1));
            for (i, b) in ui
                .branches
                .iter()
                .enumerate()
                .skip(top)
                .take(usize::from(body.height))
            {
                let y = body.y + (i - top) as u16;
                let st = row_style(&theme, i == cur, true);
                fill_row(buf, body.x, y, body.width, st);
                let mut text = format!("{} {}", if b.is_current { "*" } else { " " }, b.name);
                if b.is_remote {
                    text.push_str("  (remote)");
                }
                if b.ahead_of_upstream > 0 || b.behind_upstream > 0 {
                    text.push_str(&format!(
                        "  \u{2191}{} \u{2193}{}",
                        b.ahead_of_upstream, b.behind_upstream
                    ));
                }
                buf.set_stringn(
                    body.x + 1,
                    y,
                    text,
                    usize::from(body.width.saturating_sub(2)),
                    st,
                );
            }
            if ui.branches.is_empty() {
                buf.set_string(
                    body.x + 1,
                    body.y,
                    "No branches",
                    Style::default()
                        .fg(theme.text_muted)
                        .bg(theme.surface_raised),
                );
            }
        }
        GitTab::Pr => {
            let pr = &ui.pr;
            let list_w = (body.width / 3).clamp(22, 48).min(body.width);
            let muted = Style::default()
                .fg(theme.text_muted)
                .bg(theme.surface_raised);
            if pr.loading {
                buf.set_string(body.x + 1, body.y, "Loading pull request\u{2026}", muted);
            } else if let Some(e) = &pr.error {
                buf.set_stringn(
                    body.x + 1,
                    body.y,
                    e,
                    usize::from(body.width.saturating_sub(2)),
                    Style::default().fg(theme.error).bg(theme.surface_raised),
                );
            } else if let Some(d) = &pr.data {
                let head = format!(
                    "#{} {} ({} \u{2192} {}) by {}",
                    d.metadata.number,
                    d.metadata.title,
                    d.metadata.head_ref,
                    d.metadata.base_ref,
                    d.metadata.author
                );
                buf.set_stringn(
                    body.x,
                    body.y,
                    head,
                    usize::from(body.width),
                    Style::default()
                        .fg(theme.text)
                        .bg(theme.surface_raised)
                        .add_modifier(Modifier::BOLD),
                );
                let list = Rect::new(body.x, body.y + 1, list_w, body.height.saturating_sub(1));
                let diff = Rect::new(
                    body.x + list_w + 1,
                    body.y + 1,
                    body.width.saturating_sub(list_w + 1),
                    body.height.saturating_sub(1),
                );
                let top = pr
                    .cursor
                    .saturating_sub(usize::from(list.height).saturating_sub(1));
                for (i, f) in d
                    .files
                    .iter()
                    .enumerate()
                    .skip(top)
                    .take(usize::from(list.height))
                {
                    let y = list.y + (i - top) as u16;
                    let st = row_style(&theme, i == pr.cursor, !ui.diff_focus);
                    fill_row(buf, list.x, y, list.width, st);
                    let stats = format!(" +{} -{}", f.additions, f.deletions);
                    let name = fit_left(
                        &f.path,
                        usize::from(list.width).saturating_sub(stats.len() + 1),
                    );
                    buf.set_stringn(
                        list.x + 1,
                        y,
                        format!("{name}{stats}"),
                        usize::from(list.width.saturating_sub(1)),
                        st,
                    );
                }
                app.git.diff_height = usize::from(diff.height);
                more = render_diff(&theme, &mut app.git.pr.diff, diff, buf);
            } else {
                buf.set_string(
                    body.x + 1,
                    body.y,
                    "Press / to enter a pull request number or URL (needs the gh CLI)",
                    muted,
                );
            }
        }
    }
    app.dirty |= more;
    let ui = &mut app.git;
    if let Some(p) = &mut ui.prompt {
        let title = p.title();
        let st = Style::default()
            .fg(theme.accent)
            .bg(theme.surface_raised)
            .add_modifier(Modifier::BOLD);
        fill_row(buf, status_row.x, status_row.y, status_row.width, st);
        let tw = title.chars().count().min(usize::from(status_row.width) / 2);
        buf.set_stringn(status_row.x, status_row.y, &title, tw, st);
        if !p.is_confirm() {
            let input = Rect::new(
                status_row.x + tw as u16 + 2,
                status_row.y,
                status_row.width.saturating_sub(tw as u16 + 2),
                1,
            );
            cursor = p
                .input
                .render(input, buf, Style::default().fg(theme.text).bg(theme.bg));
        }
        hint(&theme, "Enter confirm \u{00B7} Esc cancel", hint_row, buf);
        return cursor;
    }
    if let Some((err, text)) = &ui.notice {
        let color = if *err { theme.error } else { theme.running };
        buf.set_stringn(
            status_row.x,
            status_row.y,
            text,
            usize::from(status_row.width),
            Style::default().fg(color).bg(theme.surface_raised),
        );
    } else if wants_split && !split_fits && ui.tab != GitTab::Branches {
        buf.set_stringn(
            status_row.x,
            status_row.y,
            format!(
                "Side-by-side needs a panel of {SPLIT_MIN_WIDTH}+ columns: showing the unified diff"
            ),
            usize::from(status_row.width),
            Style::default()
                .fg(theme.text_muted)
                .bg(theme.surface_raised),
        );
    }
    let keys = match ui.tab {
        GitTab::Changes if ui.diff_focus => {
            "j/k scroll \u{00B7} n/N hunk \u{00B7} s split/unified \u{00B7} PgUp/PgDn \u{00B7} h back \u{00B7} Tab/1-3 tabs"
        }
        GitTab::Changes => {
            "j/k file \u{00B7} Enter diff \u{00B7} Space stage/unstage \u{00B7} a stage all \u{00B7} c commit \u{00B7} P push \u{00B7} n/N hunk \u{00B7} s split \u{00B7} r refresh \u{00B7} Tab tabs \u{00B7} q close"
        }
        GitTab::Branches => {
            "j/k move \u{00B7} Enter checkout \u{00B7} n new branch \u{00B7} r refresh \u{00B7} Tab tabs \u{00B7} q close"
        }
        GitTab::Pr if ui.diff_focus => {
            "j/k scroll \u{00B7} n/N hunk \u{00B7} s split/unified \u{00B7} h back"
        }
        GitTab::Pr => {
            "/ PR number or URL \u{00B7} j/k file \u{00B7} Enter diff \u{00B7} Tab tabs \u{00B7} q close"
        }
    };
    hint(&theme, keys, hint_row, buf);
    None
}

// ── files ───────────────────────────────────────────────────────────────────

pub fn render_files(app: &mut App, screen: Rect, buf: &mut Buffer) -> Option<(u16, u16)> {
    let theme = app.theme;
    let area = panel_area(app.config.editor_panel_display_mode, screen);
    let root = as_core::paths::tildify(std::path::Path::new(&app.files.root));
    let title = format!(
        "Files \u{00B7} {root}{}",
        if app.files.show_hidden {
            " (hidden shown)"
        } else {
            ""
        }
    );
    let inner = frame(&theme, &title, area, buf);
    if inner.height < 3 {
        return None;
    }
    let body = Rect::new(inner.x, inner.y, inner.width, inner.height - 1);
    let hint_row = Rect::new(inner.x, inner.bottom() - 1, inner.width, 1);
    let tree_w = (body.width * 2 / 5).clamp(20, 60).min(body.width);
    let tree = Rect::new(body.x, body.y, tree_w, body.height);
    let prev = Rect::new(
        body.x + tree_w + 1,
        body.y,
        body.width.saturating_sub(tree_w + 1),
        body.height,
    );
    app.files.preview_height = usize::from(prev.height);
    let more = app
        .files
        .highlight_visible(&theme, usize::from(prev.height.saturating_sub(1)));
    app.dirty |= !more;
    let ui = &app.files;
    let rows = ui.rows();
    let top = ui
        .cursor
        .saturating_sub(usize::from(tree.height).saturating_sub(1));
    for (i, r) in rows
        .iter()
        .enumerate()
        .skip(top)
        .take(usize::from(tree.height))
    {
        let y = tree.y + (i - top) as u16;
        let st = row_style(&theme, i == ui.cursor, !ui.preview_focus);
        fill_row(buf, tree.x, y, tree.width, st);
        let glyph = if r.is_dir {
            if r.expanded { "\u{25BE} " } else { "\u{25B8} " }
        } else {
            "  "
        };
        let text = format!(
            "{}{}{}{}",
            "  ".repeat(r.depth),
            glyph,
            r.name,
            if r.is_dir { "/" } else { "" }
        );
        let st = if r.is_dir && i != ui.cursor {
            st.fg(theme.accent)
        } else {
            st
        };
        buf.set_stringn(
            tree.x + 1,
            y,
            text,
            usize::from(tree.width.saturating_sub(1)),
            st,
        );
    }
    if let Some(e) = &ui.error {
        buf.set_stringn(
            tree.x + 1,
            tree.bottom().saturating_sub(1),
            e,
            usize::from(tree.width),
            Style::default().fg(theme.error).bg(theme.surface_raised),
        );
    } else if rows.is_empty() {
        buf.set_string(
            tree.x + 1,
            tree.y,
            "Empty folder",
            Style::default()
                .fg(theme.text_muted)
                .bg(theme.surface_raised),
        );
    }
    for y in body.y..body.bottom() {
        buf.set_string(
            tree.right(),
            y,
            "\u{2502}",
            Style::default().fg(theme.border).bg(theme.surface_raised),
        );
    }
    match &ui.file {
        None => {
            buf.set_string(
                prev.x + 1,
                prev.y,
                "Enter opens a file \u{00B7} f searches",
                Style::default()
                    .fg(theme.text_muted)
                    .bg(theme.surface_raised),
            );
        }
        Some(f) => {
            let head = Style::default()
                .fg(theme.text)
                .bg(theme.surface_raised)
                .add_modifier(Modifier::BOLD);
            buf.set_stringn(prev.x, prev.y, &f.path, usize::from(prev.width), head);
            let text_area = Rect::new(
                prev.x,
                prev.y + 1,
                prev.width,
                prev.height.saturating_sub(1),
            );
            if let Some(p) = &f.placeholder {
                buf.set_stringn(
                    text_area.x + 1,
                    text_area.y,
                    p,
                    usize::from(text_area.width),
                    Style::default()
                        .fg(theme.text_muted)
                        .bg(theme.surface_raised),
                );
            } else {
                let g = f.lines.len().to_string().len().max(2);
                let num = Style::default()
                    .fg(theme.text_muted)
                    .bg(theme.surface_raised);
                let txt = Style::default().fg(theme.text).bg(theme.surface_raised);
                for (i, line) in f
                    .lines
                    .iter()
                    .enumerate()
                    .skip(f.scroll)
                    .take(usize::from(text_area.height))
                {
                    let y = text_area.y + (i - f.scroll) as u16;
                    buf.set_string(text_area.x, y, format!("{:>g$} ", i + 1), num);
                    let w = usize::from(text_area.width).saturating_sub(g + 2);
                    put_code(
                        buf,
                        (text_area.x + g as u16 + 2, y),
                        w,
                        line,
                        txt,
                        ui.spans(i),
                        false,
                    );
                }
            }
        }
    }
    let keys = if ui.preview_focus {
        "j/k PgUp/PgDn scroll \u{00B7} E editor \u{00B7} y copy path \u{00B7} o open in \u{00B7} f search \u{00B7} h back \u{00B7} Esc"
    } else {
        "j/k move \u{00B7} Enter open/expand \u{00B7} h collapse \u{00B7} . hidden \u{00B7} f search \u{00B7} E editor \u{00B7} y copy path \u{00B7} o open in \u{00B7} q close"
    };
    hint(&theme, keys, hint_row, buf);
    if app.files.search.active {
        return render_search(app, screen, buf);
    }
    None
}

fn render_search(app: &mut App, screen: Rect, buf: &mut Buffer) -> Option<(u16, u16)> {
    let theme = app.theme;
    let area = centered(
        screen,
        (screen.width * 7 / 10).max(40),
        (screen.height * 6 / 10).max(10),
    );
    let inner = frame(&theme, "Search files", area, buf);
    if inner.height < 3 {
        return None;
    }
    let s = &mut app.files.search;
    let input = Rect::new(inner.x + 1, inner.y, inner.width.saturating_sub(2), 1);
    let cursor = s
        .query
        .render(input, buf, Style::default().fg(theme.text).bg(theme.bg));
    let list = Rect::new(inner.x, inner.y + 2, inner.width, inner.height - 3);
    let top = s
        .cursor
        .saturating_sub(usize::from(list.height).saturating_sub(1));
    for (i, h) in s
        .hits
        .iter()
        .enumerate()
        .skip(top)
        .take(usize::from(list.height))
    {
        let y = list.y + (i - top) as u16;
        let st = row_style(&theme, i == s.cursor, true);
        fill_row(buf, list.x, y, list.width, st);
        buf.set_stringn(
            list.x + 1,
            y,
            &h.path,
            usize::from(list.width.saturating_sub(2)),
            st,
        );
    }
    if s.hits.is_empty() {
        buf.set_string(
            list.x + 1,
            list.y,
            "No matches",
            Style::default()
                .fg(theme.text_muted)
                .bg(theme.surface_raised),
        );
    }
    hint(
        &theme,
        "type to filter \u{00B7} \u{2191}/\u{2193} choose \u{00B7} Enter open \u{00B7} Esc cancel",
        Rect::new(
            inner.x + 1,
            inner.bottom() - 1,
            inner.width.saturating_sub(1),
            1,
        ),
        buf,
    );
    cursor
}

// ── open in ─────────────────────────────────────────────────────────────────

pub fn render_open_in(app: &App, screen: Rect, buf: &mut Buffer) {
    let theme = app.theme;
    let ui = &app.open_in;
    let h = (ui.options.len() as u16 + 5).clamp(7, 18);
    let area = centered(screen, 56, h);
    let inner = frame(&theme, "Open in\u{2026}", area, buf);
    buf.set_stringn(
        inner.x + 1,
        inner.y,
        as_core::paths::tildify(std::path::Path::new(&ui.path)),
        usize::from(inner.width.saturating_sub(2)),
        Style::default()
            .fg(theme.text_muted)
            .bg(theme.surface_raised),
    );
    for (i, o) in ui
        .options
        .iter()
        .enumerate()
        .take(usize::from(inner.height.saturating_sub(3)))
    {
        let y = inner.y + 2 + i as u16;
        let st = row_style(&theme, i == ui.cursor, true);
        fill_row(buf, inner.x, y, inner.width, st);
        buf.set_stringn(
            inner.x + 1,
            y,
            &o.label,
            usize::from(inner.width.saturating_sub(2)),
            st,
        );
    }
    if !ui.loaded {
        buf.set_string(
            inner.x + 1,
            inner.y + 2,
            "Looking for editors\u{2026}",
            Style::default()
                .fg(theme.text_muted)
                .bg(theme.surface_raised),
        );
    }
    hint(
        &theme,
        "\u{2191}/\u{2193} choose \u{00B7} Enter open \u{00B7} Esc cancel",
        Rect::new(
            inner.x + 1,
            inner.bottom() - 1,
            inner.width.saturating_sub(1),
            1,
        ),
        buf,
    );
}

// ── settings ────────────────────────────────────────────────────────────────

pub fn render_settings(app: &mut App, screen: Rect, buf: &mut Buffer) -> Option<(u16, u16)> {
    let theme = app.theme;
    let area = centered(
        screen,
        (screen.width * 9 / 10).max(50),
        (screen.height * 9 / 10).max(14),
    );
    let inner = frame(&theme, "Settings", area, buf);
    if inner.height < 4 {
        return None;
    }
    let ui = &mut app.settings;
    let side_w = 24.min(inner.width / 3);
    let side = Rect::new(inner.x, inner.y, side_w, inner.height - 2);
    let body = Rect::new(
        inner.x + side_w + 1,
        inner.y,
        inner.width.saturating_sub(side_w + 1),
        inner.height - 2,
    );
    let status_row = Rect::new(inner.x, inner.bottom() - 2, inner.width, 1);
    let hint_row = Rect::new(inner.x, inner.bottom() - 1, inner.width, 1);
    for (i, s) in Section::ALL.iter().enumerate() {
        let y = side.y + i as u16;
        if y >= side.bottom() {
            break;
        }
        let st = row_style(&theme, i == ui.section, !ui.in_items);
        fill_row(buf, side.x, y, side.width, st);
        buf.set_stringn(
            side.x + 1,
            y,
            s.label(),
            usize::from(side.width.saturating_sub(1)),
            st,
        );
    }
    for y in inner.y..inner.bottom() - 2 {
        buf.set_string(
            side.right(),
            y,
            "\u{2502}",
            Style::default().fg(theme.border).bg(theme.surface_raised),
        );
    }
    let muted = Style::default()
        .fg(theme.text_muted)
        .bg(theme.surface_raised);
    let normal = Style::default().fg(theme.text).bg(theme.surface_raised);
    let mut cursor = None;
    match ui.current() {
        Section::Skills => {
            let labels = ["Claude", "Codex"];
            let active = usize::from(ui.skills_provider == as_core::ProviderId::Codex);
            let mut line = tab_line(&theme, &labels, active);
            line.spans.push(Span::styled("  Tab switches", muted));
            line.render(Rect { height: 1, ..body }, buf);
            let list = Rect::new(
                body.x,
                body.y + 2,
                body.width,
                body.height.saturating_sub(2),
            );
            if let Some(e) = &ui.skills_error {
                buf.set_stringn(
                    list.x,
                    list.y,
                    e,
                    usize::from(list.width),
                    Style::default().fg(theme.error).bg(theme.surface_raised),
                );
            } else if ui.skills.is_none() {
                buf.set_string(list.x, list.y, "Scanning\u{2026}", muted);
            } else if ui.skill_list().is_empty() {
                buf.set_string(list.x, list.y, "No skills found", muted);
            }
            let cur = ui.cursor;
            let top = cur.saturating_sub(usize::from(list.height).saturating_sub(1));
            for (i, s) in ui
                .skill_list()
                .iter()
                .enumerate()
                .skip(top)
                .take(usize::from(list.height))
            {
                let y = list.y + (i - top) as u16;
                let st = row_style(&theme, i == cur && ui.in_items, true);
                fill_row(buf, list.x, y, list.width, st);
                let text = format!(
                    "{} {}  \u{00B7} {}  {}",
                    if s.enabled { "[x]" } else { "[ ]" },
                    s.name,
                    s.group,
                    s.description
                );
                buf.set_stringn(list.x, y, text, usize::from(list.width), st);
            }
        }
        Section::Keybindings => {
            buf.set_string(
                body.x,
                body.y,
                "Effective bindings \u{00B7} Enter opens keybindings.json in $EDITOR",
                muted,
            );
            let list = Rect::new(
                body.x,
                body.y + 2,
                body.width,
                body.height.saturating_sub(2),
            );
            let top = ui
                .cursor
                .saturating_sub(usize::from(list.height).saturating_sub(1));
            for (i, (label, keys)) in ui
                .bindings
                .iter()
                .enumerate()
                .skip(top)
                .take(usize::from(list.height))
            {
                let y = list.y + (i - top) as u16;
                let st = row_style(&theme, i == ui.cursor && ui.in_items, true);
                fill_row(buf, list.x, y, list.width, st);
                let kw = 28.min(usize::from(list.width) / 2);
                buf.set_stringn(
                    list.x,
                    y,
                    label,
                    usize::from(list.width).saturating_sub(kw + 1),
                    st,
                );
                buf.set_stringn(
                    list.x + list.width.saturating_sub(kw as u16),
                    y,
                    keys,
                    kw,
                    st,
                );
            }
        }
        Section::Profiles => {
            buf.set_string(
                body.x,
                body.y,
                "Claude profiles (project mappings: prefix P)",
                muted,
            );
            for (i, p) in ui
                .profiles
                .iter()
                .enumerate()
                .take(usize::from(body.height.saturating_sub(2)))
            {
                let mapped: Vec<_> = ui
                    .config
                    .claude_profile_by_project
                    .iter()
                    .filter(|(_, v)| **v == p.config_dir || (p.is_default && v.is_empty()))
                    .map(|(k, _)| k.as_str())
                    .collect();
                let text = format!(
                    "{}{}  {}  {}",
                    p.label,
                    if p.is_default { " (default)" } else { "" },
                    as_core::paths::tildify(std::path::Path::new(&p.config_dir)),
                    if mapped.is_empty() {
                        String::new()
                    } else {
                        format!("\u{2192} {}", mapped.join(", "))
                    }
                );
                buf.set_stringn(
                    body.x,
                    body.y + 2 + i as u16,
                    text,
                    usize::from(body.width),
                    normal,
                );
            }
        }
        Section::Data => {
            let items = ["Import from Hydra\u{2026}", "Export diagnostics bundle"];
            for (i, t) in items.iter().enumerate() {
                let y = body.y + i as u16;
                let st = row_style(&theme, i == ui.cursor && ui.in_items, true);
                fill_row(buf, body.x, y, body.width, st);
                buf.set_stringn(
                    body.x + 1,
                    y,
                    t,
                    usize::from(body.width.saturating_sub(1)),
                    st,
                );
            }
            buf.set_stringn(
                body.x,
                body.y + 3,
                "Import reads Hydra's config, workspace and keybindings (never modifies them).",
                usize::from(body.width),
                muted,
            );
            buf.set_stringn(
                body.x,
                body.y + 4,
                "Diagnostics: logs + a snapshot without credentials, in <data dir>/diagnostics.",
                usize::from(body.width),
                muted,
            );
        }
        Section::About => {
            let mut lines = vec![
                format!("Agent Studio {}", as_core::VERSION),
                format!(
                    "Data dir: {}",
                    as_core::paths::tildify(std::path::Path::new(&ui.data_dir))
                ),
            ];
            match &ui.status {
                Some(s) => {
                    lines.push(format!(
                        "Daemon: pid {} \u{00B7} v{} \u{00B7} up {}s",
                        s.pid, s.version, s.uptime_secs
                    ));
                    lines.push(format!(
                        "Agents: {} ({} running) \u{00B7} clients {}",
                        s.agent_count, s.running_count, s.clients
                    ));
                }
                None => lines.push("Daemon: asking\u{2026}".into()),
            }
            lines.push(format!("MCP: {}", ui.mcp));
            match &ui.update {
                Some(u) => {
                    let mut line = format!("Updates: {}", u.summary());
                    if !u.enabled {
                        line.push_str(" (automatic checks off)");
                    }
                    lines.push(line);
                    if u.available().is_some() {
                        lines.push(format!("  {}", u.hint));
                    }
                }
                None => lines.push("Updates: unknown".into()),
            }
            lines.push(String::new());
            lines.push("Enter / c: check for updates now".into());
            for (i, l) in lines.iter().enumerate() {
                buf.set_stringn(
                    body.x,
                    body.y + i as u16,
                    l,
                    usize::from(body.width),
                    normal,
                );
            }
        }
        section => {
            let items = section.items();
            let top = ui
                .cursor
                .saturating_sub(usize::from(body.height).saturating_sub(1));
            for (i, it) in items
                .iter()
                .enumerate()
                .skip(top)
                .take(usize::from(body.height))
            {
                let y = body.y + (i - top) as u16;
                let sel = i == ui.cursor && ui.in_items;
                let st = row_style(&theme, sel, true);
                fill_row(buf, body.x, y, body.width, st);
                buf.set_stringn(body.x + 1, y, it.label, 28, st);
                let vx = body.x + 30;
                let vw = usize::from(body.width).saturating_sub(31);
                if sel && let Some(edit) = &mut ui.edit {
                    let input = Rect::new(vx, y, vw as u16, 1);
                    cursor = edit.render(input, buf, Style::default().fg(theme.text).bg(theme.bg));
                } else {
                    let v = ui.display(it);
                    let shown = match it.kind {
                        Kind::Choice(_) => format!("\u{2039} {v} \u{203A}"),
                        _ => v,
                    };
                    buf.set_stringn(vx, y, shown, vw, st);
                }
            }
            if let Some(it) = items.get(ui.cursor) {
                let y = body.bottom().saturating_sub(1);
                buf.set_stringn(body.x, y, it.help, usize::from(body.width), muted);
            }
        }
    }
    if let Some(e) = &ui.error {
        buf.set_stringn(
            status_row.x,
            status_row.y,
            e,
            usize::from(status_row.width),
            Style::default().fg(theme.error).bg(theme.surface_raised),
        );
    } else if let Some(n) = &ui.notice {
        buf.set_stringn(
            status_row.x,
            status_row.y,
            n,
            usize::from(status_row.width),
            Style::default().fg(theme.running).bg(theme.surface_raised),
        );
    }
    let keys = if ui.edit.is_some() {
        "type \u{00B7} Enter save \u{00B7} Esc cancel"
    } else if !ui.in_items {
        "j/k section \u{00B7} Enter/\u{2192} open \u{00B7} Esc close"
    } else {
        match ui.current() {
            Section::Skills => {
                "j/k move \u{00B7} Space toggle \u{00B7} Tab Claude/Codex \u{00B7} r rescan \u{00B7} Esc back"
            }
            Section::Keybindings => {
                "j/k move \u{00B7} Enter edit keybindings.json \u{00B7} Esc back"
            }
            Section::Data => "j/k move \u{00B7} Enter run \u{00B7} Esc back",
            _ => {
                "j/k move \u{00B7} Space/\u{2190}/\u{2192} change \u{00B7} Enter edit text \u{00B7} Esc back"
            }
        }
    };
    hint(&theme, keys, hint_row, buf);
    if ui.import.is_some() {
        render_import(&theme, &app.settings, screen, buf);
    }
    cursor
}

fn render_import(
    theme: &Theme,
    ui: &crate::panels::settings::SettingsUi,
    screen: Rect,
    buf: &mut Buffer,
) {
    let area = centered(
        screen,
        (screen.width * 7 / 10).max(50),
        (screen.height * 7 / 10).max(14),
    );
    let inner = frame(theme, "Import from Hydra", area, buf);
    let st = Style::default().fg(theme.text).bg(theme.surface_raised);
    let muted = Style::default()
        .fg(theme.text_muted)
        .bg(theme.surface_raised);
    let mut lines: Vec<(String, Style)> = Vec::new();
    let mut hint_text = "Esc close";
    match &ui.import {
        Some(ImportStage::Loading) => lines.push(("Reading Hydra's data\u{2026}".into(), muted)),
        Some(ImportStage::Applying) => lines.push(("Importing\u{2026}".into(), muted)),
        Some(ImportStage::Failed(e)) => lines.push((
            e.clone(),
            Style::default().fg(theme.error).bg(theme.surface_raised),
        )),
        Some(ImportStage::Preview(p)) => {
            lines.push((format!("From {}", p.dir), muted));
            lines.push((
                format!("Agents: {} new of {}", p.agents_new, p.agents_total),
                st,
            ));
            lines.push((
                format!(
                    "Keybindings: {} of {} importable",
                    p.keybindings_importable, p.keybindings_total
                ),
                st,
            ));
            lines.push((
                format!("Config values that differ: {}", p.config_changes.len()),
                st,
            ));
            for c in p
                .config_changes
                .iter()
                .take(usize::from(inner.height).saturating_sub(9))
            {
                lines.push((
                    format!("  {}: {} \u{2192} {}", c.key, c.current, c.incoming),
                    muted,
                ));
            }
            for w in &p.warnings {
                lines.push((
                    format!("! {w}"),
                    Style::default().fg(theme.waiting).bg(theme.surface_raised),
                ));
            }
            hint_text = "Enter import \u{00B7} Esc cancel";
        }
        Some(ImportStage::Done(r)) => {
            lines.push((
                format!(
                    "Imported {} config value(s), {} agent(s), {} keybinding(s)",
                    r.config_keys_applied,
                    r.agents_added.len(),
                    r.keybindings_added
                ),
                Style::default().fg(theme.running).bg(theme.surface_raised),
            ));
            for w in &r.warnings {
                lines.push((format!("! {w}"), muted));
            }
            hint_text = "Enter close";
        }
        None => {}
    }
    for (i, (t, s)) in lines
        .iter()
        .enumerate()
        .take(usize::from(inner.height).saturating_sub(2))
    {
        buf.set_stringn(
            inner.x + 1,
            inner.y + i as u16,
            t,
            usize::from(inner.width.saturating_sub(2)),
            *s,
        );
    }
    hint(
        theme,
        hint_text,
        Rect::new(
            inner.x + 1,
            inner.bottom() - 1,
            inner.width.saturating_sub(1),
            1,
        ),
        buf,
    );
}

// ── free terminals ──────────────────────────────────────────────────────────

/// The terminals panel; returns the cursor of the active pane.
pub fn render_free_terms(app: &mut App, buf: &mut Buffer) -> Option<(u16, u16)> {
    let theme = app.theme;
    let ft = &mut app.free_terms;
    if !ft.open {
        return None;
    }
    let geo = ft.geometry();
    Clear.render(geo.panel, buf);
    let project = std::path::Path::new(&ft.project_dir)
        .file_name()
        .map_or_else(
            || ft.project_dir.clone(),
            |n| n.to_string_lossy().into_owned(),
        );
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.accent))
        .title(Span::styled(
            format!(" Terminals \u{00B7} {project} "),
            Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
        ))
        .style(Style::default().fg(theme.text).bg(theme.term_bg));
    block.render(geo.panel, buf);
    // Tabs.
    let mut x = geo.tabs.x;
    let layout = ft.layout().cloned();
    if let Some(l) = &layout {
        for g in &l.groups {
            let active = l.active_group_id.as_ref() == Some(&g.id);
            let name = ft.label_of(&g.active_pane_id);
            let extra = if g.pane_ids.len() > 1 {
                format!(" |{}", g.pane_ids.len())
            } else {
                String::new()
            };
            let text = format!(" {name}{extra} ");
            let st = if active {
                Style::default()
                    .fg(theme.accent_text)
                    .bg(theme.accent)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
                    .fg(theme.text_secondary)
                    .bg(theme.surface_raised)
            };
            let w = text.chars().count() as u16;
            if x + w > geo.tabs.right() {
                break;
            }
            buf.set_string(x, geo.tabs.y, &text, st);
            x += w + 1;
        }
    }
    let mut cursor = None;
    if let Some((_, input)) = &mut ft.rename {
        let r = Rect::new(geo.tabs.x, geo.tabs.y, geo.tabs.width.min(40), 1);
        buf.set_string(
            r.x,
            r.y,
            "rename: ",
            Style::default().fg(theme.accent).bg(theme.term_bg),
        );
        let inp = Rect::new(r.x + 8, r.y, r.width.saturating_sub(8), 1);
        cursor = input.render(inp, buf, Style::default().fg(theme.text).bg(theme.bg));
    } else if layout.as_ref().is_none_or(|l| l.groups.is_empty()) {
        buf.set_string(
            geo.tabs.x,
            geo.tabs.y,
            "starting a shell\u{2026}",
            Style::default().fg(theme.text_muted).bg(theme.term_bg),
        );
    }
    let (panes, seps) = ft.pane_rects();
    let active = ft.active_pane();
    let active_rect = active
        .as_ref()
        .and_then(|a| panes.iter().find(|(id, _)| id == a))
        .map(|(_, r)| *r);
    for sep in &seps {
        // The dividers around the active pane are drawn in the accent color.
        let vertical = sep.width == 1;
        let touches = active_rect.is_some_and(|a| {
            if vertical {
                (a.right() == sep.x || a.x == sep.x + 1) && a.y < sep.bottom() && sep.y < a.bottom()
            } else {
                (a.bottom() == sep.y || a.y == sep.y + 1) && a.x < sep.right() && sep.x < a.right()
            }
        });
        let color = if touches && panes.len() > 1 {
            theme.accent
        } else {
            theme.border
        };
        let glyph = if vertical { "\u{2502}" } else { "\u{2500}" };
        for y in sep.y..sep.bottom() {
            for x in sep.x..sep.right() {
                buf.set_string(x, y, glyph, Style::default().fg(color).bg(theme.term_bg));
            }
        }
    }
    for (id, rect) in &panes {
        let Some(t) = ft.terms.get(id) else { continue };
        if rect.width == 0 || rect.height == 0 {
            continue;
        }
        t.emu.render(*rect, buf, theme.term_palette());
        match t.sub {
            SubState::Requested(_) => buf.set_string(
                rect.x,
                rect.y,
                " attaching\u{2026} ",
                Style::default().fg(theme.text_muted).bg(theme.term_bg),
            ),
            SubState::Failed => buf.set_string(
                rect.x,
                rect.y,
                " could not attach ",
                Style::default().fg(theme.error).bg(theme.term_bg),
            ),
            SubState::Live => {}
        }
        if cursor.is_none()
            && active.as_ref() == Some(id)
            && let Some((c, r)) = t.emu.cursor()
        {
            cursor = Some((
                rect.x + c.min(rect.width.saturating_sub(1)),
                rect.y + r.min(rect.height.saturating_sub(1)),
            ));
        }
    }
    cursor
}
