//! Modal dialogs: new agent, rename, remove confirmation, quit, help.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph, Widget};

use crate::app::{App, Modal, ProfileChooser, QuitChoice};
use crate::new_agent::{Field, NewAgentForm};
use crate::sessions::{SessionPicker, format_age, one_line};
use crate::theme::Theme;
use crate::ui::sidebar::fit_left;

pub fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let w = width.min(area.width);
    let h = height.min(area.height);
    Rect {
        x: area.x + (area.width - w) / 2,
        y: area.y + (area.height - h) / 2,
        width: w,
        height: h,
    }
}

fn frame(theme: &Theme, title: &str, area: Rect, buf: &mut Buffer) -> Rect {
    Clear.render(area, buf);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.accent))
        .title(Span::styled(
            format!(" {title} "),
            Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
        ))
        .style(Style::default().fg(theme.text).bg(theme.surface_raised));
    let inner = block.inner(area);
    block.render(area, buf);
    inner
}

/// Minimum canvas the dialogs are laid out on; on smaller screens they are clipped, not squeezed.
const CANVAS_MIN: (u16, u16) = (80, 40);

/// Draw the active modal; returns the cursor position for text fields.
///
/// Dialogs are drawn on an off-screen canvas that is at least [`CANVAS_MIN`] big and then copied
/// back, so a tiny terminal clips them instead of making the layout code write out of bounds.
pub fn render(app: &mut App, screen: Rect, buf: &mut Buffer) -> Option<(u16, u16)> {
    let theme = app.theme;
    // Take the modal out so the dialog code can borrow the rest of the app freely.
    let mut modal = app.modal.take()?;
    let canvas_area = Rect::new(
        0,
        0,
        screen.width.max(CANVAS_MIN.0),
        screen.height.max(CANVAS_MIN.1),
    );
    let mut canvas = Buffer::empty(canvas_area);
    for y in 0..screen.height {
        for x in 0..screen.width {
            if let (Some(src), Some(dst)) = (
                buf.cell((screen.x + x, screen.y + y)),
                canvas.cell_mut((x, y)),
            ) {
                *dst = src.clone();
            }
        }
    }
    let canvas_screen = canvas_area;
    // What is really on screen (the canvas may be bigger): popups that should fit it use this.
    let visible = Rect::new(0, 0, screen.width, screen.height);
    let cursor = match &mut modal {
        Modal::NewAgent(form) => {
            let cursor = new_agent(&theme, form, canvas_screen, &mut canvas);
            match &mut form.picker {
                Some(picker) => session_picker(&theme, picker, visible, &mut canvas),
                None => cursor,
            }
        }
        Modal::Rename { input, .. } => rename(&theme, input, canvas_screen, &mut canvas),
        Modal::ConfirmRemove { name, .. } => {
            confirm_remove(&theme, name, canvas_screen, &mut canvas);
            None
        }
        Modal::Quit {
            running,
            choice,
            stopping,
        } => {
            quit(
                &theme,
                *running,
                *choice,
                *stopping,
                canvas_screen,
                &mut canvas,
            );
            None
        }
        Modal::Help => {
            help(app, visible, &mut canvas);
            None
        }
        Modal::Palette(p) => palette(&theme, p, visible, &mut canvas),
        Modal::Resume(picker) => session_picker(&theme, picker, visible, &mut canvas),
        Modal::Profile(chooser) => {
            profile_chooser(&theme, chooser, canvas_screen, &mut canvas);
            None
        }
    };
    app.modal = Some(modal);
    for y in 0..screen.height {
        for x in 0..screen.width {
            if let (Some(src), Some(dst)) = (
                canvas.cell((x, y)),
                buf.cell_mut((screen.x + x, screen.y + y)),
            ) {
                *dst = src.clone();
            }
        }
    }
    cursor
        .filter(|(x, y)| *x < screen.width && *y < screen.height)
        .map(|(x, y)| (screen.x + x, screen.y + y))
}

