//! The headless runs panel: run list and log viewer (Hydra `HeadlessPanel.tsx`).

use as_core::headless::{HeadlessRun, HeadlessRunStatus};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Widget};

use crate::app::App;
use crate::headless::render::{Kind, wrap};
use crate::sessions::{format_age, one_line};
use crate::theme::Theme;
use crate::ui::sidebar::fit_left;

pub fn status_glyph(t: &Theme, status: HeadlessRunStatus) -> (&'static str, Color) {
    match status {
        HeadlessRunStatus::Running => ("\u{25D0}", t.accent),
        HeadlessRunStatus::Completed => ("\u{2714}", t.running),
        HeadlessRunStatus::Errored => ("\u{2716}", t.error),
        HeadlessRunStatus::Canceled => ("\u{2298}", t.text_muted),
    }
}

fn kind_style(t: &Theme, kind: Kind) -> Style {
    match kind {
        Kind::Text | Kind::Raw => Style::default().fg(t.text),
        Kind::Tool => Style::default().fg(t.accent),
        Kind::ToolResult | Kind::Meta => Style::default().fg(t.text_muted),
        Kind::Done => Style::default().fg(t.running).add_modifier(Modifier::BOLD),
        Kind::Error => Style::default().fg(t.error),
        Kind::Stderr => Style::default().fg(t.starting),
    }
}

fn fit(text: &str, width: usize) -> String {
    let n = text.chars().count();
    if n <= width {
        return format!("{text:<width$}");
    }
    let head: String = text.chars().take(width.saturating_sub(1)).collect();
    format!("{head}\u{2026}")
}

fn basename(path: &str) -> String {
    std::path::Path::new(path)
        .file_name()
        .map_or_else(|| path.to_string(), |n| n.to_string_lossy().into_owned())
}

pub fn render(app: &mut App, screen: Rect, buf: &mut Buffer) {
    let t = app.theme;
    let width = screen.width.min(140);
    let height = screen.height.saturating_sub(2).max(8).min(screen.height);
    let area = crate::ui::dialogs::centered(screen, width, height);
    Clear.render(area, buf);
    let title = match &app.headless.view {
        Some(v) => format!(" Headless run {} ", v.run_id),
        None => " Headless Runs ".to_string(),
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(t.accent))
        .title(Span::styled(
            title,
            Style::default().fg(t.text).add_modifier(Modifier::BOLD),
        ))
        .style(Style::default().fg(t.text).bg(t.surface_raised));
    let inner = block.inner(area);
    block.render(area, buf);
    if inner.width < 30 || inner.height < 4 {
        return;
    }
    let content = Rect::new(inner.x + 1, inner.y, inner.width - 2, inner.height);
    if app.headless.view.is_some() {
        log(app, content, buf);
    } else {
        list(app, content, buf);
    }
}

fn list(app: &App, area: Rect, buf: &mut Buffer) {
    let t = &app.theme;
    let h = &app.headless;
    let muted = Style::default().fg(t.text_muted);
    let visible = h.visible();

    // Search + status chip.
    let query = h.query.text();
    let mut spans = vec![Span::styled("/ ", Style::default().fg(t.accent))];
    if query.is_empty() && !h.filtering {
        spans.push(Span::styled(
            "search id, prompt, project, model, session\u{2026}",
            muted,
        ));
    } else {
        spans.push(Span::raw(query.clone()));
        if h.filtering {
            spans.push(Span::styled("\u{258F}", Style::default().fg(t.accent)));
        }
    }
    buf.set_line(area.x, area.y, &Line::from(spans), area.width);
    let chip = format!(" status: {} ", h.status.label());
    let chip_x = area.right().saturating_sub(chip.chars().count() as u16);
    buf.set_string(
        chip_x,
        area.y,
        chip,
        Style::default().fg(t.text).bg(t.surface_hover),
    );

    let list_h = usize::from(area.height.saturating_sub(3));
    let body_y = area.y + 2;
    if visible.is_empty() {
        let msg = match (h.loaded, &h.error) {
            (_, Some(e)) => format!("Could not load the runs: {e}"),
            (false, _) => "Loading\u{2026}".to_string(),
            (true, _) if h.runs.is_empty() => "No runs yet. Press n to start one.".to_string(),
            _ => "No runs match the current filter.".to_string(),
        };
        buf.set_stringn(area.x, body_y, msg, usize::from(area.width), muted);
    }
    let offset = (h.cursor + 1).saturating_sub(list_h.max(1));
    let now = app.overview.clock().now;
    let w = usize::from(area.width);
    for (row, idx) in visible.iter().enumerate().skip(offset).take(list_h) {
        let run = &h.runs[*idx];
        let y = body_y + u16::try_from(row - offset).unwrap_or(0);
        let selected = row == h.cursor;
        let base = if selected {
            Style::default().fg(t.text).bg(t.surface_hover)
        } else {
            Style::default().fg(t.text).bg(t.surface_raised)
        };
        for x in area.x..area.right() {
            if let Some(c) = buf.cell_mut((x, y)) {
                c.set_symbol(" ");
                c.set_style(base);
            }
        }
        let (glyph, color) = status_glyph(t, run.status);
        let started = chrono::DateTime::parse_from_rfc3339(&run.started_at)
            .map_or(0, |d| d.timestamp_millis());
        let age = format_age(started, now);
        let model = fit(&format!("{}/{}", run.provider, run.model), 22);
        let project = fit(&basename(&run.project_dir), 16);
        let used = 2 + 6 + 23 + 17;
        let prompt = one_line(&run.prompt);
        let spans = vec![
            Span::styled(format!("{glyph} "), Style::default().fg(color)),
            Span::styled(format!("{age:<5} "), base.fg(t.text_secondary)),
            Span::styled(format!("{model} "), base.fg(t.text_secondary)),
            Span::styled(format!("{project} "), base.fg(t.text_secondary)),
            Span::styled(
                fit(&prompt, w.saturating_sub(used)).trim_end().to_string(),
                base.add_modifier(if selected {
                    Modifier::BOLD
                } else {
                    Modifier::empty()
                }),
            ),
        ];
        buf.set_line(area.x, y, &Line::from(spans), area.width);
    }
    let footer = format!("{} of {} runs", visible.len(), h.runs.len());
    buf.set_string(area.x, area.bottom() - 1, footer, muted);
}

