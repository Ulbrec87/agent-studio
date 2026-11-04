//! Key chords, commands and the data-driven binding tables.
//!
//! Three tables, all plain `Vec<(KeyChord, Command)>` so P5 can load user overrides:
//! * `prefix` — keys after the prefix key (tmux style), valid in every focus;
//! * `app` — keys while the sidebar/app has focus (no prefix needed);
//! * `scroll` — keys in scroll mode.

use std::fmt;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

/// A key plus modifiers, normalized so that `R` and `shift+r` compare equal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct KeyChord {
    pub code: KeyCode,
    pub mods: KeyModifiers,
}

impl KeyChord {
    pub fn new(code: KeyCode, mods: KeyModifiers) -> Self {
        let mut mods = mods & (KeyModifiers::SHIFT | KeyModifiers::ALT | KeyModifiers::CONTROL);
        let code = match code {
            KeyCode::Char(c) => {
                // The case of the character already carries shift.
                if c.is_alphabetic() || !mods.contains(KeyModifiers::CONTROL) {
                    mods.remove(KeyModifiers::SHIFT);
                }
                if mods.contains(KeyModifiers::CONTROL) {
                    KeyCode::Char(c.to_ascii_lowercase())
                } else {
                    KeyCode::Char(c)
                }
            }
            KeyCode::BackTab => {
                mods.remove(KeyModifiers::SHIFT);
                KeyCode::BackTab
            }
            other => other,
        };
        Self { code, mods }
    }