fn label(theme: &Theme, text: &str, active: bool) -> Span<'static> {
    let style = if active {
        Style::default()
            .fg(theme.accent)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.text_secondary)
    };
    Span::styled(format!("{text:<10}"), style)
}

fn selector(theme: &Theme, text: &str, active: bool) -> Vec<Span<'static>> {
    let arrow = Style::default().fg(if active {
        theme.accent
    } else {
        theme.text_muted
    });
    vec![
        Span::styled("\u{2039} ", arrow),
        Span::styled(
            text.to_string(),
            Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" \u{203A}", arrow),
    ]
}

fn input_row(
    theme: &Theme,
    input: &mut crate::widgets::TextArea,
    area: Rect,
    buf: &mut Buffer,
    active: bool,
) -> Option<(u16, u16)> {
    let bg = if active {
        theme.bg
    } else {
        theme.surface_raised
    };
    for x in area.x..area.right() {
        if let Some(c) = buf.cell_mut((x, area.y)) {
            c.set_symbol(" ");
            c.set_style(Style::default().bg(bg));
        }
    }
    let cursor = input.render(area, buf, Style::default().fg(theme.text).bg(bg));
    if active { cursor } else { None }
}

fn new_agent(
    theme: &Theme,
    form: &mut NewAgentForm,
    screen: Rect,
    buf: &mut Buffer,
) -> Option<(u16, u16)> {
    let prompt_h: u16 = 4;
    let show_profile = form.shows_profile();
    let h = 11
        + prompt_h
        + u16::from(form.provider == as_core::ProviderId::Codex)
        + u16::from(show_profile);
    let area = centered(screen, 74, h);
    let inner = frame(theme, "New agent", area, buf);
    let mut cursor = None;
    let mut y = inner.y;
    let x_label = inner.x + 1;
    let x_field = inner.x + 12;
    let w_field = inner.width.saturating_sub(13);
    let row = |y: u16| Rect {
        x: x_field,
        y,
        width: w_field,
        height: 1,
    };
    let active = |f: Field| form.field == f;

    // Name
    buf.set_line(
        x_label,
        y,
        &Line::from(label(theme, "Name", active(Field::Name))),
        11,
    );
    let r = row(y);
    let c = input_row(theme, &mut form.name, r, buf, active(Field::Name));
    cursor = cursor.or(c);
    if form.name.is_empty() && !active(Field::Name) {
        buf.set_string(
            r.x,
            y,
            "auto-generated",
            Style::default()
                .fg(theme.text_muted)
                .bg(theme.surface_raised),
        );
    }
    y += 1;

    // Project dir
    buf.set_line(
        x_label,
        y,
        &Line::from(label(theme, "Directory", active(Field::ProjectDir))),
        11,
    );
    let c = input_row(theme, &mut form.dir, row(y), buf, active(Field::ProjectDir));
    cursor = cursor.or(c);
    y += 1;
    if !form.completions.is_empty() {
        let text = format!("  {}", form.completions.join("  "));
        buf.set_stringn(
            x_field,
            y,
            text,
            usize::from(w_field),
            Style::default()
                .fg(theme.text_muted)
                .bg(theme.surface_raised),
        );
    }
    y += 1;

    // Provider
    let mut spans = vec![label(theme, "Provider", active(Field::Provider))];
    spans.extend(selector(
        theme,
        form.provider.label(),
        active(Field::Provider),
    ));
    buf.set_line(
        x_label,
        y,
        &Line::from(spans),
        inner.width.saturating_sub(1),
    );
    y += 1;

    // Model
    let mut spans = vec![label(theme, "Model", active(Field::Model))];
    if form.model_idx.is_some() {
        spans.extend(selector(theme, &form.model(), active(Field::Model)));
        spans.push(Span::styled(
            "  type for a custom id",
            Style::default().fg(theme.text_muted),
        ));
        buf.set_line(
            x_label,
            y,
            &Line::from(spans),
            inner.width.saturating_sub(1),
        );
    } else {
        buf.set_line(x_label, y, &Line::from(spans), 11);
        let c = input_row(
            theme,
            &mut form.model_text,
            row(y),
            buf,
            active(Field::Model),
        );
        cursor = cursor.or(c);
    }
    y += 1;

    // Effort (codex only)
    if form.provider == as_core::ProviderId::Codex {
        let mut spans = vec![label(theme, "Reasoning", active(Field::Effort))];
        spans.extend(selector(
            theme,
            form.effort().unwrap_or("default"),
            active(Field::Effort),
        ));
        buf.set_line(
            x_label,
            y,
            &Line::from(spans),
            inner.width.saturating_sub(1),
        );
        y += 1;
    }

    // Claude profile (only when there is a choice)
    if show_profile {
        let mut spans = vec![label(theme, "Profile", active(Field::Profile))];
        match (&form.resume, form.resume_profile()) {
            (Some(_), profile) => spans.push(Span::styled(
                format!(
                    "the session's own: {}",
                    profile.map_or("unknown", |p| p.label.as_str())
                ),
                Style::default().fg(theme.text_muted),
            )),
            (None, _) => {
                let current = form.profiles.get(form.profile_idx);
                spans.extend(selector(
                    theme,
                    &current.map_or(String::new(), |p| {
                        format!(
                            "{} ({})",
                            p.label,
                            as_core::paths::tildify(std::path::Path::new(&p.config_dir))
                        )
                    }),
                    active(Field::Profile),
                ));
            }
        }
        buf.set_line(
            x_label,
            y,
            &Line::from(spans),
            inner.width.saturating_sub(1),
        );
        y += 1;
    }

    // YOLO
    let mark = if form.yolo { "[x]" } else { "[ ]" };
    let yolo_style = if form.yolo {
        Style::default().fg(theme.yolo).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.text)
    };
    let spans = vec![
        label(theme, "YOLO", active(Field::Yolo)),
        Span::styled(mark, yolo_style),
        Span::styled(
            "  skip permission prompts (Space)",
            Style::default().fg(theme.text_muted),
        ),
    ];
    buf.set_line(
        x_label,
        y,
        &Line::from(spans),
        inner.width.saturating_sub(1),
    );
    y += 1;

    // Resume a previous session
    let mut spans = vec![label(theme, "Resume", active(Field::Resume))];
    match &form.resume {
        Some(session) => {
            let age = crate::sessions::format_age(
                chrono::DateTime::parse_from_rfc3339(&session.modified_at)
                    .map_or(0, |d| d.timestamp_millis()),
                chrono::Utc::now().timestamp_millis(),
            );
            spans.push(Span::styled(
                "[x] ",
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            ));
            let room = usize::from(w_field).saturating_sub(age.chars().count() + 8);
            let prompt = one_line(&session.first_prompt);
            let prompt = if prompt.chars().count() > room {
                format!(
                    "{}\u{2026}",
                    prompt
                        .chars()
                        .take(room.saturating_sub(1))
                        .collect::<String>()
                )
            } else {
                prompt
            };
            spans.push(Span::styled(
                format!("{age} \u{00B7} {prompt}"),
                Style::default().fg(theme.text),
            ));
            if active(Field::Resume) {
                spans.push(Span::styled(
                    "  Space change \u{00B7} Backspace clear",
                    Style::default().fg(theme.text_muted),
                ));
            }
        }
        None => spans.push(Span::styled(
            "[ ] resume a previous session (Space to pick)",
            Style::default().fg(theme.text_muted),
        )),
    }
    buf.set_line(
        x_label,
        y,
        &Line::from(spans),
        inner.width.saturating_sub(1),
    );
    y += 1;

    // Prompt
    buf.set_line(
        x_label,
        y,
        &Line::from(label(theme, "Prompt", active(Field::Prompt))),
        11,
    );
    let prompt_area = Rect {
        x: x_field,
        y,
        width: w_field,
        height: prompt_h,
    };
    let bg = if active(Field::Prompt) {
        theme.bg
    } else {
        theme.surface_raised
    };
    for py in prompt_area.y..prompt_area.bottom() {
        for px in prompt_area.x..prompt_area.right() {
            if let Some(c) = buf.cell_mut((px, py)) {
                c.set_symbol(" ");
                c.set_style(Style::default().bg(bg));
            }
        }
    }
    let c = form
        .prompt
        .render(prompt_area, buf, Style::default().fg(theme.text).bg(bg));
    if active(Field::Prompt) {
        cursor = cursor.or(c);
    } else if form.prompt.is_empty() {
        buf.set_string(
            prompt_area.x,
            y,
            "first message (optional)",
            Style::default().fg(theme.text_muted).bg(bg),
        );
    }
    y += prompt_h + 1;

    // Status / error line
    if form.submitting {
        buf.set_string(
            x_label,
            y,
            "Creating\u{2026}",
            Style::default().fg(theme.starting),
        );
    } else if let Some(error) = &form.error {
        buf.set_stringn(
            x_label,
            y,
            format!("\u{2716} {error}"),
            usize::from(inner.width.saturating_sub(2)),
            Style::default()
                .fg(theme.error)
                .add_modifier(Modifier::BOLD),
        );
    } else {
        buf.set_stringn(
            x_label,
            y,
            "Enter create \u{00B7} Tab completes the directory \u{00B7} Esc cancel",
            usize::from(inner.width.saturating_sub(2)),
            Style::default().fg(theme.text_muted),
        );
    }
    cursor
}

