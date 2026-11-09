//! Agent, provider and config types shared by every crate.
//!
//! Ported from Hydra `shared/types.ts` (agent/provider/config parts). JSON uses
//! camelCase keys and every struct is `#[serde(default)]` so partial or old
//! files still load.

use std::collections::BTreeMap;
use std::ops::{Deref, DerefMut};

use serde::{Deserialize, Serialize};

use crate::usage_settings::UsageSettings;

// ── Agent ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "lowercase")]
pub enum AgentStatus {
    Running,
    Idle,
    Errored,
    Starting,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "lowercase")]
pub enum WorkMode {
    #[default]
    Local,
    Worktree,
}

/// Free-form model identifier: any string is accepted so new models work
/// without code changes.
pub type ModelId = String;

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    PartialOrd,
    Ord,
    Default,
    Serialize,
    Deserialize,
    ts_rs::TS,
)]
#[serde(rename_all = "lowercase")]
pub enum ProviderId {
    #[default]
    Claude,
    Codex,
    Opencode,
}

impl ProviderId {
    pub const ALL: [ProviderId; 3] = [ProviderId::Claude, ProviderId::Codex, ProviderId::Opencode];

    pub fn as_str(self) -> &'static str {
        match self {
            ProviderId::Claude => "claude",
            ProviderId::Codex => "codex",
            ProviderId::Opencode => "opencode",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ProviderId::Claude => "Claude",
            ProviderId::Codex => "Codex",
            ProviderId::Opencode => "OpenCode",
        }
    }
}

impl std::fmt::Display for ProviderId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Static part of an agent (what the user asked for).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase", default)]
pub struct AgentConfig {
    pub id: String,
    pub name: String,
    pub project_dir: String,
    pub provider: ProviderId,
    pub model: ModelId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
    /// Explicit Claude profile (`CLAUDE_CONFIG_DIR`) chosen at creation; `Some("")` = default profile.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub claude_config_dir: Option<String>,
    pub yolo: bool,
    pub is_manager: bool,
    pub session_id: Option<String>,
    pub initial_prompt: String,
    pub created_at: String,
    pub work_mode: WorkMode,
    pub worktree_path: Option<String>,
    pub worktree_branch: Option<String>,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            project_dir: String::new(),
            provider: ProviderId::Claude,
            model: String::new(),
            reasoning_effort: None,
            claude_config_dir: None,
            yolo: false,
            is_manager: false,
            session_id: None,
            initial_prompt: String::new(),
            created_at: String::new(),
            work_mode: WorkMode::Local,
            worktree_path: None,
            worktree_branch: None,
        }
    }
}

/// `AgentConfig` plus runtime fields. Derefs to the config so
/// `state.provider`, `state.yolo`, ... work directly.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase", default)]
pub struct AgentState {
    #[serde(flatten)]
    pub config: AgentConfig,
    pub status: AgentStatus,
    pub pid: Option<u32>,
    pub restart_count: u32,
    pub started_at: Option<String>,
    /// When the agent's process last ended; `None` when unknown or running.
    pub ended_at: Option<String>,
    pub last_activity_at: String,
}

impl Default for AgentState {
    fn default() -> Self {
        Self {
            config: AgentConfig::default(),
            status: AgentStatus::Idle,
            pid: None,
            restart_count: 0,
            started_at: None,
            ended_at: None,
            last_activity_at: String::new(),
        }
    }
}

impl Deref for AgentState {
    type Target = AgentConfig;
    fn deref(&self) -> &AgentConfig {
        &self.config
    }
}
impl DerefMut for AgentState {
    fn deref_mut(&mut self) -> &mut AgentConfig {
        &mut self.config
    }
}

// ── Providers ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase", default)]
pub struct ProviderModelOption {
    pub id: ModelId,
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub hidden: bool,
    pub is_default: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub reasoning_efforts: Vec<String>,
    pub default_reasoning_effort: Option<String>,
}

/// `(id, label, is_default)` — Hydra `PROVIDER_MODELS`.
type StaticModel = (&'static str, &'static str, bool);

const CLAUDE_MODELS: &[StaticModel] = &[
    ("opus", "Opus", false),
    ("sonnet", "Sonnet", false),
    ("haiku", "Haiku", false),
];
const CODEX_MODELS: &[StaticModel] = &[
    ("gpt-5.3-codex", "GPT-5.3 Codex", true),
    ("gpt-5.4", "GPT-5.4", false),
    ("gpt-5.2-codex", "GPT-5.2 Codex", false),
    ("gpt-5.1-codex-max", "GPT-5.1 Codex Max", false),
    ("gpt-5.2", "GPT-5.2", false),
    ("gpt-5.1-codex-mini", "GPT-5.1 Codex Mini", false),
];
const OPENCODE_MODELS: &[StaticModel] = &[
    ("opencode/big-pickle", "Big Pickle", true),
    ("opencode/gpt-5-nano", "GPT-5 Nano", false),
    ("opencode/qwen3.6-plus-free", "Qwen3.6 Plus Free", false),
    (
        "opencode/nemotron-3-super-free",
        "Nemotron 3 Super Free",
        false,
    ),
    ("opencode/minimax-m2.5-free", "MiniMax M2.5 Free", false),
];

