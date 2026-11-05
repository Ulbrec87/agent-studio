//! Key chords, commands and the data-driven binding tables.
//!
//! Tables, all plain `Vec<(KeyChord, Command)>`, built from the command catalog
//! (`as_core::keybindings`: defaults + the user's `keybindings.json`):
//! * `prefix` — keys after the prefix key (tmux style), valid in every focus;
//! * `app` — keys while the sidebar has focus (no prefix needed);
//! * `grid` — keys while the grid has focus (tile navigation);
//! * `scroll` — keys in scroll mode;
//! * `direct` — chords that run a command in every focus (e.g. `alt+1`), user-defined only.

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
    /// New agent dialog with the manager toggle on (P8).
    NewManagerAgent,
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
    /// 1-based position in the sidebar (`JumpAgent(0)` is the catalog placeholder for the range).
    JumpAgent(u8),
    FocusSidebar,
    FocusTerminal,
    ScrollMode,
    ToggleInputBar,
    Help,
    Quit,
    SidebarNarrower,
    SidebarWider,
    /// Dismiss toasts; in the grid with nothing to dismiss, return to tile navigation.
    ClearToasts,
    /// App focus: attach the agent under the cursor / toggle the project.
    Activate,
    ToggleCollapse,
    /// Focus the sidebar search field.
    FilterSidebar,
    CycleSidebarStatus,
    CycleSidebarSort,
    // Views.
    ToggleView,
    ShowOverview,
    CycleView,
    CommandPalette,
    OpenKeybindingsFile,
    // Usage (P7).
    /// The usage report overlay.
    UsageReport,
    RefreshUsage,
    ToggleUsageCosts,
    ToggleUsageAlerts,
    // Headless runs and the MCP server (P8).
    Headless,
    McpStatus,
    // P9 panels.
    GitPanel,
    ToggleFiles,
    FileSearch,
    Settings,
    OpenIn,
    ExportDiagnostics,
    ImportHydra,
    /// Ask GitHub Releases for a newer version (P10).
    CheckUpdates,
    TerminalToggle,
    TerminalNew,
    TerminalSplit,
    TerminalSplitDown,
    TerminalFocusLeft,
    TerminalFocusRight,
    TerminalFocusUp,
    TerminalFocusDown,
    TerminalResizeLeft,
    TerminalResizeRight,
    TerminalResizeUp,
    TerminalResizeDown,
    TerminalKill,
    TerminalNextTab,
    TerminalPrevTab,
    TerminalNextPane,
    TerminalRename,
    TerminalFullscreen,
    // Grid.
    ToggleTileExpand,
    Broadcast,
    GridLeft,
    GridRight,
    GridUp,
    GridDown,
    GridAttach,
    GridOpenChat,
    GridMovePrev,
    GridMoveNext,
    GridPrevTab,
    GridNextTab,
    GridCycleColumns,
    GridOnlyRunning,
    // Scroll mode.
    ScrollUp,
    ScrollDown,
    ScrollPageUp,
    ScrollPageDown,
    ScrollTop,
    ScrollBottom,
    ExitScroll,
}