fn rename(
    theme: &Theme,
    input: &mut crate::widgets::TextArea,
    screen: Rect,
    buf: &mut Buffer,
) -> Option<(u16, u16)> {
    let area = centered(screen, 50, 5);
    let inner = frame(theme, "Rename agent", area, buf);
    let field = Rect {
        x: inner.x + 1,
        y: inner.y + 1,
        width: inner.width.saturating_sub(2),
        height: 1,
    };
    let cursor = input_row(theme, input, field, buf, true);
    buf.set_string(
        inner.x + 1,
        inner.y + 2,
        "Enter save \u{00B7} Esc cancel",
        Style::default().fg(theme.text_muted),
    );
    cursor
}

fn confirm_remove(theme: &Theme, name: &str, screen: Rect, buf: &mut Buffer) {
    let area = centered(screen, 54, 6);
    let inner = frame(theme, "Remove agent", area, buf);
    let lines = vec![
        Line::from(vec![
            Span::raw("Remove "),
            Span::styled(
                name.to_string(),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw("? Its process is stopped and it leaves the list."),
        ]),
        Line::raw(""),
        Line::from(vec![
            Span::styled(
                "[y] remove",
                Style::default()
                    .fg(theme.error)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("   "),
            Span::styled("[n] cancel", Style::default().fg(theme.text_secondary)),
        ]),
    ];
    Paragraph::new(lines)
        .wrap(ratatui::widgets::Wrap { trim: true })
        .render(
            Rect {
                x: inner.x + 1,
                width: inner.width.saturating_sub(2),
                ..inner
            },
            buf,
        );
}

fn quit(
    theme: &Theme,
    running: usize,
    choice: QuitChoice,
    stopping: bool,
    screen: Rect,
    buf: &mut Buffer,
) {
    let area = centered(screen, 66, 9);
    let inner = frame(theme, "Quit Agent Studio", area, buf);
    let noun = if running == 1 { "agent" } else { "agents" };
    let content = Rect {
        x: inner.x + 1,
        width: inner.width.saturating_sub(2),
        ..inner
    };
    if stopping {
        Paragraph::new(vec![
            Line::raw(""),
            Line::styled(
                format!("Stopping {running} {noun} and the daemon\u{2026}"),
                Style::default()
                    .fg(theme.starting)
                    .add_modifier(Modifier::BOLD),
            ),
        ])
        .render(content, buf);
        return;
    }
    let btn = |text: &str, this: QuitChoice, danger: bool| {
        let selected = choice == this;
        let style = if selected {
            Style::default()
                .fg(theme.accent_text)
                .bg(if danger { theme.error } else { theme.accent })
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(if danger { theme.error } else { theme.text })
        };
        Span::styled(format!(" {text} "), style)
    };
    let lines = vec![
        Line::from(vec![Span::styled(
            format!("{running} {noun} running."),
            Style::default().add_modifier(Modifier::BOLD),
        )]),
        Line::raw(""),
        Line::from(vec![
            btn("Detach", QuitChoice::Detach, false),
            Span::styled(
                "  keep them running in the background",
                Style::default().fg(theme.text_secondary),
            ),
        ]),
        Line::raw(""),
        Line::from(vec![
            btn("Stop all & quit", QuitChoice::StopAll, true),
            Span::styled(
                "  stop every agent and the daemon",
                Style::default().fg(theme.text_secondary),
            ),
        ]),
        Line::raw(""),
        Line::from(vec![btn("Cancel", QuitChoice::Cancel, false)]),
    ];
    Paragraph::new(lines).render(content, buf);
}

/// The help overlay's lines: every command the user can run, grouped by category, with the keys
/// the *current* keymap binds to it (so overrides from `keybindings.json` show up).
pub fn help_lines(app: &App) -> Vec<Line<'static>> {
    use as_core::keybindings::{CATEGORY_ORDER, COMMANDS};
    let theme = &app.theme;
    let prefix = app.prefix_label();
    let dim = Style::default().fg(theme.text_secondary);
    let mut lines = vec![
        Line::from(vec![
            Span::styled("Prefix: ", dim),
            Span::styled(
                prefix.clone(),
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("  then a key; every other key goes to the agent", dim),
        ]),
        Line::styled(
            "Keys without a prefix work in the sidebar, grid and scroll mode as named below.",
            dim,
        ),
    ];
    for category in CATEGORY_ORDER {
        let entries: Vec<_> = COMMANDS
            .iter()
            .filter(|c| {
                c.category == category
                    && c.implemented
                    && c.show_in_shortcuts
                    && crate::keymap::Command::from_id(c.id).is_some()
            })
            .collect();
        if entries.is_empty() {
            continue;
        }
        lines.push(Line::raw(""));
        lines.push(Line::styled(
            category.to_string(),
            Style::default()
                .fg(theme.text)
                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
        ));
        for c in entries {
            let keys = app.keymap.labels_for(c.id);
            let (text, style) = if keys.is_empty() {
                (
                    "(unbound)".to_string(),
                    Style::default().fg(theme.text_muted),
                )
            } else {
                (keys.join("  "), Style::default().fg(theme.accent))
            };
            lines.push(Line::from(vec![
                Span::styled(format!(" {:<46}", fit_right(&text, 46)), style),
                Span::raw(c.label.to_string()),
            ]));
        }
    }
    lines.push(Line::raw(""));
    lines.push(Line::styled(
        "Edit keys in keybindings.json (palette: Open Keybindings File); changes apply at once.",
        dim,
    ));
    lines
}

/// Keep the head of `text` within `max` columns.
fn fit_right(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let head: String = text.chars().take(max.saturating_sub(1)).collect();
    format!("{head}\u{2026}")
}

fn help(app: &mut App, screen: Rect, buf: &mut Buffer) {
    let lines = help_lines(app);
    let area = centered(screen, 110, screen.height.saturating_sub(2).max(6));
    let theme = app.theme;
    let inner = frame(&theme, "Key bindings", area, buf);
    let body_h = usize::from(inner.height.saturating_sub(1));
    let max_scroll = lines.len().saturating_sub(body_h);
    app.help_scroll = app.help_scroll.min(max_scroll);
    let scroll = app.help_scroll;
    let shown: Vec<Line> = lines.iter().skip(scroll).take(body_h).cloned().collect();
    Paragraph::new(shown).render(
        Rect {
            x: inner.x + 1,
            width: inner.width.saturating_sub(2),
            height: u16::try_from(body_h).unwrap_or(0),
            ..inner
        },
        buf,
    );
    let last = (scroll + body_h).min(lines.len());
    let footer = format!(
        " j/k PgUp/PgDn g/G scroll \u{00B7} Esc close \u{00B7} {}-{} of {}",
        scroll + 1,
        last,
        lines.len()
    );
    if inner.height > 0 {
        buf.set_string(
            inner.x,
            inner.bottom() - 1,
            fit_right(&footer, usize::from(inner.width)),
            Style::default()
                .fg(theme.text_muted)
                .bg(theme.surface_raised),
        );
    }
}

/// The command palette popup; returns the cursor position of the query field.
fn palette(
    theme: &Theme,
    palette: &mut crate::palette::Palette,
    screen: Rect,
    buf: &mut Buffer,
) -> Option<(u16, u16)> {
    let w = screen.width.saturating_sub(4).clamp(20, 84);
    let h = screen.height.saturating_sub(2).clamp(6, 24);
    let area = centered(screen, w, h);
    let inner = frame(theme, "Command palette", area, buf);
    if inner.height < 4 || inner.width < 10 {
        return None;
    }
    // Query row.
    let query_area = Rect {
        x: inner.x + 2,
        width: inner.width.saturating_sub(3),
        height: 1,
        ..inner
    };
    buf.set_string(
        inner.x,
        inner.y,
        "\u{203A} ",
        Style::default().fg(theme.accent).bg(theme.surface_raised),
    );
    let cursor = input_row(theme, &mut palette.query, query_area, buf, true);
    // Result rows.
    let list_h = usize::from(inner.height - 2);
    let n = palette.filtered.len();
    let offset = (palette.cursor + 1).saturating_sub(list_h);
    if n == 0 {
        buf.set_string(
            inner.x + 1,
            inner.y + 1,
            "No matching command or agent",
            Style::default()
                .fg(theme.text_muted)
                .bg(theme.surface_raised),
        );
    }
    for (row, idx) in palette
        .filtered
        .iter()
        .enumerate()
        .skip(offset)
        .take(list_h)
    {
        let entry = &palette.entries[*idx];
        let y = inner.y + 1 + u16::try_from(row - offset).unwrap_or(0);
        let selected = row == palette.cursor;
        let bg = if selected {
            theme.surface_hover
        } else {
            theme.surface_raised
        };
        for x in inner.x..inner.right() {
            if let Some(c) = buf.cell_mut((x, y)) {
                c.set_symbol(" ");
                c.set_style(Style::default().bg(bg));
            }
        }
        let marker = if selected { "\u{25B8} " } else { "  " };
        let right = if entry.hint.is_empty() {
            entry.detail.clone()
        } else {
            entry.hint.clone()
        };
        let right_w = right.chars().count();
        let label_room = usize::from(inner.width).saturating_sub(right_w + 5);
        let label = fit_right(&entry.label, label_room);
        buf.set_string(
            inner.x,
            y,
            format!("{marker}{label}"),
            Style::default()
                .fg(theme.text)
                .bg(bg)
                .add_modifier(if selected {
                    Modifier::BOLD
                } else {
                    Modifier::empty()
                }),
        );
        if right_w + 3 < usize::from(inner.width) {
            let x = inner.right() - u16::try_from(right_w).unwrap_or(0) - 1;
            let color = if entry.hint.is_empty() {
                theme.text_muted
            } else {
                theme.accent
            };
            buf.set_string(x, y, right, Style::default().fg(color).bg(bg));
        }
    }
    let status = format!(
        " {n} match{} \u{00B7} \u{2191}/\u{2193} choose \u{00B7} Enter run \u{00B7} Esc close",
        if n == 1 { "" } else { "es" }
    );
    buf.set_string(
        inner.x,
        inner.bottom() - 1,
        fit_right(&status, usize::from(inner.width)),
        Style::default()
            .fg(theme.text_muted)
            .bg(theme.surface_raised),
    );
    cursor
}

/// The session picker popup (also drawn over the New agent dialog).
fn session_picker(
    theme: &Theme,
    picker: &mut SessionPicker,
    screen: Rect,
    buf: &mut Buffer,
) -> Option<(u16, u16)> {
    let w = screen.width.saturating_sub(6).min(118);
    let h = screen.height.saturating_sub(4).min(30);
    let area = centered(screen, w, h);
    let title = format!("Resume session \u{00B7} {}", picker.provider.label());
    let inner = frame(theme, &title, area, buf);
    let content = Rect {
        x: inner.x + 1,
        width: inner.width.saturating_sub(2),
        ..inner
    };
    let muted = Style::default().fg(theme.text_muted);
    let secondary = Style::default().fg(theme.text_secondary);
    let mut cursor = None;

    // Filter + toggles.
    let toggles = format!(
        "{}  {}  {}",
        if picker.all_ages || picker.max_age_days == 0 {
            "all ages".to_string()
        } else {
            format!("\u{2264} {}d", picker.max_age_days)
        },
        if picker.project_only {
            "this project"
        } else {
            "all projects"
        },
        if picker.show_hidden {
            "hidden shown"
        } else {
            "hidden off"
        },
    );
    let toggles_w = u16::try_from(toggles.chars().count()).unwrap_or(0);
    buf.set_string(
        content.x,
        content.y,
        "/ ",
        Style::default().fg(theme.accent),
    );
    let filter_area = Rect {
        x: content.x + 2,
        y: content.y,
        width: content.width.saturating_sub(toggles_w + 4),
        height: 1,
    };
    if picker.filtering || !picker.query.is_empty() {
        let bg = if picker.filtering {
            theme.bg
        } else {
            theme.surface_raised
        };
        for x in filter_area.x..filter_area.right() {
            if let Some(c) = buf.cell_mut((x, filter_area.y)) {
                c.set_symbol(" ");
                c.set_style(Style::default().bg(bg));
            }
        }
        let c = picker
            .query
            .render(filter_area, buf, Style::default().fg(theme.text).bg(bg));
        if picker.filtering {
            cursor = c;
        }
    } else {
        buf.set_stringn(
            filter_area.x,
            filter_area.y,
            "fuzzy filter over project and prompt (/)",
            usize::from(filter_area.width),
            muted,
        );
    }
    buf.set_string(
        content.right().saturating_sub(toggles_w),
        content.y,
        &toggles,
        secondary,
    );

    // Column widths: age, project, prompt (rest), messages, branch.
    let list_w = usize::from(content.width);
    let age_w = 4;
    let msgs_w = 5;
    let branch_w = (list_w / 6).clamp(8, 18);
    let project_w = (list_w / 4).clamp(12, 30);
    let prompt_w = list_w
        .saturating_sub(age_w + msgs_w + branch_w + project_w + 4)
        .max(8);
    let header = format!(
        "{:<age_w$} {:<project_w$} {:<prompt_w$} {:>msgs_w$} {}",
        "AGE", "PROJECT", "PROMPT", "MSGS", "BRANCH"
    );
    buf.set_stringn(
        content.x,
        content.y + 1,
        &header,
        list_w,
        muted.add_modifier(Modifier::BOLD),
    );

    let rows_h = usize::from(content.height.saturating_sub(4));
    let status = if picker.loading {
        Some("Loading sessions\u{2026}".to_string())
    } else if let Some(error) = &picker.error {
        Some(format!("\u{2716} {error}"))
    } else if picker.visible_len() == 0 {
        Some(if picker.provider == as_core::ProviderId::Opencode {
            "OpenCode keeps no session files to list.".to_string()
        } else if picker.session_count() == 0 {
            format!("No {} sessions found.", picker.provider.label())
        } else {
            "No session matches the filters (a all ages, p all projects, s hidden).".to_string()
        })
    } else {
        None
    };
    if let Some(text) = status {
        buf.set_stringn(content.x, content.y + 2, &text, list_w, secondary);
    } else {
        let offset = (picker.cursor + 1).saturating_sub(rows_h.max(1));
        for (row, (i, s)) in picker
            .visible()
            .enumerate()
            .skip(offset)
            .take(rows_h)
            .enumerate()
            .map(|(row, (i, s))| (row, (i + offset, s)))
        {
            let selected = i == picker.cursor;
            let y = content.y + 2 + u16::try_from(row).unwrap_or(0);
            let style = if selected {
                Style::default()
                    .fg(theme.text)
                    .bg(theme.surface_hover)
                    .add_modifier(Modifier::BOLD)
            } else if s.hidden {
                muted
            } else {
                Style::default().fg(theme.text)
            };
            for x in content.x..content.right() {
                if let Some(c) = buf.cell_mut((x, y)) {
                    c.set_symbol(" ");
                    c.set_style(style);
                }
            }
            let modified = chrono::DateTime::parse_from_rfc3339(&s.modified_at)
                .map_or(0, |d| d.timestamp_millis());
            let project = fit_left(
                &as_core::paths::tildify(std::path::Path::new(&s.project_path)),
                project_w,
            );
            let prompt: String = one_line(&s.first_prompt).chars().take(prompt_w).collect();
            let prompt = if one_line(&s.first_prompt).chars().count() > prompt_w {
                format!(
                    "{}\u{2026}",
                    prompt
                        .chars()
                        .take(prompt_w.saturating_sub(1))
                        .collect::<String>()
                )
            } else {
                prompt
            };
            let msgs = if s.message_count == 0 {
                String::new()
            } else {
                s.message_count.to_string()
            };
            let branch = s.git_branch.clone().unwrap_or_default();
            let branch: String = branch.chars().take(branch_w).collect();
            let hidden = if s.hidden { "\u{2298} " } else { "" };
            let line = format!(
                "{:<age_w$} {project:<project_w$} {:<prompt_w$} {msgs:>msgs_w$} {hidden}{branch}",
                format_age(modified, picker.now_ms),
                prompt,
            );
            buf.set_stringn(content.x, y, &line, list_w, style);
        }
    }

    // Footer line.
    let count = format!(
        "{} of {} sessions",
        picker.visible_len(),
        picker.session_count()
    );
    let hint = if picker.filtering {
        "type to filter \u{00B7} Enter keep \u{00B7} Esc clear".to_string()
    } else {
        let tab = if picker.origin == crate::sessions::PickerOrigin::Standalone {
            " \u{00B7} Tab provider"
        } else {
            ""
        };
        format!(
            "Enter resume \u{00B7} / filter \u{00B7} a/p/s toggles \u{00B7} h hide \u{00B7} r reload{tab} \u{00B7} Esc"
        )
    };
    let y = content.bottom().saturating_sub(1);
    buf.set_stringn(content.x, y, &hint, list_w, muted);
    let count_w = u16::try_from(count.chars().count()).unwrap_or(0);
    if usize::from(count_w) + hint.chars().count() + 2 < list_w {
        buf.set_string(
            content.right().saturating_sub(count_w),
            y,
            &count,
            secondary,
        );
    }
    cursor
}

fn profile_chooser(theme: &Theme, chooser: &ProfileChooser, screen: Rect, buf: &mut Buffer) {
    let h = u16::try_from(chooser.choices.len() + 6)
        .unwrap_or(14)
        .min(screen.height);
    let area = centered(screen, 74, h);
    let inner = frame(theme, "Claude profile", area, buf);
    let content = Rect {
        x: inner.x + 1,
        width: inner.width.saturating_sub(2),
        ..inner
    };
    let project = as_core::paths::tildify(std::path::Path::new(&chooser.project_dir));
    buf.set_stringn(
        content.x,
        content.y,
        format!("Project: {project} (and its subfolders)"),
        usize::from(content.width),
        Style::default().fg(theme.text_secondary),
    );
    for (i, choice) in chooser.choices.iter().enumerate() {
        let y = content.y + 2 + u16::try_from(i).unwrap_or(0);
        let selected = i == chooser.idx;
        let style = if selected {
            Style::default()
                .fg(theme.accent_text)
                .bg(theme.accent)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.text)
        };
        buf.set_stringn(
            content.x,
            y,
            format!(" {} ", choice.label),
            usize::from(content.width),
            style,
        );
    }
    buf.set_stringn(
        content.x,
        content.bottom().saturating_sub(1),
        "Enter apply \u{00B7} Esc cancel \u{00B7} new agents of the project use this profile",
        usize::from(content.width),
        Style::default().fg(theme.text_muted),
    );
}
