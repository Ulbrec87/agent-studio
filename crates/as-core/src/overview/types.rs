//! Overview types (Hydra `shared/overview/types.ts`). JSON uses camelCase keys, kebab-case
//! status names and epoch milliseconds, like Hydra's wire format.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::types::{ModelId, ProviderId};

/// What an agent is doing right now, as shown in the Overview.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "kebab-case")]
pub enum OverviewStatus {
    NeedsYou,
    Working,
    Idle,
    Starting,
    Finished,
    Failed,
    /// Restored or imported at startup and not run in this daemon run.
    NotRunning,
}

/// Why a session waits on the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "lowercase")]
pub enum WaitingReason {
    Permission,
    Question,
    Other,
}

/// The status fields of a Claude Code registry entry (undocumented, so loosely typed).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ClaudeRegistryStatusFields {
    /// Live process status: `busy`, `waiting`, `idle` or `shell`.
    pub status: Option<String>,
    /// Background session state: `working`, `blocked`, `done`, `failed` or `stopped`.
    pub state: Option<String>,
    /// Present while waiting, for example `permission prompt`.
    pub waiting_for: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct OverviewSession {
    pub agent_id: String,
    pub name: String,
    pub provider: ProviderId,
    pub model: ModelId,
    pub project_dir: String,
    pub worktree_branch: Option<String>,
    /// Repository name (worktrees group under their main repo).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo_name: Option<String>,
    /// Live branch of `project_dir`; absent when detached or not a repository.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// Short commit id, set when HEAD is detached.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    pub status: OverviewStatus,
    /// Epoch ms when the derived status last changed (kept by the tracker).
    pub status_since: i64,
    /// Epoch ms when its process ended (or its last activity); finished, failed, not-running.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub waiting_reason: Option<WaitingReason>,
    /// What the session asks, one line.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub waiting_text: Option<String>,
    /// Working with no output for at least `QUIET_AFTER_MS`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quiet_for_ms: Option<i64>,
    /// Idle for at least `STALE_AFTER_MS`.
    pub stale: bool,
    pub pid: Option<u32>,
    /// Epoch ms, `0` when the agent never started.
    pub started_at: i64,
    pub last_output_at: i64,
}

impl OverviewSession {
    /// A session with only the required fields set (tests, fixtures).
    pub fn basic(agent_id: &str, status: OverviewStatus) -> Self {
        Self {
            agent_id: agent_id.to_string(),
            name: format!("name-{agent_id}"),
            provider: ProviderId::Claude,
            model: "sonnet".into(),
            project_dir: format!("/work/{agent_id}"),
            worktree_branch: None,
            repo_name: None,
            branch: None,
            commit: None,
            status,
            status_since: 0,
            ended_at: None,
            exit_code: None,
            waiting_reason: None,
            waiting_text: None,
            quiet_for_ms: None,
            stale: false,
            pid: None,
            started_at: 0,
            last_output_at: 0,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase", default)]
pub struct OverviewCounts {
    pub working: u32,
    /// Idle sessions that are not stale.
    pub idle: u32,
    pub stale: u32,
    pub needs_you: u32,
    /// Sessions whose process is alive or starting.
    pub open: u32,
    pub finished: u32,
    pub failed: u32,
    /// Restored or imported sessions that have not run in this daemon run.
    pub not_running: u32,
    pub total: u32,
    /// Longest time in `working`.
    pub longest_working_ms: Option<i64>,
    /// Longest time in `idle` among non-stale sessions.
    pub longest_idle_ms: Option<i64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase", default)]
pub struct OverviewSnapshot {
    pub at: i64,
    pub sessions: Vec<OverviewSession>,
    pub counts: OverviewCounts,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "lowercase")]
pub enum EventKind {
    Appeared,
    Status,
    Ended,
    /// Watching resumed after an unmeasured gap; it has no agent.
    Resumed,
}

/// A change worth logging.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct OverviewEvent {
    /// `<agentId>@<at>:<kind>` (`resumed@<at>` for gaps).
    pub id: String,
    pub at: i64,
    pub kind: EventKind,
    /// Empty for `resumed`.
    pub agent_id: String,
    pub name: String,
    pub project_dir: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<OverviewStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<OverviewStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub waiting_reason: Option<WaitingReason>,
    /// Set when leaving `needs-you`: how long the user was waited on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub waited_ms: Option<i64>,
    /// Set on `resumed`: how long nothing was measured.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gap_ms: Option<i64>,
}

/// One 5 s sample of the counts per status.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct HistoryPoint {
    pub at: i64,
    pub needs_you: u32,
    pub working: u32,
    pub idle: u32,
    pub total: u32,
    /// Status per agent, for the Timeline.
    pub statuses: BTreeMap<String, OverviewStatus>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct HistoryAgent {
    pub name: String,
    pub project_dir: String,
}

/// Answer of `GetOverviewHistory`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase", default)]
pub struct HistoryResponse {
    /// Where the history held begins; time before it is unmeasured.
    pub since: i64,
    /// When this run of the daemon started watching.
    pub started_at: i64,
    /// Each start of the daemon (restored and this run's), oldest first.
    pub starts: Vec<i64>,
    pub points: Vec<HistoryPoint>,
    pub events: Vec<OverviewEvent>,
    pub agents: BTreeMap<String, HistoryAgent>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "lowercase")]
pub enum WaitDirection {
    In,
    Out,
}

/// A move of an agent into (`in`) or out of (`out`) `needs-you`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct WaitTransition {
    pub agent_id: String,
    pub name: String,
    pub project_dir: String,
    pub at: i64,
    pub direction: WaitDirection,
}

