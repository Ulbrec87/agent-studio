//! State of the P9 panels (git, files, free terminals, settings, open-in). Each one is a plain
//! struct with `key`/`on_reply` methods that return [`Effect`]s, so it is testable without a
//! terminal or a daemon; `app::panels` wires them into the [`App`](crate::app::App) and `ui::*`
//! draws them.

pub mod diff_view;
pub mod files;
pub mod git;
pub mod open_in;
pub mod settings;
pub mod terminals;

use as_proto::{Request, Response};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::Effect;

/// What a panel's key handler wants besides effects.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Outcome {
    #[default]
    Stay,
    Close,
}

/// A daemon request whose answer comes back as `Msg::Reply { key, .. }`.
pub fn req(key: impl Into<String>, request: Request) -> Effect {
    Effect::Req {
        key: key.into(),
        req: request,
    }
}

pub type Reply = Result<Response, String>;

/// `delta` rows down (negative: up), clamped to `0..len`.
pub fn step(cursor: usize, len: usize, delta: isize) -> usize {
    if len == 0 {
        return 0;
    }
    cursor
        .saturating_add_signed(delta)
        .min(len.saturating_sub(1))
}

pub fn is_char(key: &KeyEvent, c: char) -> bool {
    key.code == KeyCode::Char(c)
        && !key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
}

/// Vim-ish vertical movement shared by the lists: `(delta, to_edge)` of a key.
pub fn nav(key: &KeyEvent, page: usize) -> Option<isize> {
    let page = isize::try_from(page.max(1)).unwrap_or(10);
    match key.code {
        KeyCode::Down | KeyCode::Char('j') => Some(1),
        KeyCode::Up | KeyCode::Char('k') => Some(-1),
        KeyCode::PageDown => Some(page),
        KeyCode::PageUp => Some(-page),
        KeyCode::Char('g') | KeyCode::Home => Some(isize::MIN / 2),
        KeyCode::Char('G') | KeyCode::End => Some(isize::MAX / 2),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn step_clamps() {
        assert_eq!(step(0, 0, 5), 0);
        assert_eq!(step(2, 5, 1), 3);
        assert_eq!(step(2, 5, -9), 0);
        assert_eq!(step(2, 5, isize::MAX / 2), 4);
    }
}