/// Every command once (`JumpAgent(0)` stands for the whole range).
pub const ALL_COMMANDS: &[Command] = &[
    Command::SendPrefix,
    Command::NewAgent,
    Command::NewManagerAgent,
    Command::ResumeSession,
    Command::SetProfile,
    Command::KillAgent,
    Command::RestartAgent,
    Command::RemoveAgent,
    Command::RenameAgent,
    Command::ToggleYolo,
    Command::ToggleGlobalYolo,
    Command::NextAgent,
    Command::PrevAgent,
    Command::CursorDown,
    Command::CursorUp,
    Command::JumpAgent(0),
    Command::FocusSidebar,
    Command::FocusTerminal,
    Command::ScrollMode,
    Command::ToggleInputBar,
    Command::Help,
    Command::Quit,
    Command::SidebarNarrower,
    Command::SidebarWider,
    Command::ClearToasts,
    Command::Activate,
    Command::ToggleCollapse,
    Command::FilterSidebar,
    Command::CycleSidebarStatus,
    Command::CycleSidebarSort,
    Command::ToggleView,
    Command::ShowOverview,
    Command::CycleView,
    Command::CommandPalette,
    Command::OpenKeybindingsFile,
    Command::UsageReport,
    Command::RefreshUsage,
    Command::ToggleUsageCosts,
    Command::ToggleUsageAlerts,
    Command::Headless,
    Command::McpStatus,
    Command::GitPanel,
    Command::ToggleFiles,
    Command::FileSearch,
    Command::Settings,
    Command::OpenIn,
    Command::ExportDiagnostics,
    Command::ImportHydra,
    Command::CheckUpdates,
    Command::TerminalToggle,
    Command::TerminalNew,
    Command::TerminalSplit,
    Command::TerminalSplitDown,
    Command::TerminalFocusLeft,
    Command::TerminalFocusRight,
    Command::TerminalFocusUp,
    Command::TerminalFocusDown,
    Command::TerminalResizeLeft,
    Command::TerminalResizeRight,
    Command::TerminalResizeUp,
    Command::TerminalResizeDown,
    Command::TerminalKill,
    Command::TerminalNextTab,
    Command::TerminalPrevTab,
    Command::TerminalNextPane,
    Command::TerminalRename,
    Command::TerminalFullscreen,
    Command::ToggleTileExpand,
    Command::Broadcast,
    Command::GridLeft,
    Command::GridRight,
    Command::GridUp,
    Command::GridDown,
    Command::GridAttach,
    Command::GridOpenChat,
    Command::GridMovePrev,
    Command::GridMoveNext,
    Command::GridPrevTab,
    Command::GridNextTab,
    Command::GridCycleColumns,
    Command::GridOnlyRunning,
    Command::ScrollUp,
    Command::ScrollDown,
    Command::ScrollPageUp,
    Command::ScrollPageDown,
    Command::ScrollTop,
    Command::ScrollBottom,
    Command::ExitScroll,
];