fn static_models(provider: ProviderId) -> &'static [StaticModel] {
    match provider {
        ProviderId::Claude => CLAUDE_MODELS,
        ProviderId::Codex => CODEX_MODELS,
        ProviderId::Opencode => OPENCODE_MODELS,
    }
}

/// Built-in model list of a provider (`PROVIDER_MODELS[provider]`).
pub fn provider_models(provider: ProviderId) -> Vec<ProviderModelOption> {
    static_models(provider)
        .iter()
        .map(|(id, label, is_default)| ProviderModelOption {
            id: (*id).to_string(),
            label: (*label).to_string(),
            is_default: *is_default,
            ..Default::default()
        })
        .collect()
}

/// Ids of the built-in models of a provider.
pub fn provider_model_ids(provider: ProviderId) -> impl Iterator<Item = &'static str> {
    static_models(provider).iter().map(|(id, _, _)| *id)
}

pub fn get_default_model_for_provider(provider: ProviderId) -> ModelId {
    static_models(provider)[0].0.to_string()
}

pub fn get_provider_for_model(model: &str) -> ProviderId {
    ProviderId::ALL
        .into_iter()
        .find(|p| provider_model_ids(*p).any(|id| id == model))
        .unwrap_or(ProviderId::Claude)
}

pub fn is_valid_model_for_provider(provider: ProviderId, model: &str) -> bool {
    provider_model_ids(provider).any(|id| id == model)
}

// ── Editors ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "lowercase")]
pub enum EditorId {
    #[default]
    Vscode,
    Cursor,
    Windsurf,
    Antigravity,
    Zed,
    Finder,
    Terminal,
}

#[derive(Debug, Clone, Copy)]
pub struct EditorDefinition {
    pub id: EditorId,
    pub label: &'static str,
    pub command: &'static str,
    pub extra_args: &'static [&'static str],
}

pub const EDITOR_REGISTRY: &[EditorDefinition] = &[
    EditorDefinition {
        id: EditorId::Vscode,
        label: "VS Code",
        command: "code",
        extra_args: &[],
    },
    EditorDefinition {
        id: EditorId::Cursor,
        label: "Cursor",
        command: "cursor",
        extra_args: &[],
    },
    EditorDefinition {
        id: EditorId::Windsurf,
        label: "Windsurf",
        command: "windsurf",
        extra_args: &[],
    },
    EditorDefinition {
        id: EditorId::Antigravity,
        label: "Antigravity",
        command: "antigravity",
        extra_args: &[],
    },
    EditorDefinition {
        id: EditorId::Zed,
        label: "Zed",
        command: "zed",
        extra_args: &[],
    },
    EditorDefinition {
        id: EditorId::Finder,
        label: "Finder",
        command: "open",
        extra_args: &[],
    },
    EditorDefinition {
        id: EditorId::Terminal,
        label: "Terminal",
        command: "open",
        extra_args: &["-a", "Terminal"],
    },
];

// ── Config enums ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "lowercase")]
pub enum ThemeId {
    Light,
    Dark,
    #[default]
    Midnight,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "lowercase")]
pub enum ViewMode {
    Grid,
    #[default]
    Chat,
    Overview,
}

/// `'auto' | 2 | 3` in JSON.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, ts_rs::TS)]
#[ts(type = "'auto' | 2 | 3")]
pub enum GridColumns {
    #[default]
    Auto,
    Two,
    Three,
}

impl Serialize for GridColumns {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            GridColumns::Auto => s.serialize_str("auto"),
            GridColumns::Two => s.serialize_u8(2),
            GridColumns::Three => s.serialize_u8(3),
        }
    }
}

impl<'de> Deserialize<'de> for GridColumns {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Text(String),
            Number(u64),
        }
        match Raw::deserialize(d)? {
            Raw::Text(s) if s == "auto" => Ok(GridColumns::Auto),
            Raw::Text(s) if s == "2" => Ok(GridColumns::Two),
            Raw::Text(s) if s == "3" => Ok(GridColumns::Three),
            Raw::Number(2) => Ok(GridColumns::Two),
            Raw::Number(3) => Ok(GridColumns::Three),
            _ => Err(serde::de::Error::custom(
                "gridColumns must be \"auto\", 2 or 3",
            )),
        }
    }
}

