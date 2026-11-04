//! Rendering. `draw` paints the whole screen from the app state.

pub mod chat;
pub mod dialogs;
pub mod footer;
pub mod header;
pub mod layout;
pub mod sidebar;
pub mod toasts;

use ratatui::Frame;
use ratatui::style::Style;
use ratatui::widgets::Block;

use crate::app::App;

pub fn draw(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    let t = app.theme;
    frame.render_widget(
        Block::default().style(Style::default().bg(t.bg).fg(t.text)),
        area,
    );
    let l = app.layout();
    let mut cursor = None;
    {
        let buf = frame.buffer_mut();
        header::render(app, l.header, buf);
        if let Some(side) = l.sidebar {
            sidebar::render(app, side, buf);
        }
        chat::render_title(app, l.title, buf);
        cursor = cursor.or(chat::render_terminal(app, l.terminal, buf));
        if let Some(input) = l.input {
            cursor = chat::render_input(app, input, buf).or(cursor);
        }
        footer::render(app, l.footer, buf);
        toasts::render(app, area, buf);
        if let Some(c) = dialogs::render(app, area, buf) {
            cursor = Some(c);
        } else if app.modal.is_some() {
            cursor = None;
        }
    }
    if let Some((x, y)) = cursor {
        frame.set_cursor_position((x, y));
    }
}

#[cfg(test)]
mod tests;
