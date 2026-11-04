//! Modal dialogs: new agent, rename, remove confirmation, quit, help.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph, Widget};

use crate::app::{App, Modal, QuitChoice};
use crate::keymap::Command;
use crate::new_agent::{Field, NewAgentForm};
use crate::theme::Theme;

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
    let cursor = match &mut modal {
        Modal::NewAgent(form) => new_agent(&theme, form, canvas_screen, &mut canvas),
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
            help(app, canvas_screen, &mut canvas);
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
    let h = 11 + prompt_h + u16::from(form.provider == as_core::ProviderId::Codex);
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

    // Session (P4 placeholder, disabled)
    let spans = vec![
        Span::styled(
            format!("{:<10}", "Resume"),
            Style::default().fg(theme.text_muted),
        ),
        Span::styled(
            "[ ] resume a previous session (available in a later release)",
            Style::default()
                .fg(theme.text_muted)
                .add_modifier(Modifier::DIM),
        ),
    ];
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

fn help(app: &App, screen: Rect, buf: &mut Buffer) {
    let theme = &app.theme;
    let km = &app.keymap;
    let entries: Vec<(String, String)> = [
        Command::SendPrefix,
        Command::NewAgent,
        Command::KillAgent,
        Command::RestartAgent,
        Command::RemoveAgent,
        Command::RenameAgent,
        Command::ToggleYolo,
        Command::ToggleGlobalYolo,
        Command::NextAgent,
        Command::PrevAgent,
        Command::JumpAgent(1),
        Command::FocusSidebar,
        Command::ScrollMode,
        Command::ToggleInputBar,
        Command::SidebarNarrower,
        Command::SidebarWider,
        Command::ClearToasts,
        Command::Help,
        Command::Quit,
    ]
    .into_iter()
    .map(|cmd| {
        let mut keys: Vec<String> = km
            .prefix_keys_for(cmd)
            .iter()
            .map(ToString::to_string)
            .collect();
        if let Command::JumpAgent(1) = cmd {
            keys = vec!["1-9".into()];
        }
        (keys.join(" / "), cmd.description())
    })
    .collect();
    let h = u16::try_from(entries.len() + 8).unwrap_or(30);
    let area = centered(screen, 74, h);
    let inner = frame(theme, "Key bindings", area, buf);
    let prefix = app.prefix_label();
    let mut lines = vec![
        Line::from(vec![
            Span::styled("Prefix: ", Style::default().fg(theme.text_secondary)),
            Span::styled(
                prefix.clone(),
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "  then a key (all other keys go to the agent):",
                Style::default().fg(theme.text_secondary),
            ),
        ]),
        Line::raw(""),
    ];
    for (keys, desc) in entries {
        lines.push(Line::from(vec![
            Span::styled(
                format!(" {prefix} {keys:<14}"),
                Style::default().fg(theme.accent),
            ),
            Span::raw(desc),
        ]));
    }
    lines.push(Line::raw(""));
    lines.push(Line::styled(
        " Sidebar focus: j/k move \u{00B7} Enter attach \u{00B7} Space fold \u{00B7} / filter \u{00B7} Esc back",
        Style::default().fg(theme.text_secondary),
    ));
    lines.push(Line::styled(
        " Scroll mode: \u{2191}/\u{2193} PgUp/PgDn g/G \u{00B7} Esc/q leave (Shift+PgUp enters)",
        Style::default().fg(theme.text_secondary),
    ));
    Paragraph::new(lines).render(
        Rect {
            x: inner.x + 1,
            width: inner.width.saturating_sub(2),
            ..inner
        },
        buf,
    );
}