impl Command {
    /// Id in the catalog (`as_core::keybindings`).
    pub fn id(self) -> &'static str {
        match self {
            Command::SendPrefix => "send-prefix",
            Command::NewAgent => "new-agent",
            Command::NewManagerAgent => "new-manager-agent",
            Command::ResumeSession => "resume-session",
            Command::SetProfile => "set-profile",
            Command::KillAgent => "close-agent",
            Command::RestartAgent => "restart-agent",
            Command::RemoveAgent => "remove-agent",
            Command::RenameAgent => "rename-agent",
            Command::ToggleYolo => "toggle-yolo",
            Command::ToggleGlobalYolo => "toggle-global-yolo",
            Command::NextAgent => "next-agent",
            Command::PrevAgent => "prev-agent",
            Command::CursorDown => "cursor-down",
            Command::CursorUp => "cursor-up",
            Command::JumpAgent(_) => "switch-agent-by-index",
            Command::FocusSidebar => "focus-sidebar",
            Command::FocusTerminal => "focus-terminal",
            Command::ScrollMode => "scroll-mode",
            Command::ToggleInputBar => "toggle-chat-input",
            Command::Help => "show-shortcuts",
            Command::Quit => "quit",
            Command::SidebarNarrower => "sidebar-narrower",
            Command::SidebarWider => "sidebar-wider",
            Command::ClearToasts => "close-dialogs",
            Command::Activate => "activate",
            Command::ToggleCollapse => "toggle-collapse",
            Command::FilterSidebar => "focus-sidebar-search",
            Command::CycleSidebarStatus => "cycle-sidebar-status",
            Command::CycleSidebarSort => "cycle-sidebar-sort",
            Command::ToggleView => "toggle-view",
            Command::ShowOverview => "show-overview",
            Command::CycleView => "cycle-view",
            Command::CommandPalette => "command-palette",
            Command::OpenKeybindingsFile => "open-keybindings-file",
            Command::UsageReport => "usage-dashboard",
            Command::RefreshUsage => "refresh-usage",
            Command::ToggleUsageCosts => "toggle-usage-costs",
            Command::ToggleUsageAlerts => "toggle-usage-alerts",
            Command::Headless => "headless",
            Command::McpStatus => "mcp-status",
            Command::GitPanel => "git-panel",
            Command::ToggleFiles => "toggle-editor",
            Command::FileSearch => "file-search",
            Command::Settings => "settings",
            Command::OpenIn => "open-in",
            Command::ExportDiagnostics => "export-diagnostics",
            Command::ImportHydra => "import-hydra",
            Command::CheckUpdates => "updates",
            Command::TerminalToggle => "toggle-terminal",
            Command::TerminalNew => "terminal-new",
            Command::TerminalSplit => "terminal-split",
            Command::TerminalSplitDown => "terminal-split-down",
            Command::TerminalFocusLeft => "terminal-focus-left",
            Command::TerminalFocusRight => "terminal-focus-right",
            Command::TerminalFocusUp => "terminal-focus-up",
            Command::TerminalFocusDown => "terminal-focus-down",
            Command::TerminalResizeLeft => "terminal-resize-left",
            Command::TerminalResizeRight => "terminal-resize-right",
            Command::TerminalResizeUp => "terminal-resize-up",
            Command::TerminalResizeDown => "terminal-resize-down",
            Command::TerminalKill => "terminal-kill",
            Command::TerminalNextTab => "terminal-next-tab",
            Command::TerminalPrevTab => "terminal-prev-tab",
            Command::TerminalNextPane => "terminal-next-pane",
            Command::TerminalRename => "terminal-rename",
            Command::TerminalFullscreen => "terminal-fullscreen",
            Command::ToggleTileExpand => "toggle-tile-expand",
            Command::Broadcast => "broadcast",
            Command::GridLeft => "grid-left",
            Command::GridRight => "grid-right",
            Command::GridUp => "grid-up",
            Command::GridDown => "grid-down",
            Command::GridAttach => "grid-attach",
            Command::GridOpenChat => "grid-open-chat",
            Command::GridMovePrev => "grid-move-prev",
            Command::GridMoveNext => "grid-move-next",
            Command::GridPrevTab => "grid-prev-tab",
            Command::GridNextTab => "grid-next-tab",
            Command::GridCycleColumns => "grid-cycle-columns",
            Command::GridOnlyRunning => "grid-only-running",
            Command::ScrollUp => "scroll-up",
            Command::ScrollDown => "scroll-down",
            Command::ScrollPageUp => "scroll-page-up",
            Command::ScrollPageDown => "scroll-page-down",
            Command::ScrollTop => "scroll-top",
            Command::ScrollBottom => "scroll-bottom",
            Command::ExitScroll => "scroll-exit",
        }
    }

    /// The command of a catalog id (`None` for commands of later phases).
    pub fn from_id(id: &str) -> Option<Command> {
        ALL_COMMANDS.iter().copied().find(|c| c.id() == id)
    }

    pub fn description(self) -> String {
        match self {
            Command::JumpAgent(n) if n > 0 => format!("jump to agent {n}"),
            other => as_core::keybindings::command_def(other.id())
                .map_or_else(|| other.id().to_string(), |d| d.label.to_string()),
        }
    }
}

pub type Bindings = Vec<(KeyChord, Command)>;

/// Where a binding applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Scope {
    Prefix,
    App,
    Grid,
    Scroll,
    /// A chord that works in every focus without the prefix.
    Direct,
}