    pub fn plain(c: char) -> Self {
        Self::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    pub fn ctrl(c: char) -> Self {
        Self::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    pub fn key(code: KeyCode) -> Self {
        Self::new(code, KeyModifiers::NONE)
    }

    pub fn from_event(key: &KeyEvent) -> Self {
        Self::new(key.code, key.modifiers)
    }

    /// Parse `ctrl+a`, `alt+x`, `shift+tab`, `R`, `esc`, `f5` ...
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        if text.is_empty() {
            return None;
        }
        let mut mods = KeyModifiers::NONE;
        let mut rest = text;
        loop {
            let lower = rest.to_ascii_lowercase();
            let (m, len) = if lower.starts_with("ctrl+") {
                (KeyModifiers::CONTROL, 5)
            } else if lower.starts_with("control+") {
                (KeyModifiers::CONTROL, 8)
            } else if lower.starts_with("alt+") {
                (KeyModifiers::ALT, 4)
            } else if lower.starts_with("shift+") {
                (KeyModifiers::SHIFT, 6)
            } else {
                break;
            };
            mods |= m;
            rest = &rest[len..];
        }
        let lower = rest.to_ascii_lowercase();
        let code = match lower.as_str() {
            "esc" | "escape" => KeyCode::Esc,
            "enter" | "return" => KeyCode::Enter,
            "tab" => KeyCode::Tab,
            "backtab" => KeyCode::BackTab,
            "space" => KeyCode::Char(' '),
            "backspace" => KeyCode::Backspace,
            "delete" | "del" => KeyCode::Delete,
            "insert" => KeyCode::Insert,
            "up" => KeyCode::Up,
            "down" => KeyCode::Down,
            "left" => KeyCode::Left,
            "right" => KeyCode::Right,
            "home" => KeyCode::Home,
            "end" => KeyCode::End,
            "pageup" | "pgup" => KeyCode::PageUp,
            "pagedown" | "pgdn" => KeyCode::PageDown,
            f if f.starts_with('f')
                && f.len() > 1
                && f[1..].chars().all(|c| c.is_ascii_digit()) =>
            {
                KeyCode::F(f[1..].parse().ok()?)
            }
            _ => {
                let mut chars = rest.chars();
                let c = chars.next()?;
                if chars.next().is_some() {
                    return None;
                }
                KeyCode::Char(c)
            }
        };
        // `shift+tab` is BackTab.
        if code == KeyCode::Tab && mods.contains(KeyModifiers::SHIFT) {
            return Some(Self::new(KeyCode::BackTab, mods));
        }
        Some(Self::new(code, mods))
    }
}

impl fmt::Display for KeyChord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.mods.contains(KeyModifiers::CONTROL) {
            f.write_str("Ctrl+")?;
        }
        if self.mods.contains(KeyModifiers::ALT) {
            f.write_str("Alt+")?;
        }
        if self.mods.contains(KeyModifiers::SHIFT) {
            f.write_str("Shift+")?;
        }
        match self.code {
            KeyCode::Char(' ') => f.write_str("Space"),
            KeyCode::Char(c) if self.mods.contains(KeyModifiers::CONTROL) => {
                write!(f, "{}", c.to_ascii_uppercase())
            }
            KeyCode::Char(c) => write!(f, "{c}"),
            KeyCode::Esc => f.write_str("Esc"),
            KeyCode::Enter => f.write_str("Enter"),
            KeyCode::Tab => f.write_str("Tab"),
            KeyCode::BackTab => f.write_str("Shift+Tab"),
            KeyCode::Backspace => f.write_str("Backspace"),
            KeyCode::Delete => f.write_str("Del"),
            KeyCode::Insert => f.write_str("Ins"),
            KeyCode::Up => f.write_str("Up"),
            KeyCode::Down => f.write_str("Down"),
            KeyCode::Left => f.write_str("Left"),
            KeyCode::Right => f.write_str("Right"),
            KeyCode::Home => f.write_str("Home"),
            KeyCode::End => f.write_str("End"),
            KeyCode::PageUp => f.write_str("PgUp"),
            KeyCode::PageDown => f.write_str("PgDn"),
            KeyCode::F(n) => write!(f, "F{n}"),
            other => write!(f, "{other:?}"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Command {
    /// Send the prefix key itself to the agent.
    SendPrefix,
    NewAgent,
    /// Pick a previous session of the provider and resume it.
    ResumeSession,
    /// Choose the Claude profile of the project under the cursor.
    SetProfile,
    KillAgent,
    RestartAgent,
    RemoveAgent,
    RenameAgent,
    ToggleYolo,
    ToggleGlobalYolo,
    NextAgent,
    PrevAgent,
    /// App focus: move the sidebar cursor over every row.
    CursorDown,
    CursorUp,
    /// 1-based position in the sidebar.
    JumpAgent(u8),
    FocusSidebar,
    FocusTerminal,
    ScrollMode,
    ToggleInputBar,
    Help,
    Quit,
    SidebarNarrower,
    SidebarWider,
    ClearToasts,
    /// App focus: attach the agent under the cursor / toggle the project.
    Activate,
    ToggleCollapse,
    FilterSidebar,
    // Scroll mode.
    ScrollUp,
    ScrollDown,
    ScrollPageUp,
    ScrollPageDown,
    ScrollTop,
    ScrollBottom,
    ExitScroll,
}

impl Command {
    pub fn description(self) -> String {
        match self {
            Command::SendPrefix => "send the prefix key to the agent".into(),
            Command::NewAgent => "new agent".into(),
            Command::ResumeSession => "resume session\u{2026}".into(),
            Command::SetProfile => "Claude profile of the project".into(),
            Command::KillAgent => "kill agent (stop its process)".into(),
            Command::RestartAgent => "restart agent".into(),
            Command::RemoveAgent => "remove agent".into(),
            Command::RenameAgent => "rename agent".into(),
            Command::ToggleYolo => "toggle YOLO for this agent".into(),
            Command::ToggleGlobalYolo => "toggle global YOLO".into(),
            Command::NextAgent => "next agent".into(),
            Command::PrevAgent => "previous agent".into(),
            Command::CursorDown => "cursor down".into(),
            Command::CursorUp => "cursor up".into(),
            Command::JumpAgent(n) => format!("jump to agent {n}"),
            Command::FocusSidebar => "focus the sidebar".into(),
            Command::FocusTerminal => "focus the terminal".into(),
            Command::ScrollMode => "scroll mode (scrollback)".into(),
            Command::ToggleInputBar => "toggle the input bar".into(),
            Command::Help => "this help".into(),
            Command::Quit => "quit".into(),
            Command::SidebarNarrower => "narrower sidebar".into(),
            Command::SidebarWider => "wider sidebar".into(),
            Command::ClearToasts => "dismiss notifications".into(),
            Command::Activate => "attach the agent / toggle the project".into(),
            Command::ToggleCollapse => "collapse / expand the project".into(),
            Command::FilterSidebar => "filter the sidebar".into(),
            Command::ScrollUp => "scroll up a line".into(),
            Command::ScrollDown => "scroll down a line".into(),
            Command::ScrollPageUp => "scroll up a page".into(),
            Command::ScrollPageDown => "scroll down a page".into(),
            Command::ScrollTop => "oldest output".into(),
            Command::ScrollBottom => "latest output".into(),
            Command::ExitScroll => "leave scroll mode".into(),
        }
    }
}

pub type Bindings = Vec<(KeyChord, Command)>;

#[derive(Debug, Clone)]
pub struct Keymap {
    pub prefix: KeyChord,
    /// Keys after the prefix.
    pub prefix_bindings: Bindings,
    /// Keys in App (sidebar) focus.
    pub app_bindings: Bindings,
    /// Keys in scroll mode.
    pub scroll_bindings: Bindings,
}

fn lookup(table: &Bindings, chord: KeyChord) -> Option<Command> {
    table.iter().find(|(c, _)| *c == chord).map(|(_, cmd)| *cmd)
}

impl Keymap {
    /// Defaults for a given prefix chord.
    pub fn with_prefix(prefix: KeyChord) -> Self {
        let mut prefix_bindings: Bindings = vec![
            (prefix, Command::SendPrefix),
            (KeyChord::plain('n'), Command::NewAgent),
            (KeyChord::plain('o'), Command::ResumeSession),
            (KeyChord::plain('P'), Command::SetProfile),
            (KeyChord::plain('x'), Command::KillAgent),
            (KeyChord::plain('R'), Command::RestartAgent),
            (KeyChord::plain('d'), Command::RemoveAgent),
            (KeyChord::key(KeyCode::Delete), Command::RemoveAgent),
            (KeyChord::plain('r'), Command::RenameAgent),
            (KeyChord::plain('y'), Command::ToggleYolo),
            (KeyChord::plain('Y'), Command::ToggleGlobalYolo),
            (KeyChord::plain('j'), Command::NextAgent),
            (KeyChord::key(KeyCode::Down), Command::NextAgent),
            (KeyChord::plain('k'), Command::PrevAgent),
            (KeyChord::key(KeyCode::Up), Command::PrevAgent),
            (KeyChord::plain('s'), Command::FocusSidebar),
            (KeyChord::plain('['), Command::ScrollMode),
            (KeyChord::plain('i'), Command::ToggleInputBar),
            (KeyChord::plain('?'), Command::Help),
            (KeyChord::plain('q'), Command::Quit),
            (KeyChord::plain('<'), Command::SidebarNarrower),
            (KeyChord::plain('>'), Command::SidebarWider),
            (KeyChord::key(KeyCode::Esc), Command::ClearToasts),
        ];
        if prefix == KeyChord::ctrl('a') {
            prefix_bindings.push((KeyChord::plain('a'), Command::SendPrefix));
        }
        for n in 1..=9u8 {
            prefix_bindings.push((KeyChord::plain(char::from(b'0' + n)), Command::JumpAgent(n)));
        }
        let app_bindings: Bindings = vec![
            (KeyChord::key(KeyCode::Down), Command::CursorDown),
            (KeyChord::plain('j'), Command::CursorDown),
            (KeyChord::key(KeyCode::Up), Command::CursorUp),
            (KeyChord::plain('k'), Command::CursorUp),
            (KeyChord::key(KeyCode::Enter), Command::Activate),
            (KeyChord::key(KeyCode::Right), Command::Activate),
            (KeyChord::plain('l'), Command::Activate),
            (KeyChord::plain(' '), Command::ToggleCollapse),
            (KeyChord::key(KeyCode::Left), Command::ToggleCollapse),
            (KeyChord::plain('h'), Command::ToggleCollapse),
            (KeyChord::plain('/'), Command::FilterSidebar),
            (KeyChord::key(KeyCode::Esc), Command::FocusTerminal),
            (KeyChord::key(KeyCode::Tab), Command::FocusTerminal),
            (KeyChord::plain('n'), Command::NewAgent),
            (KeyChord::plain('o'), Command::ResumeSession),
            (KeyChord::plain('P'), Command::SetProfile),
            (KeyChord::plain('x'), Command::KillAgent),
            (KeyChord::plain('R'), Command::RestartAgent),
            (KeyChord::plain('d'), Command::RemoveAgent),
            (KeyChord::key(KeyCode::Delete), Command::RemoveAgent),
            (KeyChord::plain('r'), Command::RenameAgent),
            (KeyChord::plain('y'), Command::ToggleYolo),
            (KeyChord::plain('Y'), Command::ToggleGlobalYolo),
            (KeyChord::plain('['), Command::ScrollMode),
            (KeyChord::plain('i'), Command::ToggleInputBar),
            (KeyChord::plain('?'), Command::Help),
            (KeyChord::plain('q'), Command::Quit),
            (KeyChord::plain('<'), Command::SidebarNarrower),
            (KeyChord::plain('>'), Command::SidebarWider),
        ];
        let scroll_bindings: Bindings = vec![
            (KeyChord::key(KeyCode::Up), Command::ScrollUp),
            (KeyChord::plain('k'), Command::ScrollUp),
            (KeyChord::key(KeyCode::Down), Command::ScrollDown),
            (KeyChord::plain('j'), Command::ScrollDown),
            (KeyChord::key(KeyCode::PageUp), Command::ScrollPageUp),
            (KeyChord::ctrl('b'), Command::ScrollPageUp),
            (KeyChord::key(KeyCode::PageDown), Command::ScrollPageDown),
            (KeyChord::ctrl('f'), Command::ScrollPageDown),
            (KeyChord::plain('g'), Command::ScrollTop),
            (KeyChord::key(KeyCode::Home), Command::ScrollTop),
            (KeyChord::plain('G'), Command::ScrollBottom),
            (KeyChord::key(KeyCode::End), Command::ScrollBottom),
            (KeyChord::key(KeyCode::Esc), Command::ExitScroll),
            (KeyChord::plain('q'), Command::ExitScroll),
        ];
        Self {
            prefix,
            prefix_bindings,
            app_bindings,
            scroll_bindings,
        }
    }

    /// Build from the config's `prefixKey` (falls back to Ctrl+A on garbage).
    pub fn from_prefix_key(prefix_key: &str) -> Self {
        let prefix = KeyChord::parse(prefix_key).unwrap_or_else(|| KeyChord::ctrl('a'));
        Self::with_prefix(prefix)
    }

    pub fn is_prefix(&self, key: &KeyEvent) -> bool {
        key.kind != KeyEventKind::Release && KeyChord::from_event(key) == self.prefix
    }

    pub fn after_prefix(&self, key: &KeyEvent) -> Option<Command> {
        lookup(&self.prefix_bindings, KeyChord::from_event(key))
    }

    pub fn in_app_focus(&self, key: &KeyEvent) -> Option<Command> {
        lookup(&self.app_bindings, KeyChord::from_event(key))
    }

    pub fn in_scroll_mode(&self, key: &KeyEvent) -> Option<Command> {
        lookup(&self.scroll_bindings, KeyChord::from_event(key))
    }

    /// Chords bound to `command` in the prefix table, for the help overlay.
    pub fn prefix_keys_for(&self, command: Command) -> Vec<KeyChord> {
        self.prefix_bindings
            .iter()
            .filter(|(_, c)| *c == command)
            .map(|(k, _)| *k)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, mods)
    }

    #[test]
    fn parses_chords() {
        assert_eq!(KeyChord::parse("ctrl+a"), Some(KeyChord::ctrl('a')));
        assert_eq!(KeyChord::parse("Ctrl+B"), Some(KeyChord::ctrl('b')));
        assert_eq!(KeyChord::parse("esc"), Some(KeyChord::key(KeyCode::Esc)));
        assert_eq!(KeyChord::parse("R"), Some(KeyChord::plain('R')));
        assert_eq!(
            KeyChord::parse("shift+tab"),
            Some(KeyChord::key(KeyCode::BackTab))
        );
        assert_eq!(
            KeyChord::parse("alt+x"),
            Some(KeyChord::new(KeyCode::Char('x'), KeyModifiers::ALT))
        );
        assert_eq!(KeyChord::parse("f5"), Some(KeyChord::key(KeyCode::F(5))));
        assert_eq!(KeyChord::parse("ctrl+"), None);
        assert_eq!(KeyChord::parse(""), None);
        assert_eq!(KeyChord::parse("ab"), None);
    }

    #[test]
    fn shift_on_a_char_is_normalized_away() {
        let from_terminal = ev(KeyCode::Char('R'), KeyModifiers::SHIFT);
        assert_eq!(KeyChord::from_event(&from_terminal), KeyChord::plain('R'));
        let km = Keymap::with_prefix(KeyChord::ctrl('a'));
        assert_eq!(km.after_prefix(&from_terminal), Some(Command::RestartAgent));
    }

    #[test]
    fn default_prefix_table() {
        let km = Keymap::from_prefix_key("ctrl+a");
        assert!(km.is_prefix(&ev(KeyCode::Char('a'), KeyModifiers::CONTROL)));
        assert!(!km.is_prefix(&ev(KeyCode::Char('a'), KeyModifiers::NONE)));
        let get = |c| km.after_prefix(&ev(KeyCode::Char(c), KeyModifiers::NONE));
        assert_eq!(get('a'), Some(Command::SendPrefix));
        assert_eq!(get('n'), Some(Command::NewAgent));
        assert_eq!(get('5'), Some(Command::JumpAgent(5)));
        assert_eq!(get('Y'), Some(Command::ToggleGlobalYolo));
        assert_eq!(get('z'), None);
        assert_eq!(
            km.after_prefix(&ev(KeyCode::Char('a'), KeyModifiers::CONTROL)),
            Some(Command::SendPrefix)
        );
        assert_eq!(
            km.after_prefix(&ev(KeyCode::Delete, KeyModifiers::NONE)),
            Some(Command::RemoveAgent)
        );
    }

    #[test]
    fn custom_prefix_sends_itself_and_drops_the_a_alias() {
        let km = Keymap::from_prefix_key("ctrl+b");
        assert_eq!(
            km.after_prefix(&ev(KeyCode::Char('b'), KeyModifiers::CONTROL)),
            Some(Command::SendPrefix)
        );
        assert_eq!(
            km.after_prefix(&ev(KeyCode::Char('a'), KeyModifiers::NONE)),
            None
        );
        // Garbage falls back to the default.
        assert_eq!(Keymap::from_prefix_key("???").prefix, KeyChord::ctrl('a'));
    }

    #[test]
    fn chords_display() {
        assert_eq!(KeyChord::ctrl('a').to_string(), "Ctrl+A");
        assert_eq!(KeyChord::plain('n').to_string(), "n");
        assert_eq!(KeyChord::key(KeyCode::PageUp).to_string(), "PgUp");
    }
}
