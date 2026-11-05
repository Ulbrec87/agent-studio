//! Settings screen (`prefix ,`): sections with editable values written through `UpdateConfig`
//! patches, the skills tabs, keybindings, data (Hydra import, diagnostics) and About.

use as_core::ProviderId;
use as_core::hydra_import::{ImportPreview, ImportReport};
use as_core::skills::{SkillInfo, SkillScan, SkillToggle};
use as_core::{AppConfig, sessions::ClaudeProfile};
use as_proto::{DaemonStatus, Request, Response};
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use serde_json::{Value, json};

use super::{Outcome, Reply, nav, req, step};
use crate::app::Effect;
use crate::widgets::TextArea;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Bool,
    Choice(&'static [&'static str]),
    Number { min: i64, max: i64, step: i64 },
    Text,
}

#[derive(Debug, Clone, Copy)]
pub struct Item {
    /// Dotted JSON path in the config (`usage.showCost`).
    pub path: &'static str,
    pub label: &'static str,
    pub kind: Kind,
    pub help: &'static str,
}

const fn item(path: &'static str, label: &'static str, kind: Kind, help: &'static str) -> Item {
    Item {
        path,
        label,
        kind,
        help,
    }
}

const PROVIDERS: &[&str] = &["claude", "codex", "opencode"];
const THEMES: &[&str] = &["light", "dark", "midnight"];
const VIEWS: &[&str] = &["chat", "grid", "overview"];
const SHELL_MODES: &[&str] = &["auto", "direct", "login", "custom"];
const CURSORS: &[&str] = &["block", "bar", "underline"];
const POLICIES: &[&str] = &["explicit", "lru", "idle"];
const PANEL_MODES: &[&str] = &["overlay", "split"];
const DIFF_LAYOUTS: &[&str] = &["unified", "split"];
const CHIPS: &[&str] = &["auto", "claude", "codex"];

pub const GENERAL: &[Item] = &[
    item(
        "defaultProvider",
        "Default provider",
        Kind::Choice(PROVIDERS),
        "Provider preselected in the New Agent dialog",
    ),
    item(
        "defaultModel",
        "Default model",
        Kind::Text,
        "Model id preselected for new agents",
    ),
    item("theme", "Theme", Kind::Choice(THEMES), "Colors of the TUI"),
    item(
        "defaultViewMode",
        "Default view",
        Kind::Choice(VIEWS),
        "View shown at start",
    ),
    item(
        "maxAgents",
        "Max running agents",
        Kind::Number {
            min: 1,
            max: 10,
            step: 1,
        },
        "Hard limit is 10",
    ),
    item(
        "globalYolo",
        "Global YOLO",
        Kind::Bool,
        "Skip permission prompts for every new agent",
    ),
    item(
        "defaultProjectDir",
        "Default project folder",
        Kind::Text,
        "Preset of the Directory field",
    ),
    item(
        "enableSoundEffects",
        "Sound effects",
        Kind::Bool,
        "Bell when an agent needs you",
    ),
    item(
        "prefixKey",
        "Prefix key",
        Kind::Text,
        "tmux-style prefix, e.g. ctrl+a (applies at once)",
    ),
    item(
        "checkForUpdates",
        "Check for updates",
        Kind::Bool,
        "Ask GitHub for a newer release at start and every 6 h",
    ),
    item(
        "mouse",
        "Mouse support",
        Kind::Bool,
        "Clicks and wheel; hides native text selection (Shift+drag). Applies at next start",
    ),
    item(
        "importSessionsOnStartup",
        "Import sessions at start",
        Kind::Bool,
        "Add Claude/Codex sessions as idle agents",
    ),
    item(
        "sessionImportLimit",
        "Session import limit",
        Kind::Number {
            min: 0,
            max: 5000,
            step: 50,
        },
        "0 = no limit",
    ),
    item(
        "sessionMaxAgeDays",
        "Session max age (days)",
        Kind::Number {
            min: 0,
            max: 365,
            step: 1,
        },
        "0 = any age",
    ),
    item(
        "sessionImportProjectPrefix",
        "Import only under",
        Kind::Text,
        "Only sessions of projects below this folder",
    ),
];