/// Terminal shell wrapping mode.
/// * `direct` — spawn the CLI binary directly.
/// * `login` — spawn through the user's login shell so rc files set PATH.
/// * `custom` — use `terminalShellPath` + `terminalShellArgs` verbatim.
/// * `auto` — `login` on macOS/Linux, `direct` on Windows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "lowercase")]
pub enum TerminalShellMode {
    #[default]
    Auto,
    Direct,
    Login,
    Custom,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "lowercase")]
pub enum TerminalCursorStyle {
    Block,
    #[default]
    Bar,
    Underline,
}

/// How background free terminals are managed across project switches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "lowercase")]
pub enum FreeTerminalLifecyclePolicy {
    /// Keep all running until the user closes them.
    #[default]
    Explicit,
    /// Cap at `freeTerminalMaxCount`, evict least-recently-used.
    Lru,
    /// Kill terminals with no input for `freeTerminalIdleTimeoutMinutes`.
    Idle,
}

/// Presentation of an auxiliary panel (Git, Files).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "lowercase")]
pub enum PanelDisplayMode {
    #[default]
    Overlay,
    Split,
}

pub const MAX_CONCURRENT_AGENTS_HARD_LIMIT: usize = 10;

/// Hydra's default for `terminalFontFamily`.
pub const DEFAULT_TERMINAL_FONT_FAMILY: &str = "\"SF Mono\", \"Menlo\", \"Monaco\", monospace";
/// The Settings panel of the desktop app clamps the font size to this range (Hydra: 8-32).
pub const TERMINAL_FONT_SIZE_RANGE: std::ops::RangeInclusive<u32> = 8..=32;

/// Application config (`config.json`). Hydra `AppConfig` plus `checkForUpdates`.
/// Keys of old config files that no longer exist (`prefixKey`, `mouse`, `diffLayout`) are ignored on load.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase", default)]
pub struct AppConfig {
    pub schema_version: u32,
    pub default_provider: ProviderId,
    pub default_model: ModelId,
    pub global_yolo: bool,
    pub max_agents: usize,
    pub theme: ThemeId,
    pub default_view_mode: ViewMode,
    pub grid_columns: GridColumns,
    pub default_project_dir: String,
    pub default_editor: EditorId,
    pub import_sessions_on_startup: bool,
    pub session_import_limit: u32,
    pub session_max_age_days: u32,
    pub session_import_project_prefix: String,
    pub hidden_session_ids: Vec<String>,
    /// Deprecated, kept so old config files round-trip.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage_daily_token_budget: Option<f64>,
    /// Deprecated.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage_daily_cost_budget_usd: Option<f64>,
    /// Deprecated.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage_budget_warning_threshold_pct: Option<f64>,
    /// Terminal bell / OS notification sounds (Hydra: sound effects).
    pub enable_sound_effects: bool,
    pub enable_remote_error_reporting: bool,
    pub error_reporting_endpoint: String,
    pub include_sensitive_diagnostics: bool,
    pub remote_control_enabled: bool,
    pub remote_session_timeout_minutes: u32,
    // Terminal / shell
    pub terminal_shell_mode: TerminalShellMode,
    /// Absolute path to a custom shell; only used when mode is `custom`.
    pub terminal_shell_path: String,
    /// Extra args for the shell wrapper, split on whitespace.
    pub terminal_shell_args: String,
    pub terminal_cursor_style: TerminalCursorStyle,
    pub terminal_cursor_blink: bool,
    /// Desktop app only (xterm.js): CSS font-family list.
    pub terminal_font_family: String,
    /// Desktop app only: font size in px, kept within [`TERMINAL_FONT_SIZE_RANGE`].
    pub terminal_font_size: u32,
    /// Desktop app only: use the WebGL renderer of xterm.js.
    pub terminal_enable_webgl: bool,
    // Free terminals
    pub free_terminal_lifecycle_policy: FreeTerminalLifecyclePolicy,
    pub free_terminal_max_count: u32,
    pub free_terminal_idle_timeout_minutes: u32,
    pub free_terminal_scrollback_lines: u32,
    // Panels: `overlay` is a floating popup, `split` a side column.
    pub git_panel_display_mode: PanelDisplayMode,
    pub editor_panel_display_mode: PanelDisplayMode,
    /// Claude profile (`CLAUDE_CONFIG_DIR`) per project directory. Subfolders
    /// inherit; the longest matching path wins. `""` forces the default profile.
    pub claude_profile_by_project: BTreeMap<String, String>,
    pub usage: UsageSettings,
    /// The daemon asks GitHub Releases for a newer version (at start and every 6 h).
    pub check_for_updates: bool,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            schema_version: 1,
            default_provider: ProviderId::Claude,
            default_model: "sonnet".into(),
            global_yolo: false,
            max_agents: 8,
            theme: ThemeId::Midnight,
            default_view_mode: ViewMode::Chat,
            grid_columns: GridColumns::Auto,
            default_project_dir: String::new(),
            default_editor: EditorId::Vscode,
            import_sessions_on_startup: true,
            session_import_limit: 500,
            session_max_age_days: 7,
            session_import_project_prefix: String::new(),
            hidden_session_ids: Vec::new(),
            usage_daily_token_budget: None,
            usage_daily_cost_budget_usd: None,
            usage_budget_warning_threshold_pct: None,
            enable_sound_effects: true,
            enable_remote_error_reporting: false,
            error_reporting_endpoint: String::new(),
            include_sensitive_diagnostics: false,
            remote_control_enabled: false,
            remote_session_timeout_minutes: 480,
            terminal_shell_mode: TerminalShellMode::Auto,
            terminal_shell_path: String::new(),
            terminal_shell_args: String::new(),
            terminal_cursor_style: TerminalCursorStyle::Bar,
            terminal_cursor_blink: false,
            terminal_font_family: DEFAULT_TERMINAL_FONT_FAMILY.into(),
            terminal_font_size: 12,
            terminal_enable_webgl: false,
            free_terminal_lifecycle_policy: FreeTerminalLifecyclePolicy::Explicit,
            free_terminal_max_count: 6,
            free_terminal_idle_timeout_minutes: 60,
            free_terminal_scrollback_lines: 5000,
            git_panel_display_mode: PanelDisplayMode::Overlay,
            editor_panel_display_mode: PanelDisplayMode::Overlay,
            claude_profile_by_project: BTreeMap::new(),
            usage: UsageSettings::default(),
            check_for_updates: true,
        }
    }
}