impl Scope {
    fn parse(word: &str) -> Option<Scope> {
        match word {
            "prefix" => Some(Scope::Prefix),
            "app" => Some(Scope::App),
            "grid" => Some(Scope::Grid),
            "scroll" => Some(Scope::Scroll),
            _ => None,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Scope::Prefix | Scope::Direct => "",
            Scope::App => "sidebar ",
            Scope::Grid => "grid ",
            Scope::Scroll => "scroll ",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Keymap {
    pub prefix: KeyChord,
    /// Keys after the prefix.
    pub prefix_bindings: Bindings,
    /// Keys in App (sidebar) focus.
    pub app_bindings: Bindings,
    /// Keys in grid focus (tile navigation).
    pub grid_bindings: Bindings,
    /// Keys in scroll mode.
    pub scroll_bindings: Bindings,
    /// Chords that run in every focus.
    pub direct_bindings: Bindings,
    /// Display strings of the bindings per command id, for the help overlay and palette hints.
    labels: Vec<(&'static str, Vec<String>)>,
}

fn lookup(table: &Bindings, chord: KeyChord) -> Option<Command> {
    table.iter().find(|(c, _)| *c == chord).map(|(_, cmd)| *cmd)
}

/// One parsed alternative of a `keys` string.
struct Parsed {
    scope: Scope,
    chords: Vec<KeyChord>,
    label: String,
}

/// Parse `prefix g`, `app j`, `ctrl+g`, `alt+1-9`, `prefix prefix` ...
fn parse_binding(alt: &str, prefix: KeyChord, command: Command) -> Result<Parsed, String> {
    let alt = alt.trim();
    let (scope, rest) = match alt.split_once(' ') {
        Some((word, rest)) => match Scope::parse(word) {
            Some(scope) => (scope, rest.trim()),
            None => return Err(format!("unknown scope \"{word}\"")),
        },
        None => (Scope::Direct, alt),
    };
    if rest.is_empty() {
        return Err("missing key".into());
    }
    if scope == Scope::Prefix && rest == "prefix" {
        return Ok(Parsed {
            scope,
            chords: vec![prefix],
            label: format!("{prefix} {prefix}"),
        });
    }
    let prefix_label = |chord: &str| match scope {
        Scope::Prefix => format!("{prefix} {chord}"),
        other => format!("{}{chord}", other.label()),
    };
    // Digit range: `1-9`, `alt+1-9`.
    if let Some(mods) = rest.strip_suffix("1-9")
        && (mods.is_empty() || mods.ends_with('+'))
    {
        if !matches!(command, Command::JumpAgent(_)) {
            return Err("a 1-9 range only works for switch-agent-by-index".into());
        }
        let mut chords = Vec::new();
        for n in 1..=9u8 {
            let text = format!("{mods}{n}");
            chords.push(KeyChord::parse(&text).ok_or_else(|| format!("bad key \"{rest}\""))?);
        }
        let shown = chords[0].to_string();
        let shown = shown.trim_end_matches('1');
        return Ok(Parsed {
            scope,
            chords,
            label: prefix_label(&format!("{shown}1-9")),
        });
    }
    let chord = KeyChord::parse(rest).ok_or_else(|| format!("bad key \"{rest}\""))?;
    if scope == Scope::Direct && chord == prefix {
        return Err("that is the prefix key".into());
    }
    Ok(Parsed {
        scope,
        chords: vec![chord],
        label: prefix_label(&chord.to_string()),
    })
}

impl Keymap {
    /// Defaults for a given prefix chord.
    pub fn with_prefix(prefix: KeyChord) -> Self {
        Self::build(prefix, &[]).0
    }

    /// Build from the config's `prefixKey` (falls back to Ctrl+A on garbage).
    pub fn from_prefix_key(prefix_key: &str) -> Self {
        Self::from_user_rules(prefix_key, &[]).0
    }

    pub fn from_user_rules(
        prefix_key: &str,
        rules: &[as_core::keybindings::KeybindingRule],
    ) -> (Self, Vec<String>) {
        let prefix = KeyChord::parse(prefix_key).unwrap_or_else(|| KeyChord::ctrl('a'));
        Self::build(prefix, rules)
    }

    /// Defaults merged with the user's overrides. The second value lists problems (bad keys,
    /// conflicts); a command whose override is broken keeps its default.
    pub fn build(
        prefix: KeyChord,
        user: &[as_core::keybindings::KeybindingRule],
    ) -> (Self, Vec<String>) {
        use as_core::keybindings::{COMMANDS, split_keys};
        let mut problems = Vec::new();
        let mut km = Keymap {
            prefix,
            prefix_bindings: Vec::new(),
            app_bindings: Vec::new(),
            grid_bindings: Vec::new(),
            scroll_bindings: Vec::new(),
            direct_bindings: Vec::new(),
            labels: Vec::new(),
        };
        // The last override of a command wins; user commands are placed before the defaults so
        // that a conflict is always decided in the user's favour.
        let mut overrides: Vec<(&str, &str)> = Vec::new();
        for rule in user {
            let keys = rule.keys.trim();
            if keys.is_empty() {
                continue;
            }
            match overrides.iter_mut().find(|(c, _)| *c == rule.command) {
                Some(e) => e.1 = keys,
                None => overrides.push((rule.command.as_str(), keys)),
            }
        }
        // (table scope, chord) -> (command, from the user)
        let mut owners: Vec<((Scope, KeyChord), (Command, bool))> = Vec::new();
        let mut plan: Vec<(Command, &str, bool)> = Vec::new();
        for (id, keys) in &overrides {
            if let Some(cmd) = Command::from_id(id) {
                plan.push((cmd, keys, true));
            }
        }
        for def in COMMANDS {
            let Some(cmd) = Command::from_id(def.id) else {
                continue;
            };
            if overrides.iter().any(|(id, _)| *id == def.id) {
                continue;
            }
            plan.push((cmd, def.default_keys, false));
        }
        for (cmd, keys, from_user) in plan {
            let mut parsed = Vec::new();
            for alt in split_keys(keys) {
                match parse_binding(alt, prefix, cmd) {
                    Ok(p) => parsed.push(p),
                    Err(why) => {
                        problems.push(format!(
                            "{}: invalid keys \"{alt}\" ({why}); using the defaults",
                            cmd.id()
                        ));
                        parsed.clear();
                        // Fall back to the catalog default of this command.
                        let default = as_core::keybindings::command_def(cmd.id())
                            .map_or("", |d| d.default_keys);
                        parsed.extend(
                            split_keys(default)
                                .into_iter()
                                .filter_map(|a| parse_binding(a, prefix, cmd).ok()),
                        );
                        break;
                    }
                }
            }
            let mut shown = Vec::new();
            for p in parsed {
                let mut placed = false;
                for chord in &p.chords {
                    let command = match cmd {
                        Command::JumpAgent(_) => match chord.code {
                            KeyCode::Char(c) if c.is_ascii_digit() => {
                                Command::JumpAgent(c as u8 - b'0')
                            }
                            _ => cmd,
                        },
                        other => other,
                    };
                    match owners.iter().find(|(k, _)| *k == (p.scope, *chord)) {
                        Some((_, (other, other_user))) if *other != command => {
                            if from_user {
                                problems.push(format!(
                                    "{} is bound to both {} and {}; keeping {}",
                                    p.label,
                                    other.id(),
                                    cmd.id(),
                                    other.id()
                                ));
                            } else if *other_user {
                                problems.push(format!(
                                    "{} now runs {} (yours); {} lost this key",
                                    p.label,
                                    other.id(),
                                    cmd.id()
                                ));
                            }
                        }
                        Some(_) => {}
                        None => {
                            owners.push(((p.scope, *chord), (command, from_user)));
                            km.table(p.scope).push((*chord, command));
                            placed = true;
                        }
                    }
                }
                if placed {
                    shown.push(p.label);
                }
            }
            // `prefix a` stays an alias of the default prefix key (tmux habit).
            if cmd == Command::SendPrefix
                && !from_user
                && prefix == KeyChord::ctrl('a')
                && lookup(&km.prefix_bindings, KeyChord::plain('a')).is_none()
            {
                km.prefix_bindings
                    .push((KeyChord::plain('a'), Command::SendPrefix));
            }
            if !shown.is_empty() {
                km.labels.push((cmd.id(), shown));
            }
        }
        (km, problems)
    }

    fn table(&mut self, scope: Scope) -> &mut Bindings {
        match scope {
            Scope::Prefix => &mut self.prefix_bindings,
            Scope::App => &mut self.app_bindings,
            Scope::Grid => &mut self.grid_bindings,
            Scope::Scroll => &mut self.scroll_bindings,
            Scope::Direct => &mut self.direct_bindings,
        }
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

    pub fn in_grid_focus(&self, key: &KeyEvent) -> Option<Command> {
        lookup(&self.grid_bindings, KeyChord::from_event(key))
    }

    pub fn in_scroll_mode(&self, key: &KeyEvent) -> Option<Command> {
        lookup(&self.scroll_bindings, KeyChord::from_event(key))
    }

    pub fn direct(&self, key: &KeyEvent) -> Option<Command> {
        lookup(&self.direct_bindings, KeyChord::from_event(key))
    }

    /// Chords bound to `command` in the prefix table.
    pub fn prefix_keys_for(&self, command: Command) -> Vec<KeyChord> {
        self.prefix_bindings
            .iter()
            .filter(|(_, c)| *c == command)
            .map(|(k, _)| *k)
            .collect()
    }

    /// Display strings of every binding of a command id (`Ctrl+A g`, `sidebar j`, `Alt+1-9`).
    pub fn labels_for(&self, id: &str) -> &[String] {
        self.labels
            .iter()
            .find(|(i, _)| *i == id)
            .map_or(&[][..], |(_, l)| l.as_slice())
    }

    /// The first binding of a command id, as a hint next to its name.
    pub fn hint_for(&self, id: &str) -> Option<&str> {
        self.labels_for(id).first().map(String::as_str)
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
        assert_eq!(get('z'), Some(Command::ToggleTileExpand));
        assert_eq!(get('w'), None);
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

    use as_core::keybindings::KeybindingRule;

    fn rule(command: &str, keys: &str) -> KeybindingRule {
        KeybindingRule {
            command: command.into(),
            keys: keys.into(),
        }
    }

    fn build(rules: &[KeybindingRule]) -> (Keymap, Vec<String>) {
        Keymap::build(KeyChord::ctrl('a'), rules)
    }

    #[test]
    fn every_catalog_command_maps_to_a_command_and_back() {
        for def in as_core::keybindings::COMMANDS {
            match Command::from_id(def.id) {
                Some(cmd) => assert_eq!(cmd.id(), def.id),
                None => assert!(!def.implemented, "{} is implemented but unmapped", def.id),
            }
        }
        for cmd in ALL_COMMANDS {
            let def = as_core::keybindings::command_def(cmd.id());
            assert!(
                def.is_some_and(|d| d.implemented),
                "{:?} has no catalog entry",
                cmd
            );
        }
    }

    #[test]
    fn defaults_have_no_problems_and_no_unbound_commands() {
        let (km, problems) = build(&[]);
        assert!(problems.is_empty(), "{problems:?}");
        for cmd in ALL_COMMANDS {
            let def = as_core::keybindings::command_def(cmd.id()).unwrap();
            if !def.default_keys.is_empty() {
                assert!(!km.labels_for(cmd.id()).is_empty(), "{} unbound", cmd.id());
            }
        }
    }

    #[test]
    fn new_defaults_toggle_view_and_palette() {
        let km = Keymap::from_prefix_key("ctrl+a");
        let get = |c| km.after_prefix(&ev(KeyCode::Char(c), KeyModifiers::NONE));
        assert_eq!(get('g'), Some(Command::ToggleView));
        assert_eq!(get('\\'), Some(Command::ToggleView));
        assert_eq!(get('O'), Some(Command::ShowOverview));
        assert_eq!(get('v'), Some(Command::CycleView));
        assert_eq!(get('p'), Some(Command::CommandPalette));
        assert_eq!(get(':'), Some(Command::CommandPalette));
        assert_eq!(get('b'), Some(Command::Broadcast));
        assert_eq!(get('/'), Some(Command::FilterSidebar));
        assert_eq!(
            km.after_prefix(&ev(KeyCode::Left, KeyModifiers::SHIFT)),
            Some(Command::GridMovePrev)
        );
        assert_eq!(
            km.in_grid_focus(&ev(KeyCode::Char('h'), KeyModifiers::NONE)),
            Some(Command::GridLeft)
        );
        assert_eq!(
            km.in_app_focus(&ev(KeyCode::Char('h'), KeyModifiers::NONE)),
            Some(Command::ToggleCollapse)
        );
    }

    #[test]
    fn user_override_replaces_the_defaults_of_that_command_only() {
        let (km, problems) = build(&[rule("new-agent", "prefix N")]);
        assert!(problems.is_empty(), "{problems:?}");
        let get = |c| km.after_prefix(&ev(KeyCode::Char(c), KeyModifiers::NONE));
        assert_eq!(get('N'), Some(Command::NewAgent));
        assert_eq!(get('n'), None, "the old key is gone");
        assert_eq!(get('o'), Some(Command::ResumeSession));
        // The override replaced every default of the command, the sidebar's included.
        assert_eq!(
            km.in_app_focus(&ev(KeyCode::Char('n'), KeyModifiers::NONE)),
            None
        );
        assert_eq!(km.hint_for("new-agent"), Some("Ctrl+A N"));
    }

    #[test]
    fn direct_chords_and_ranges() {
        let (km, problems) = build(&[
            rule("switch-agent-by-index", "alt+1-9"),
            rule("toggle-view", "ctrl+g | prefix g"),
        ]);
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(
            km.direct(&ev(KeyCode::Char('3'), KeyModifiers::ALT)),
            Some(Command::JumpAgent(3))
        );
        assert_eq!(
            km.direct(&ev(KeyCode::Char('g'), KeyModifiers::CONTROL)),
            Some(Command::ToggleView)
        );
        assert_eq!(
            km.after_prefix(&ev(KeyCode::Char('3'), KeyModifiers::NONE)),
            None
        );
        assert_eq!(km.hint_for("switch-agent-by-index"), Some("Alt+1-9"));
        assert_eq!(km.hint_for("toggle-view"), Some("Ctrl+G"));
        let default = build(&[]).0;
        assert_eq!(
            default.hint_for("switch-agent-by-index"),
            Some("Ctrl+A 1-9")
        );
    }

    #[test]
    fn conflicts_are_reported_and_the_user_wins() {
        let (km, problems) = build(&[rule("new-agent", "prefix r")]);
        assert_eq!(
            km.after_prefix(&ev(KeyCode::Char('r'), KeyModifiers::NONE)),
            Some(Command::NewAgent)
        );
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(problems[0].contains("rename-agent") && problems[0].contains("new-agent"));

        let (_, problems) = build(&[rule("new-agent", "prefix w"), rule("quit", "prefix w")]);
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(problems[0].contains("bound to both"));
    }

    #[test]
    fn invalid_keys_fall_back_to_the_default_and_report() {
        let (km, problems) = build(&[
            rule("new-agent", "prefix nope"),
            rule("quit", "hyper+q"),
            rule("toggle-view", "prefix 1-9"),
            rule("show-overview", "ctrl+a"),
        ]);
        assert_eq!(problems.len(), 4, "{problems:?}");
        let get = |c| km.after_prefix(&ev(KeyCode::Char(c), KeyModifiers::NONE));
        assert_eq!(get('n'), Some(Command::NewAgent));
        assert_eq!(get('q'), Some(Command::Quit));
        assert_eq!(get('g'), Some(Command::ToggleView));
        assert_eq!(get('O'), Some(Command::ShowOverview));
    }

    #[test]
    fn none_unbinds_and_later_phase_commands_are_ignored() {
        let (km, problems) = build(&[rule("quit", "none"), rule("remote-control", "ctrl+g")]);
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(
            km.after_prefix(&ev(KeyCode::Char('q'), KeyModifiers::NONE)),
            None
        );
        assert!(km.direct_bindings.is_empty());
    }

    #[test]
    fn prefix_prefix_follows_the_configured_prefix() {
        let (km, _) = Keymap::build(KeyChord::ctrl('b'), &[]);
        assert_eq!(
            km.after_prefix(&ev(KeyCode::Char('b'), KeyModifiers::CONTROL)),
            Some(Command::SendPrefix)
        );
        assert_eq!(km.hint_for("send-prefix"), Some("Ctrl+B Ctrl+B"));
    }
}
