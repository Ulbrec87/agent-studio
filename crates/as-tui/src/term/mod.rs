//! Terminal emulation of agent output and keyboard encoding.
//!
//! [`TerminalEmulator`] is the seam between the app and the emulation library: today
//! [`vt::Vt100Emulator`] (the `vt100` crate), later possibly `alacritty_terminal`.

pub mod keys;
pub mod render;
pub mod vt;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

pub use vt::Vt100Emulator;

/// Scrollback lines kept per agent.
pub const SCROLLBACK_LINES: usize = 5000;

/// Colors the emulator needs for cells that use the terminal's default fg/bg.
#[derive(Debug, Clone, Copy)]
pub struct TermPalette {
    pub fg: ratatui::style::Color,
    pub bg: ratatui::style::Color,
}

pub trait TerminalEmulator: Send {
    /// Feed PTY output.
    fn process(&mut self, bytes: &[u8]);
    fn resize(&mut self, cols: u16, rows: u16);
    /// `(cols, rows)`.
    fn size(&self) -> (u16, u16);
    /// Scroll the view `rows` lines into the history (0 = live screen); clamped.
    fn set_scrollback(&mut self, rows: usize);
    fn scrollback(&self) -> usize;
    fn scrollback_len(&self) -> usize;
    /// DECCKM: arrows are sent as `ESC O x`.
    fn application_cursor(&self) -> bool;
    /// DECSET 2004: the app wants bracketed paste.
    fn bracketed_paste(&self) -> bool;
    /// Visible cursor `(col, row)` in the live view, `None` when hidden or scrolled back.
    fn cursor(&self) -> Option<(u16, u16)>;
    /// Bytes the emulator wants to answer to the application (cursor position report, DA ...).
    fn take_replies(&mut self) -> Vec<u8>;
    /// Whether the app rang the bell since the last call.
    fn take_bell(&mut self) -> bool;
    fn render(&self, area: Rect, buf: &mut Buffer, palette: TermPalette);
    /// Like [`render`](Self::render) for the part of the screen starting at `(row_off, col_off)`.
    fn render_region(
        &self,
        area: Rect,
        buf: &mut Buffer,
        palette: TermPalette,
        row_off: u16,
        col_off: u16,
    );
    /// Rows of the live screen that matter: up to the last non-blank row or the cursor row,
    /// whichever is lower (at least 1).
    fn content_rows(&self) -> u16;
    /// Cursor `(col, row)` even when the application hides it.
    fn cursor_raw(&self) -> (u16, u16);
}