pub const TERMINAL: &[Item] = &[
    item(
        "terminalShellMode",
        "Shell mode",
        Kind::Choice(SHELL_MODES),
        "How agents and terminals start their shell",
    ),
    item(
        "terminalShellPath",
        "Shell path",
        Kind::Text,
        "Used by the custom mode",
    ),
    item(
        "terminalShellArgs",
        "Shell args",
        Kind::Text,
        "Whitespace separated, e.g. -lc",
    ),
    item(
        "terminalCursorStyle",
        "Cursor style",
        Kind::Choice(CURSORS),
        "Recorded; the TUI uses your terminal's cursor",
    ),
    item(
        "terminalCursorBlink",
        "Cursor blink",
        Kind::Bool,
        "Recorded; the TUI uses your terminal's cursor",
    ),
    item(
        "freeTerminalLifecyclePolicy",
        "Free terminals",
        Kind::Choice(POLICIES),
        "explicit: keep, lru: cap the count, idle: kill unused",
    ),
    item(
        "freeTerminalMaxCount",
        "Max free terminals",
        Kind::Number {
            min: 1,
            max: 50,
            step: 1,
        },
        "Used by the lru policy",
    ),
    item(
        "freeTerminalIdleTimeoutMinutes",
        "Idle timeout (min)",
        Kind::Number {
            min: 1,
            max: 1440,
            step: 5,
        },
        "Used by the idle policy",
    ),
    item(
        "freeTerminalScrollbackLines",
        "Scrollback lines",
        Kind::Number {
            min: 100,
            max: 100_000,
            step: 500,
        },
        "Per free terminal",
    ),
];

pub const PANELS: &[Item] = &[
    item(
        "gitPanelDisplayMode",
        "Git panel",
        Kind::Choice(PANEL_MODES),
        "overlay: centered, split: right column",
    ),
    item(
        "editorPanelDisplayMode",
        "Files panel",
        Kind::Choice(PANEL_MODES),
        "overlay: centered, split: right column",
    ),
    item(
        "diffLayout",
        "Diff layout",
        Kind::Choice(DIFF_LAYOUTS),
        "unified, or split (side by side, needs a git panel of 100+ columns); `s` toggles it in the Git panel",
    ),
];

pub const USAGE: &[Item] = &[
    item(
        "usage.showCost",
        "Show costs",
        Kind::Bool,
        "Estimated API-equivalent cost (off = hide costs)",
    ),
    item(
        "usage.alertsEnabled",
        "Limit alerts",
        Kind::Bool,
        "Notify when a plan window nears its limit",
    ),
    item(
        "usage.sessionAlertPercent",
        "Session alert at %",
        Kind::Number {
            min: 50,
            max: 100,
            step: 5,
        },
        "5-hour window",
    ),
    item(
        "usage.weeklyAlertPercent",
        "Weekly alert at %",
        Kind::Number {
            min: 50,
            max: 100,
            step: 5,
        },
        "Weekly windows",
    ),
    item(
        "usage.chipProvider",
        "Header chip",
        Kind::Choice(CHIPS),
        "Which provider the header chip shows",
    ),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    General,
    Terminal,
    Panels,
    Usage,
    Profiles,
    Skills,
    Keybindings,
    Data,
    About,
}

