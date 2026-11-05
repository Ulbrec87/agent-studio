//! Command palette (`prefix p` / `prefix :`): a fuzzy list of every palette command with its key
//! hint, "Switch to agent" entries and "Switch view" entries (Hydra `CommandPalette.tsx`).

use as_core::keybindings::{COMMANDS, category_rank};
use as_core::{AgentState, ViewMode};
use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::grid::basename;
use crate::keymap::{Command, Keymap};
use crate::widgets::TextArea;

#[derive(Debug, Clone, PartialEq)]
pub enum PaletteAction {
    Command(Command),
    SwitchAgent(String),
    SwitchView(ViewMode),
}

#[derive(Debug, Clone, PartialEq)]
pub struct PaletteEntry {
    pub label: String,
    pub category: String,
    /// Extra text on the right (project of an agent).
    pub detail: String,
    /// Key binding to show, e.g. `Ctrl+A g`.
    pub hint: String,
    pub action: PaletteAction,
}

/// What a key press in the palette asks the app to do.
#[derive(Debug, Clone, PartialEq)]
pub enum PaletteOutcome {
    None,
    Close,
    Run(PaletteAction),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Palette {
    pub query: TextArea,
    pub entries: Vec<PaletteEntry>,
    /// Indices into `entries`, best match first.
    pub filtered: Vec<usize>,
    pub cursor: usize,
}

pub fn view_label(view: ViewMode) -> &'static str {
    match view {
        ViewMode::Chat => "Chat",
        ViewMode::Grid => "Grid",
        ViewMode::Overview => "Overview",
    }
}

/// Entries in their default order: commands by category, then the views, then the agents.
pub fn build_entries(
    keymap: &Keymap,
    agents: &[AgentState],
    current: ViewMode,
) -> Vec<PaletteEntry> {
    let mut commands: Vec<_> = COMMANDS
        .iter()
        .filter(|c| c.implemented && c.show_in_palette)
        .filter_map(|c| Some((c, Command::from_id(c.id)?)))
        .collect();
    commands.sort_by_key(|(c, _)| category_rank(c.category));
    let mut out: Vec<PaletteEntry> = commands
        .into_iter()
        .map(|(c, cmd)| PaletteEntry {
            label: c.label.to_string(),
            category: c.category.to_string(),
            detail: String::new(),
            hint: keymap.hint_for(c.id).unwrap_or_default().to_string(),
            action: PaletteAction::Command(cmd),
        })
        .collect();
    for view in [ViewMode::Chat, ViewMode::Grid, ViewMode::Overview] {
        out.push(PaletteEntry {
            label: format!("Switch view: {}", view_label(view)),
            category: "Navigation".to_string(),
            detail: if view == current {
                "current".to_string()
            } else {
                String::new()
            },
            hint: String::new(),
            action: PaletteAction::SwitchView(view),
        });
    }
    for agent in agents {
        out.push(PaletteEntry {
            label: format!("Switch to agent: {}", agent.name),
            category: "Agents".to_string(),
            detail: basename(&agent.project_dir),
            hint: String::new(),
            action: PaletteAction::SwitchAgent(agent.id.clone()),
        });
    }
    out
}

/// Indices of the entries matching `query`, best first; all of them in order for an empty query.
pub fn filter(entries: &[PaletteEntry], query: &str) -> Vec<usize> {
    let query = query.trim();
    if query.is_empty() {
        return (0..entries.len()).collect();
    }
    let pattern = Pattern::parse(query, CaseMatching::Smart, Normalization::Smart);
    let mut matcher = Matcher::new(Config::DEFAULT);
    let mut buf = Vec::new();
    let mut scored: Vec<(u32, usize)> = entries
        .iter()
        .enumerate()
        .filter_map(|(i, e)| {
            let hay = format!("{} {} {}", e.label, e.category, e.detail);
            pattern
                .score(Utf32Str::new(&hay, &mut buf), &mut matcher)
                .map(|score| (score, i))
        })
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    scored.into_iter().map(|(_, i)| i).collect()
}

impl Palette {
    pub fn new(entries: Vec<PaletteEntry>) -> Self {
        let filtered = (0..entries.len()).collect();
        Self {
            query: TextArea::single_line(""),
            entries,
            filtered,
            cursor: 0,
        }
    }

    pub fn selected(&self) -> Option<&PaletteEntry> {
        self.filtered.get(self.cursor).map(|i| &self.entries[*i])
    }

    /// A new query starts at the best match.
    fn refilter(&mut self) {
        self.filtered = filter(&self.entries, &self.query.text());
        self.cursor = 0;
    }

    pub fn handle_paste(&mut self, text: &str) {
        self.query.insert_str(text.lines().next().unwrap_or(""));
        self.refilter();
    }

    fn step(&mut self, delta: isize) {
        let n = self.filtered.len();
        if n == 0 {
            return;
        }
        self.cursor = (self.cursor as isize + delta).clamp(0, n as isize - 1) as usize;
    }

