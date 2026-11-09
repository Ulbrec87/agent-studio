//! Usage data model (Hydra `shared/usage/types.ts`). Wire/cache shape is camelCase like Hydra's.

use serde::{Deserialize, Serialize};

use super::codex_line::CodexParsedLimits;

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, ts_rs::TS,
)]
#[serde(rename_all = "lowercase")]
pub enum UsageProvider {
    Claude,
    Codex,
}

/// Providers in display order.
pub const USAGE_PROVIDERS: [UsageProvider; 2] = [UsageProvider::Claude, UsageProvider::Codex];

impl UsageProvider {
    pub fn label(self) -> &'static str {
        match self {
            UsageProvider::Claude => "Claude",
            UsageProvider::Codex => "Codex",
        }
    }
    pub fn index(self) -> usize {
        match self {
            UsageProvider::Claude => 0,
            UsageProvider::Codex => 1,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "lowercase")]
pub enum UsagePeriod {
    Today,
    Week,
    Month,
}

/// Token counts of one or more model calls. `input` is fresh (uncached) input only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase", default)]
pub struct TokenCounts {
    pub input: u64,
    /// Includes reasoning/thinking tokens.
    pub output: u64,
    pub cache_write5m: u64,
    pub cache_write1h: u64,
    pub cache_read: u64,
}

impl TokenCounts {
    pub fn add(&self, other: &TokenCounts) -> TokenCounts {
        TokenCounts {
            input: self.input + other.input,
            output: self.output + other.output,
            cache_write5m: self.cache_write5m + other.cache_write5m,
            cache_write1h: self.cache_write1h + other.cache_write1h,
            cache_read: self.cache_read + other.cache_read,
        }
    }
    pub fn total(&self) -> u64 {
        self.input + self.output + self.cache_write5m + self.cache_write1h + self.cache_read
    }
}

/// One usage record as parsed from a log line.
#[derive(Debug, Clone, PartialEq)]
pub struct UsageRecord {
    /// Epoch ms.
    pub ts: i64,
    pub provider: UsageProvider,
    /// Normalised model id (see `pricing::normalize_model`).
    pub model: String,
    /// Folder/cwd the call was made in, `""` when unknown.
    pub project_key: String,
    pub tokens: TokenCounts,
}

/// Aggregate of records in one UTC hour for one (provider, model, project).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct UsageBucket {
    /// Epoch ms at the start of the UTC hour.
    pub hour: i64,
    pub provider: UsageProvider,
    pub model: String,
    pub project_key: String,
    pub tokens: TokenCounts,
    pub messages: u64,
    /// Estimated cost of the priced records only.
    pub cost_usd: f64,
    /// Records whose model has no price.
    pub unpriced_messages: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct ProviderUsage {
    pub provider: UsageProvider,
    pub tokens: TokenCounts,
    pub total_tokens: u64,
    pub messages: u64,
    pub cost_usd: f64,
    pub cost_partial: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct ModelUsage {
    pub model: String,
    pub provider: UsageProvider,
    pub total_tokens: u64,
    pub cost_usd: f64,
}

/// One chart slot (an hour, a day) with its tokens per provider (`USAGE_PROVIDERS` order).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct UsageSlot {
    pub start: i64,
    pub label: String,
    pub tokens_by_provider: [u64; 2],
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct TimeRange {
    pub start: i64,
    pub end: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct PreviousUsage {
    pub total_tokens: u64,
    pub cost_usd: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct UsageSummary {
    pub period: UsagePeriod,
    pub range: TimeRange,
    pub total_tokens: u64,
    pub messages: u64,
    pub cost_usd: f64,
    /// Some records had no price, so `cost_usd` is a lower bound.
    pub cost_partial: bool,
    /// One entry per `USAGE_PROVIDERS`, in that order (zeros when unused).
    pub by_provider: Vec<ProviderUsage>,
    /// Sorted by `total_tokens`, largest first.
    pub by_model: Vec<ModelUsage>,
    pub slots: Vec<UsageSlot>,
    /// `None` when the previous range has no data.
    pub previous: Option<PreviousUsage>,
    /// `(current - previous) / previous * 100`, `None` when `previous` is `None` or 0.
    pub tokens_change_pct: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "kebab-case")]
pub enum LimitWindowKind {
    Session,
    Weekly,
    WeeklyModel,
    Other,
}

/// One plan-limit window (5-hour session, weekly, weekly for one model).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct LimitWindow {
    pub provider: UsageProvider,
    /// Claude profile name or Codex account id; `default` when unknown.
    pub account: String,
    /// Stable within (provider, account): `session` | `weekly` | `weekly:<model-slug>` | other kind.
    pub id: String,
    pub kind: LimitWindowKind,
    /// Display label, e.g. `5-hour window`, `Weekly`.
    pub label: String,
    /// 0-100, clamped.
    pub used_percent: f64,
    /// Epoch ms.
    pub resets_at: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "kebab-case")]
pub enum ProfileStatus {
    Ok,
    NotSignedIn,
    Expired,
    Unavailable,
    RateLimited,
    Error,
    KeychainDenied,
}

/// Plan limits of one Claude profile. Only percentages, times and labels: never a credential.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeProfileLimits {
    /// Profile label (`default`, `work`, ...).
    pub account: String,
    pub config_dir: String,
    pub plan: Option<String>,
    pub status: ProfileStatus,
    /// Last good values when `status` is not `Ok` (may be empty).
    pub windows: Vec<LimitWindow>,
    /// When `windows` were fetched.
    pub fetched_at: Option<i64>,
    /// Last attempt.
    pub checked_at: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct UsageScanProgress {
    pub files_done: u64,
    pub files_total: u64,
    pub bytes_done: u64,
    pub bytes_total: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct IndexingState {
    pub indexing: bool,
    pub progress: Option<UsageScanProgress>,
    pub last_scan_at: Option<i64>,
}

/// What the TUI shows of the usage service; pushed on change.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct UsageState {
    pub claude: Vec<ClaudeProfileLimits>,
    /// Passive limits from the newest Codex rollout line, `None` when none was seen.
    pub codex: Option<CodexParsedLimits>,
    pub indexing: IndexingState,
    /// Bumps whenever the store's data changed; clients refetch summaries when it changes.
    pub data_version: u64,
    pub updated_at: i64,
}