fn log(app: &mut App, area: Rect, buf: &mut Buffer) {
    let t = app.theme;
    let muted = Style::default().fg(t.text_muted);
    let now = app.overview.clock().now;
    let run: Option<HeadlessRun> = app
        .headless
        .view
        .as_ref()
        .and_then(|v| app.headless.run(&v.run_id))
        .cloned();
    let mut y = area.y;
    if let Some(run) = &run {
        let (glyph, color) = status_glyph(&t, run.status);
        let started = chrono::DateTime::parse_from_rfc3339(&run.started_at)
            .map_or(0, |d| d.timestamp_millis());
        let mut spans = vec![
            Span::styled(
                format!("{glyph} {}", run.status.as_str()),
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!(
                    "  {}/{}  started {}",
                    run.provider,
                    run.model,
                    match format_age(started, now).as_str() {
                        "now" => "just now".to_string(),
                        age => format!("{age} ago"),
                    }
                ),
                muted,
            ),
        ];
        if let Some(effort) = &run.reasoning_effort {
            spans.push(Span::styled(format!("  effort {effort}"), muted));
        }
        buf.set_line(area.x, y, &Line::from(spans), area.width);
        y += 1;
        buf.set_stringn(
            area.x,
            y,
            format!(
                "project {}",
                fit_left(
                    &as_core::paths::tildify(std::path::Path::new(&run.project_dir)),
                    usize::from(area.width).saturating_sub(8)
                )
            ),
            usize::from(area.width),
            muted,
        );
        y += 1;
        if let Some(session) = &run.session_id {
            buf.set_stringn(
                area.x,
                y,
                format!("session {session}"),
                usize::from(area.width),
                muted,
            );
            y += 1;
        }
        buf.set_stringn(
            area.x,
            y,
            format!("prompt  {}", one_line(&run.prompt)),
            usize::from(area.width),
            Style::default().fg(t.text_secondary),
        );
        y += 1;
        if let Some(error) = &run.error {
            buf.set_stringn(
                area.x,
                y,
                format!("\u{2716} {}", one_line(error)),
                usize::from(area.width),
                Style::default().fg(t.error),
            );
            y += 1;
        }
    }
    for x in area.x..area.right() {
        if let Some(c) = buf.cell_mut((x, y)) {
            c.set_symbol("\u{2500}");
            c.set_style(muted);
        }
    }
    y += 1;

    let body_h = usize::from(area.bottom().saturating_sub(y + 1));
    let width = usize::from(area.width);
    let Some(view) = app.headless.view.as_mut() else {
        return;
    };
    view.width = width;
    let footer;
    if !view.loaded {
        buf.set_string(area.x, y, "Loading the log\u{2026}", muted);
        footer = String::new();
    } else if let Some(error) = &view.error {
        buf.set_stringn(
            area.x,
            y,
            format!("Could not read the log: {error}"),
            width,
            Style::default().fg(t.error),
        );
        footer = String::new();
    } else {
        // Wrap from the end until the window (plus what is scrolled away) is filled.
        let need = body_h + view.up;
        let mut gathered: Vec<(Kind, String)> = Vec::new();
        for row in view.rows().iter().rev() {
            let mut pieces = wrap(&row.text, width);
            pieces.reverse();
            gathered.extend(pieces.into_iter().map(|p| (row.kind, p)));
            if gathered.len() >= need {
                break;
            }
        }
        gathered.reverse();
        let max_up = gathered.len().saturating_sub(body_h);
        view.up = view.up.min(max_up);
        let end = gathered.len() - view.up;
        let start = end.saturating_sub(body_h);
        if gathered.is_empty() {
            buf.set_string(area.x, y, "(no log output yet)", muted);
        }
        for (i, (kind, text)) in gathered[start..end].iter().enumerate() {
            buf.set_stringn(
                area.x,
                y + u16::try_from(i).unwrap_or(0),
                text,
                width,
                kind_style(&t, *kind),
            );
        }
        footer = format!(
            "{} \u{00B7} {} \u{00B7} {} line{}{}",
            if view.raw { "raw" } else { "rendered" },
            if view.follow() {
                "following"
            } else {
                "scrolled (G to follow)"
            },
            view.total_lines,
            if view.total_lines == 1 { "" } else { "s" },
            if view.truncated {
                " (older lines cut)"
            } else {
                ""
            },
        );
    }
    buf.set_stringn(area.x, area.bottom() - 1, footer, width, muted);
}