    pub fn handle_key(&mut self, key: &KeyEvent, page: usize) -> PaletteOutcome {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => PaletteOutcome::Close,
            KeyCode::Char('c' | 'g') if ctrl => PaletteOutcome::Close,
            KeyCode::Enter => match self.selected() {
                Some(entry) => PaletteOutcome::Run(entry.action.clone()),
                None => PaletteOutcome::None,
            },
            KeyCode::Down | KeyCode::Tab => {
                self.step(1);
                PaletteOutcome::None
            }
            KeyCode::Up | KeyCode::BackTab => {
                self.step(-1);
                PaletteOutcome::None
            }
            KeyCode::Char('n' | 'j') if ctrl => {
                self.step(1);
                PaletteOutcome::None
            }
            KeyCode::Char('p' | 'k') if ctrl => {
                self.step(-1);
                PaletteOutcome::None
            }
            KeyCode::PageDown => {
                self.step(page as isize);
                PaletteOutcome::None
            }
            KeyCode::PageUp => {
                self.step(-(page as isize));
                PaletteOutcome::None
            }
            _ => {
                let before = self.query.text();
                self.query.handle_key(key);
                if self.query.text() != before {
                    self.refilter();
                }
                PaletteOutcome::None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use as_core::AgentStatus;

    fn agent(id: &str, name: &str, dir: &str) -> AgentState {
        let mut a = AgentState::default();
        a.config.id = id.into();
        a.config.name = name.into();
        a.config.project_dir = dir.into();
        a.status = AgentStatus::Idle;
        a
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn palette() -> Palette {
        let km = Keymap::from_prefix_key("ctrl+a");
        let agents = vec![
            agent("a1", "api-refactor", "/work/api"),
            agent("w1", "ui-fix", "/work/web"),
        ];
        Palette::new(build_entries(&km, &agents, ViewMode::Chat))
    }

    fn labels(p: &Palette) -> Vec<&str> {
        p.filtered
            .iter()
            .map(|i| p.entries[*i].label.as_str())
            .collect()
    }

    fn type_text(p: &mut Palette, text: &str) {
        for c in text.chars() {
            p.handle_key(&key(KeyCode::Char(c)), 5);
        }
    }

    #[test]
    fn lists_commands_views_and_agents_with_key_hints() {
        let p = palette();
        let by_label = |l: &str| p.entries.iter().find(|e| e.label == l).unwrap();
        assert_eq!(by_label("Toggle Chat / Grid View").hint, "Ctrl+A g");
        assert_eq!(by_label("New Agent").hint, "Ctrl+A n");
        assert_eq!(
            by_label("Switch to agent: ui-fix").action,
            PaletteAction::SwitchAgent("w1".into())
        );
        assert_eq!(by_label("Switch to agent: ui-fix").detail, "web");
        assert_eq!(
            by_label("Switch view: Grid").action,
            PaletteAction::SwitchView(ViewMode::Grid)
        );
        assert_eq!(by_label("Switch view: Chat").detail, "current");
        // Hidden commands (index switching, the palette itself) and later phases stay out.
        assert!(!p.entries.iter().any(|e| e.label == "Open Command Palette"));
        assert!(!p.entries.iter().any(|e| e.label == "Check for Updates"));
        // Commands come first, ordered by category.
        assert_eq!(p.entries[0].category, "Navigation");
        assert!(matches!(p.entries[0].action, PaletteAction::Command(_)));
        assert!(matches!(
            p.entries.last().unwrap().action,
            PaletteAction::SwitchAgent(_)
        ));
    }

    #[test]
    fn fuzzy_filtering_ranks_the_best_match_first() {
        let mut p = palette();
        type_text(&mut p, "togview");
        assert_eq!(labels(&p)[0], "Toggle Chat / Grid View");
        let mut p = palette();
        type_text(&mut p, "uifix");
        assert_eq!(labels(&p)[0], "Switch to agent: ui-fix");
        let mut p = palette();
        type_text(&mut p, "zzzzqq");
        assert!(p.filtered.is_empty());
        assert_eq!(p.handle_key(&key(KeyCode::Enter), 5), PaletteOutcome::None);
    }

    #[test]
    fn category_and_project_are_searchable() {
        let mut p = palette();
        type_text(&mut p, "web");
        assert!(labels(&p).contains(&"Switch to agent: ui-fix"));
        assert!(!labels(&p).contains(&"Switch to agent: api-refactor"));
    }

    #[test]
    fn enter_runs_the_selected_entry_and_a_new_query_resets_the_cursor() {
        let mut p = palette();
        type_text(&mut p, "yolo");
        p.handle_key(&key(KeyCode::Down), 5);
        assert_eq!(p.cursor, 1);
        let second = p.selected().unwrap().action.clone();
        assert_eq!(
            p.handle_key(&key(KeyCode::Enter), 5),
            PaletteOutcome::Run(second)
        );
        p.handle_key(&key(KeyCode::Backspace), 5);
        assert_eq!(p.cursor, 0);
    }

    #[test]
    fn navigation_is_clamped_and_esc_closes() {
        let mut p = palette();
        p.handle_key(&key(KeyCode::Up), 5);
        assert_eq!(p.cursor, 0);
        for _ in 0..500 {
            p.handle_key(&key(KeyCode::Down), 5);
        }
        assert_eq!(p.cursor, p.filtered.len() - 1);
        p.handle_key(&key(KeyCode::PageUp), 5);
        assert_eq!(p.cursor, p.filtered.len() - 6);
        assert_eq!(p.handle_key(&key(KeyCode::Esc), 5), PaletteOutcome::Close);
    }

    #[test]
    fn empty_query_keeps_the_default_order() {
        let p = palette();
        assert_eq!(p.filtered, (0..p.entries.len()).collect::<Vec<_>>());
    }
}