/// A stretch of time the daemon was measuring.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
pub struct MeasuredSpan {
    pub start: i64,
    pub end: i64,
}

/// What the wait totals are worked out from: the last 9 days.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase", default)]
pub struct WaitLedgerData {
    pub transitions: Vec<WaitTransition>,
    pub spans: Vec<MeasuredSpan>,
    /// Each start of the daemon, oldest first; a start ends a measured stretch.
    pub starts: Vec<i64>,
}

/// An agent that needs the user now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WaitingNow {
    pub agent_id: String,
    pub name: String,
    /// Epoch ms when the wait began (the session's `status_since`).
    pub since: i64,
}

/// How long agents waited in a stretch of time. Only measured time counts.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct WaitFigure {
    pub ms: i64,
    pub count: u32,
    pub measured_ms: i64,
    /// Part of `ms` that belongs to waits still open.
    pub open_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WaitDayTotal {
    /// Local day, `2026-10-22`.
    pub day: String,
    pub figure: WaitFigure,
    /// False when nothing was measured that day.
    pub measured: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LongestWait {
    pub agent_id: String,
    pub name: String,
    pub ms: i64,
    pub count: u32,
    pub waiting_now: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LongestLists {
    pub today: Vec<LongestWait>,
    pub week: Vec<LongestWait>,
}

/// Time agents waited on the user.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WaitTotals {
    pub today: WaitFigure,
    /// Today and the six days before it.
    pub week: WaitFigure,
    pub by_day: Vec<WaitDayTotal>,
    pub longest: LongestLists,
}

/// One bar of "Waited on you".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WaitBar {
    pub agent_id: String,
    pub name: String,
    pub ms: i64,
    pub count: u32,
    /// The agent needs the user now.
    pub open: bool,
}

/// The last message of a session, or why there is none.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase", untagged)]
pub enum LastMessage {
    Text { text: String, truncated: bool },
    Unavailable { unavailable: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(rename = "OverviewTokenCounts")]
pub struct TokenCounts {
    pub input: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cached: Option<u64>,
    pub output: u64,
}

/// What the details drawer shows, read on demand from the transcript.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct AgentDetails {
    pub last_message: LastMessage,
    /// Token counts of the newest reply.
    pub tokens: Option<TokenCounts>,
}

impl AgentDetails {
    pub fn unavailable(reason: &str) -> Self {
        Self {
            last_message: LastMessage::Unavailable {
                unavailable: reason.to_string(),
            },
            tokens: None,
        }
    }
}
