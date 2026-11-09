//! Headless run types (Hydra `shared/types.ts`, camelCase JSON).

use serde::{Deserialize, Serialize};

use crate::types::{ModelId, ProviderId};

pub const DEFAULT_LOG_TAIL_LINES: usize = 400;
pub const DEFAULT_LOG_MAX_CHARS: usize = 120_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "lowercase")]
pub enum HeadlessRunStatus {
    #[default]
    Running,
    Completed,
    Errored,
    Canceled,
}

impl HeadlessRunStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Errored => "errored",
            Self::Canceled => "canceled",
        }
    }
    pub const ALL: [HeadlessRunStatus; 4] = [
        Self::Running,
        Self::Completed,
        Self::Errored,
        Self::Canceled,
    ];
}

/// Status filter of a list request (`all` = no filter).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "lowercase")]
pub enum StatusFilter {
    #[default]
    All,
    Running,
    Completed,
    Errored,
    Canceled,
}

impl StatusFilter {
    pub fn matches(self, status: HeadlessRunStatus) -> bool {
        matches!(
            (self, status),
            (Self::All, _)
                | (Self::Running, HeadlessRunStatus::Running)
                | (Self::Completed, HeadlessRunStatus::Completed)
                | (Self::Errored, HeadlessRunStatus::Errored)
                | (Self::Canceled, HeadlessRunStatus::Canceled)
        )
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Errored => "errored",
            Self::Canceled => "canceled",
        }
    }
}

/// One run. Loading is tolerant: only `id` is required.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase", default)]
pub struct HeadlessRun {
    pub id: String,
    pub prompt: String,
    pub project_dir: String,
    pub provider: ProviderId,
    pub model: ModelId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
    pub resume_session_id: Option<String>,
    pub status: HeadlessRunStatus,
    /// RFC 3339.
    pub started_at: String,
    pub ended_at: Option<String>,
    pub session_id: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase", default)]
pub struct StartHeadlessRunPayload {
    pub prompt: String,
    pub project_dir: String,
    pub provider: ProviderId,
    pub model: ModelId,
    pub reasoning_effort: Option<String>,
    pub resume_session_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase", default)]
pub struct ListHeadlessRunsOptions {
    pub query: Option<String>,
    pub status: StatusFilter,
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase", default)]
pub struct HeadlessRunLogOptions {
    pub tail_lines: Option<usize>,
    pub max_chars: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct HeadlessRunLog {
    pub run_id: String,
    pub content: String,
    pub total_lines: usize,
    pub returned_lines: usize,
    pub truncated: bool,
}

/// What the orchestrator announces.
#[derive(Debug, Clone, PartialEq)]
pub enum HeadlessEvent {
    /// One stdout line (or stderr text) of a run.
    Output { run_id: String, data: String },
    /// A run changed state (started, session id captured, finished).
    Changed(Box<HeadlessRun>),
}