impl Section {
    pub const ALL: [Section; 9] = [
        Section::General,
        Section::Terminal,
        Section::Panels,
        Section::Usage,
        Section::Profiles,
        Section::Skills,
        Section::Keybindings,
        Section::Data,
        Section::About,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Section::General => "General",
            Section::Terminal => "Terminal / shell",
            Section::Panels => "Panels",
            Section::Usage => "Usage",
            Section::Profiles => "Claude profiles",
            Section::Skills => "Skills",
            Section::Keybindings => "Keybindings",
            Section::Data => "Data & diagnostics",
            Section::About => "About",
        }
    }

    pub fn items(self) -> &'static [Item] {
        match self {
            Section::General => GENERAL,
            Section::Terminal => TERMINAL,
            Section::Panels => PANELS,
            Section::Usage => USAGE,
            _ => &[],
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ImportStage {
    Loading,
    Preview(Box<ImportPreview>),
    Applying,
    Done(Box<ImportReport>),
    Failed(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct SettingsUi {
    pub section: usize,
    /// Focus is on the value list (true) or the section list.
    pub in_items: bool,
    pub cursor: usize,
    pub edit: Option<TextArea>,
    pub config: AppConfig,
    /// Value sent and not yet confirmed: `(path, value)`.
    pending: Option<(String, Value)>,
    pub error: Option<String>,
    pub notice: Option<String>,
    pub skills: Option<SkillScan>,
    pub skills_provider: ProviderId,
    pub skills_error: Option<String>,
    pub profiles: Vec<ClaudeProfile>,
    /// `(label, keys)` of every effective binding.
    pub bindings: Vec<(String, String)>,
    pub status: Option<DaemonStatus>,
    pub mcp: String,
    pub data_dir: String,
    pub import: Option<ImportStage>,
    /// Enter on "Keybindings": the app opens the file in `$EDITOR`.
    pub want_keybindings_file: bool,
    /// The daemon's update state (About).
    pub update: Option<as_core::updates::UpdateState>,
}

impl Default for SettingsUi {
    fn default() -> Self {
        Self {
            section: 0,
            in_items: false,
            cursor: 0,
            edit: None,
            config: AppConfig::default(),
            pending: None,
            error: None,
            notice: None,
            skills: None,
            skills_provider: ProviderId::Claude,
            skills_error: None,
            profiles: Vec::new(),
            bindings: Vec::new(),
            status: None,
            mcp: String::new(),
            data_dir: String::new(),
            import: None,
            want_keybindings_file: false,
            update: None,
        }
    }
}

fn get<'a>(v: &'a Value, path: &str) -> Option<&'a Value> {
    path.split('.').try_fold(v, |acc, k| acc.get(k))
}

fn patch_for(path: &str, value: Value) -> Value {
    match path.split_once('.') {
        Some((a, b)) => json!({ a: { b: value } }),
        None => json!({ path: value }),
    }
}

impl SettingsUi {
    pub fn current(&self) -> Section {
        Section::ALL[self.section]
    }

    /// Open (or re-open) the screen with fresh data.
    pub fn open(
        &mut self,
        config: &AppConfig,
        profiles: Vec<ClaudeProfile>,
        bindings: Vec<(String, String)>,
        mcp: String,
        data_dir: String,
    ) -> Vec<Effect> {
        self.config = config.clone();
        self.profiles = profiles;
        self.bindings = bindings;
        self.mcp = mcp;
        self.data_dir = data_dir;
        self.edit = None;
        self.error = None;
        self.import = None;
        vec![
            req("set:skills", Request::ScanSkills),
            req("set:status", Request::Status),
        ]
    }

    /// The daemon's config changed (ours or someone else's).
    pub fn on_config(&mut self, config: &AppConfig) {
        self.config = config.clone();
        if let Some((path, wanted)) = self.pending.take() {
            let now = serde_json::to_value(config).ok();
            let got = now.as_ref().and_then(|v| get(v, &path)).cloned();
            if got.as_ref() != Some(&wanted) {
                self.error = Some(format!(
                    "{path}: the daemon kept {} instead of {}",
                    got.map_or("nothing".into(), |g| g.to_string()),
                    wanted
                ));
            }
        }
    }

    pub fn value(&self, item: &Item) -> Value {
        serde_json::to_value(&self.config)
            .ok()
            .and_then(|v| get(&v, item.path).cloned())
            .unwrap_or(Value::Null)
    }

    pub fn display(&self, item: &Item) -> String {
        match (self.value(item), item.kind) {
            (Value::Bool(b), _) => if b { "[x]" } else { "[ ]" }.to_string(),
            (Value::String(s), _) if s.is_empty() => "(empty)".into(),
            (Value::String(s), _) => s,
            (Value::Null, _) => "-".into(),
            (v, _) => v.to_string(),
        }
    }

    fn set(&mut self, item: &Item, value: Value) -> Vec<Effect> {
        self.error = None;
        self.pending = Some((item.path.to_string(), value.clone()));
        vec![Effect::PatchConfig(patch_for(item.path, value))]
    }

    pub fn on_reply(&mut self, key: &str, result: Reply) -> Vec<Effect> {
        match (key, result) {
            ("set:skills", Ok(Response::Skills(s))) => {
                self.skills = Some(s);
                self.skills_error = None;
            }
            ("set:skills", Err(e)) => self.skills_error = Some(e),
            ("set:skill-toggle", Ok(_)) => return vec![req("set:skills", Request::ScanSkills)],
            ("set:skill-toggle", Err(e)) => self.skills_error = Some(e),
            ("set:status", Ok(Response::Status(s))) => self.status = Some(s),
            ("set:diag", Ok(Response::Path(p))) => {
                self.notice = Some(format!("Diagnostics written to {p}"))
            }
            ("set:diag", Err(e)) => self.error = Some(e),
            ("set:import-preview", Ok(Response::HydraPreview(p))) => {
                self.import = Some(ImportStage::Preview(p));
            }
            ("set:import-preview", Err(e)) => self.import = Some(ImportStage::Failed(e)),
            ("set:import-apply", Ok(Response::HydraReport(r))) => {
                self.import = Some(ImportStage::Done(r));
            }
            ("set:import-apply", Err(e)) => self.import = Some(ImportStage::Failed(e)),
            _ => {}
        }
        Vec::new()
    }

    pub fn start_import(&mut self) -> Vec<Effect> {
        self.import = Some(ImportStage::Loading);
        vec![req(
            "set:import-preview",
            Request::PreviewHydraImport { dir: None },
        )]
    }

    pub fn skill_list(&self) -> &[SkillInfo] {
        match (&self.skills, self.skills_provider) {
            (Some(s), ProviderId::Claude) => &s.claude,
            (Some(s), ProviderId::Codex) => &s.codex,
            (Some(s), ProviderId::Opencode) => &s.opencode,
            (None, _) => &[],
        }
    }

    fn rows(&self) -> usize {
        match self.current() {
            Section::Skills => self.skill_list().len(),
            Section::Keybindings => self.bindings.len(),
            Section::Data => 2,
            s => s.items().len(),
        }
    }

    pub fn key(&mut self, key: &KeyEvent) -> (Outcome, Vec<Effect>) {
        if self.import.is_some() {
            return (Outcome::Stay, self.import_key(key));
        }
        if self.edit.is_some() {
            return (Outcome::Stay, self.edit_key(key));
        }
        if !self.in_items {
            match key.code {
                KeyCode::Esc | KeyCode::Char('q') => return (Outcome::Close, Vec::new()),
                KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') | KeyCode::Tab => {
                    self.in_items = true;
                    self.cursor = 0;
                }
                _ => {
                    if let Some(d) = nav(key, 5) {
                        self.section = step(self.section, Section::ALL.len(), d);
                        self.cursor = 0;
                        self.notice = None;
                        self.error = None;
                    }
                }
            }
            return (Outcome::Stay, Vec::new());
        }
        match key.code {
            KeyCode::Esc | KeyCode::Left | KeyCode::Char('h')
                if !(self.current() == Section::Skills && key.code != KeyCode::Esc) =>
            {
                self.in_items = false;
                return (Outcome::Stay, Vec::new());
            }
            KeyCode::Char('q') => return (Outcome::Close, Vec::new()),
            _ => {}
        }
        let fx = match self.current() {
            Section::Skills => self.skills_key(key),
            Section::Keybindings => {
                if key.code == KeyCode::Enter {
                    self.want_keybindings_file = true;
                } else if let Some(d) = nav(key, 10) {
                    self.cursor = step(self.cursor, self.rows(), d);
                }
                Vec::new()
            }
            Section::Data => self.data_key(key),
            Section::About => match key.code {
                KeyCode::Enter | KeyCode::Char('c') => {
                    vec![req("upd:check", Request::CheckForUpdates)]
                }
                _ => Vec::new(),
            },
            Section::Profiles => Vec::new(),
            _ => self.item_key(key),
        };
        (Outcome::Stay, fx)
    }

    fn data_key(&mut self, key: &KeyEvent) -> Vec<Effect> {
        match key.code {
            KeyCode::Enter | KeyCode::Char(' ') => {
                self.notice = None;
                self.error = None;
                if self.cursor == 0 {
                    return self.start_import();
                }
                return vec![req("set:diag", Request::ExportDiagnostics)];
            }
            _ => {
                if let Some(d) = nav(key, 2) {
                    self.cursor = step(self.cursor, 2, d);
                }
            }
        }
        Vec::new()
    }

    fn skills_key(&mut self, key: &KeyEvent) -> Vec<Effect> {
        match key.code {
            KeyCode::Tab | KeyCode::Left | KeyCode::Right | KeyCode::Char('h' | 'l') => {
                self.skills_provider = match self.skills_provider {
                    ProviderId::Claude => ProviderId::Codex,
                    _ => ProviderId::Claude,
                };
                self.cursor = 0;
            }
            KeyCode::Char(' ') | KeyCode::Enter => {
                let Some(s) = self.skill_list().get(self.cursor).cloned() else {
                    return Vec::new();
                };
                let toggle = SkillToggle {
                    provider: s.provider,
                    enabled: !s.enabled,
                    group: s.group,
                    config_dir: s.config_dir,
                    path: s.path,
                };
                return vec![req("set:skill-toggle", Request::ToggleSkill(toggle))];
            }
            KeyCode::Char('r') => return vec![req("set:skills", Request::ScanSkills)],
            _ => {
                if let Some(d) = nav(key, 10) {
                    self.cursor = step(self.cursor, self.rows(), d);
                }
            }
        }
        Vec::new()
    }

    fn selected_item(&self) -> Option<&'static Item> {
        self.current().items().get(self.cursor)
    }

    fn item_key(&mut self, key: &KeyEvent) -> Vec<Effect> {
        let Some(it) = self.selected_item() else {
            return Vec::new();
        };
        let current = self.value(it);
        match (it.kind, key.code) {
            (
                Kind::Bool,
                KeyCode::Char(' ' | 'l' | 'h') | KeyCode::Enter | KeyCode::Left | KeyCode::Right,
            ) => {
                return self.set(it, Value::Bool(!current.as_bool().unwrap_or(false)));
            }
            (
                Kind::Choice(opts),
                KeyCode::Char(' ' | 'l' | 'h') | KeyCode::Enter | KeyCode::Left | KeyCode::Right,
            ) => {
                let back = matches!(key.code, KeyCode::Left | KeyCode::Char('h'));
                let now = current
                    .as_str()
                    .and_then(|c| opts.iter().position(|o| *o == c))
                    .unwrap_or(0);
                let n = opts.len() as isize;
                let next = (now as isize + if back { -1 } else { 1 }).rem_euclid(n) as usize;
                return self.set(it, Value::String(opts[next].to_string()));
            }
            (Kind::Number { min, max, step }, KeyCode::Char('+' | '=' | 'l') | KeyCode::Right) => {
                let v = current
                    .as_i64()
                    .unwrap_or(min)
                    .saturating_add(step)
                    .clamp(min, max);
                return self.set(it, json!(v));
            }
            (Kind::Number { min, max, step }, KeyCode::Char('-' | 'h') | KeyCode::Left) => {
                let v = current
                    .as_i64()
                    .unwrap_or(min)
                    .saturating_sub(step)
                    .clamp(min, max);
                return self.set(it, json!(v));
            }
            (Kind::Number { .. } | Kind::Text, KeyCode::Enter | KeyCode::Char(' ')) => {
                let text = match &current {
                    Value::String(s) => s.clone(),
                    Value::Null => String::new(),
                    v => v.to_string(),
                };
                self.edit = Some(TextArea::single_line(&text));
            }
            _ => {
                if let Some(d) = nav(key, 5) {
                    self.cursor = step(self.cursor, self.rows(), d);
                }
            }
        }
        Vec::new()
    }

    fn edit_key(&mut self, key: &KeyEvent) -> Vec<Effect> {
        let Some(mut input) = self.edit.take() else {
            return Vec::new();
        };
        match key.code {
            KeyCode::Esc => return Vec::new(),
            KeyCode::Enter => {
                let Some(it) = self.selected_item() else {
                    return Vec::new();
                };
                let text = input.text().trim().to_string();
                return match it.kind {
                    Kind::Number { min, max, .. } => match text.parse::<i64>() {
                        Ok(n) if (min..=max).contains(&n) => self.set(it, json!(n)),
                        Ok(_) => {
                            self.error =
                                Some(format!("{} must be between {min} and {max}", it.label));
                            self.edit = Some(input);
                            Vec::new()
                        }
                        Err(_) => {
                            self.error = Some(format!("{} must be a number", it.label));
                            self.edit = Some(input);
                            Vec::new()
                        }
                    },
                    _ => self.set(it, Value::String(text)),
                };
            }
            _ => {
                input.handle_key(key);
            }
        }
        self.edit = Some(input);
        Vec::new()
    }

    fn import_key(&mut self, key: &KeyEvent) -> Vec<Effect> {
        match (&self.import, key.code) {
            (Some(ImportStage::Preview(_)), KeyCode::Enter) => {
                self.import = Some(ImportStage::Applying);
                return vec![req(
                    "set:import-apply",
                    Request::ApplyHydraImport { dir: None },
                )];
            }
            (Some(ImportStage::Loading | ImportStage::Applying), _) => {}
            (Some(_), KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q')) => self.import = None,
            _ => {}
        }
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::crossterm::event::KeyModifiers;

    fn k(c: KeyCode) -> KeyEvent {
        KeyEvent::new(c, KeyModifiers::NONE)
    }

    fn ui_at(section: Section) -> SettingsUi {
        SettingsUi {
            section: Section::ALL.iter().position(|s| *s == section).unwrap(),
            in_items: true,
            ..SettingsUi::default()
        }
    }

    fn patch(fx: &[Effect]) -> Value {
        match fx {
            [Effect::PatchConfig(p)] => p.clone(),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn bool_choice_and_number_edits_become_patches() {
        let mut ui = ui_at(Section::General);
        ui.cursor = 5; // global yolo
        let (_, fx) = ui.key(&k(KeyCode::Char(' ')));
        assert_eq!(patch(&fx), json!({"globalYolo": true}));
        ui.cursor = 2; // theme: midnight -> light
        let (_, fx) = ui.key(&k(KeyCode::Right));
        assert_eq!(patch(&fx), json!({"theme": "light"}));
        ui.cursor = 4; // max agents 8 -> 9
        let (_, fx) = ui.key(&k(KeyCode::Char('+')));
        assert_eq!(patch(&fx), json!({"maxAgents": 9}));
        let mut ui = ui_at(Section::Usage);
        let (_, fx) = ui.key(&k(KeyCode::Char(' ')));
        assert_eq!(patch(&fx), json!({"usage": {"showCost": false}}));
    }

    #[test]
    fn text_and_number_entry_validate_before_sending() {
        let mut ui = ui_at(Section::General);
        ui.cursor = 4;
        ui.key(&k(KeyCode::Enter));
        assert!(ui.edit.is_some());
        for _ in 0..3 {
            ui.key(&k(KeyCode::Backspace));
        }
        for c in "99".chars() {
            ui.key(&k(KeyCode::Char(c)));
        }
        let (_, fx) = ui.key(&k(KeyCode::Enter));
        assert!(fx.is_empty(), "99 is above the maximum");
        assert!(ui.error.as_deref().is_some_and(|e| e.contains("between")));
        assert!(ui.edit.is_some(), "stays in the editor");
        ui.key(&k(KeyCode::Backspace));
        ui.key(&k(KeyCode::Backspace));
        ui.key(&k(KeyCode::Char('3')));
        let (_, fx) = ui.key(&k(KeyCode::Enter));
        assert_eq!(patch(&fx), json!({"maxAgents": 3}));
        assert!(ui.edit.is_none());
        // Free text.
        ui.cursor = 1;
        ui.key(&k(KeyCode::Enter));
        let (_, fx) = ui.key(&k(KeyCode::Enter));
        assert_eq!(patch(&fx), json!({"defaultModel": "sonnet"}));
    }

    #[test]
    fn a_value_the_daemon_changes_is_reported() {
        let mut ui = ui_at(Section::General);
        ui.cursor = 4;
        ui.set(&GENERAL[4], json!(50));
        let clamped = AppConfig {
            max_agents: 10,
            ..AppConfig::default()
        };
        ui.on_config(&clamped);
        assert!(ui.error.as_deref().is_some_and(|e| e.contains("maxAgents")));
        // A value that arrives as asked leaves no error.
        ui.error = None;
        ui.set(&GENERAL[4], json!(10));
        ui.on_config(&clamped);
        assert!(ui.error.is_none());
    }

    #[test]
    fn sections_navigation_and_close() {
        let mut ui = SettingsUi::default();
        ui.key(&k(KeyCode::Down));
        assert_eq!(ui.current(), Section::Terminal);
        ui.key(&k(KeyCode::Enter));
        assert!(ui.in_items);
        ui.key(&k(KeyCode::Esc));
        assert!(!ui.in_items);
        let (out, _) = ui.key(&k(KeyCode::Esc));
        assert_eq!(out, Outcome::Close);
    }

    #[test]
    fn skills_tabs_and_toggle() {
        let skill = |id: &str, provider, enabled| SkillInfo {
            id: id.into(),
            name: id.into(),
            description: String::new(),
            provider,
            group: "g".into(),
            enabled,
            path: format!("/x/{id}/SKILL.md"),
            config_dir: "/c".into(),
        };
        let mut ui = ui_at(Section::Skills);
        ui.on_reply(
            "set:skills",
            Ok(Response::Skills(SkillScan {
                claude: vec![skill("a", ProviderId::Claude, true)],
                codex: vec![skill("b", ProviderId::Codex, false)],
                opencode: Vec::new(),
            })),
        );
        let (_, fx) = ui.key(&k(KeyCode::Char(' ')));
        assert!(
            matches!(&fx[..], [Effect::Req { req: Request::ToggleSkill(t), .. }] if !t.enabled && t.provider == ProviderId::Claude)
        );
        ui.key(&k(KeyCode::Tab));
        assert_eq!(ui.skills_provider, ProviderId::Codex);
        let (_, fx) = ui.key(&k(KeyCode::Char(' ')));
        assert!(
            matches!(&fx[..], [Effect::Req { req: Request::ToggleSkill(t), .. }] if t.enabled && t.provider == ProviderId::Codex)
        );
        // After a toggle the list is rescanned.
        let fx = ui.on_reply("set:skill-toggle", Ok(Response::Ok));
        assert!(matches!(
            &fx[..],
            [Effect::Req {
                req: Request::ScanSkills,
                ..
            }]
        ));
    }

    #[test]
    fn hydra_import_flow_previews_then_applies_on_confirm() {
        let mut ui = ui_at(Section::Data);
        let (_, fx) = ui.key(&k(KeyCode::Enter));
        assert!(matches!(
            &fx[..],
            [Effect::Req {
                req: Request::PreviewHydraImport { .. },
                ..
            }]
        ));
        assert_eq!(ui.import, Some(ImportStage::Loading));
        ui.on_reply(
            "set:import-preview",
            Ok(Response::HydraPreview(Box::default())),
        );
        // Esc cancels without applying.
        let mut cancelled = ui.clone();
        let (_, fx) = cancelled.key(&k(KeyCode::Esc));
        assert!(fx.is_empty() && cancelled.import.is_none());
        let (_, fx) = ui.key(&k(KeyCode::Enter));
        assert!(matches!(
            &fx[..],
            [Effect::Req {
                req: Request::ApplyHydraImport { .. },
                ..
            }]
        ));
        ui.on_reply(
            "set:import-apply",
            Ok(Response::HydraReport(Box::default())),
        );
        assert!(matches!(ui.import, Some(ImportStage::Done(_))));
        ui.key(&k(KeyCode::Enter));
        assert!(ui.import.is_none());
    }

    #[test]
    fn keybindings_enter_asks_for_the_file_and_diagnostics_export() {
        let mut ui = ui_at(Section::Keybindings);
        ui.key(&k(KeyCode::Enter));
        assert!(ui.want_keybindings_file);
        let mut ui = ui_at(Section::Data);
        ui.cursor = 1;
        let (_, fx) = ui.key(&k(KeyCode::Enter));
        assert!(matches!(
            &fx[..],
            [Effect::Req {
                req: Request::ExportDiagnostics,
                ..
            }]
        ));
        ui.on_reply("set:diag", Ok(Response::Path("/d".into())));
        assert!(ui.notice.as_deref().is_some_and(|n| n.contains("/d")));
    }
}