/// Create-agent request (Hydra `CreateAgentPayload`).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase", default)]
pub struct CreateAgentPayload {
    pub name: String,
    pub project_dir: String,
    pub provider: ProviderId,
    pub model: ModelId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
    /// Explicit Claude profile for this agent; `Some("")` = default profile, `None` = project mapping.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub claude_config_dir: Option<String>,
    pub yolo: bool,
    pub initial_prompt: String,
    pub resume_session_id: Option<String>,
    pub is_manager: bool,
    pub work_mode: WorkMode,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn defaults_match_hydra() {
        let c = AppConfig::default();
        assert_eq!(c.theme, ThemeId::Midnight);
        assert_eq!(c.max_agents, 8);
        assert_eq!(c.remote_session_timeout_minutes, 480);
        let v = serde_json::to_value(&c).unwrap();
        assert_eq!(v["gridColumns"], json!("auto"));
        assert_eq!(v["defaultViewMode"], json!("chat"));
        assert!(v.get("usageDailyTokenBudget").is_none());
    }

    #[test]
    fn grid_columns_json() {
        assert_eq!(serde_json::to_value(GridColumns::Two).unwrap(), json!(2));
        assert_eq!(
            serde_json::from_value::<GridColumns>(json!(3)).unwrap(),
            GridColumns::Three
        );
        assert_eq!(
            serde_json::from_value::<GridColumns>(json!("auto")).unwrap(),
            GridColumns::Auto
        );
        assert!(serde_json::from_value::<GridColumns>(json!(4)).is_err());
    }

    #[test]
    fn partial_config_loads() {
        let c: AppConfig =
            serde_json::from_value(json!({ "maxAgents": 3, "theme": "dark" })).unwrap();
        assert_eq!(c.max_agents, 3);
        assert_eq!(c.theme, ThemeId::Dark);
        assert_eq!(c.default_model, "sonnet");
    }

    #[test]
    fn agent_state_is_flat_camel_case() {
        let mut s = AgentState::default();
        s.config.id = "abcd1234".into();
        s.config.project_dir = "/p".into();
        s.status = AgentStatus::Running;
        let v = serde_json::to_value(&s).unwrap();
        assert_eq!(v["id"], json!("abcd1234"));
        assert_eq!(v["projectDir"], json!("/p"));
        assert_eq!(v["status"], json!("running"));
        assert_eq!(v["workMode"], json!("local"));
        let back: AgentState = serde_json::from_value(v).unwrap();
        assert_eq!(back, s);
        assert_eq!(s.id, "abcd1234");
    }

    #[test]
    fn model_tables() {
        assert_eq!(get_default_model_for_provider(ProviderId::Claude), "opus");
        assert_eq!(get_provider_for_model("gpt-5.4"), ProviderId::Codex);
        assert_eq!(get_provider_for_model("unknown"), ProviderId::Claude);
        assert!(is_valid_model_for_provider(
            ProviderId::Opencode,
            "opencode/big-pickle"
        ));
        assert!(!is_valid_model_for_provider(ProviderId::Claude, "gpt-5.4"));
        assert_eq!(
            provider_models(ProviderId::Codex)
                .iter()
                .filter(|m| m.is_default)
                .count(),
            1
        );
    }
}
