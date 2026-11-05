//! Protocol messages. JSON, adjacently tagged (`{"type": "...", "payload": ...}`) with snake_case
//! names, so adding a variant never changes the encoding of the others. Binary data is base64.

use as_core::agents::providers::PreflightResult;
use as_core::fs::{DirSuggestion, FileContent, FsEntry, SearchHit};
use as_core::git::{
    GitBranch, GitCommit, GitDiffStats, GitFileContents, GitPrDiff, GitStatus, GitWorktree,
};
use as_core::headless::{
    HeadlessRun, HeadlessRunLog, HeadlessRunLogOptions, ListHeadlessRunsOptions,
    StartHeadlessRunPayload,
};
use as_core::hydra_import::{ImportPreview, ImportReport};
use as_core::open_in::{OpenOption, OpenTarget};
use as_core::overview::{
    AgentDetails, HistoryResponse, OverviewEvent, OverviewSnapshot, WaitLedgerData,
};
use as_core::sessions::{ClaudeProfile, ListSessionsOptions, SessionSummary};
use as_core::skills::{SkillScan, SkillToggle};
use as_core::terminal_tree::SplitDir;
use as_core::terminals::{FreeTerminalLayout, SpawnedTerminal, TerminalInfo};
use as_core::updates::UpdateState;
use as_core::usage::report::{UsageReport, UsageReportProvider};
use as_core::usage::{UsagePeriod, UsageState, UsageSummary};
use as_core::{
    AgentStatus, AppConfig, CreateAgentPayload, ModelId, ProviderId, ProviderModelOption,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Bumped when an incompatible change is made to the messages below.
pub const PROTOCOL_VERSION: u32 = 1;

pub type AgentState = as_core::AgentState;

/// client -> daemon.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum ClientMsg {
    Request { id: u64, req: Request },
}

/// daemon -> client.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
#[allow(clippy::large_enum_variant)] // short-lived values, boxing would only clutter the API
pub enum ServerMsg {
    /// Answer to the request with the same `id`.
    Response {
        id: u64,
        result: Result<Response, String>,
    },
    /// Pushed without a request.
    Event(Event),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum Request {
    /// First request of a connection.
    Hello {
        client_version: String,
    },
    Ping,
    ListAgents,
    GetAgent {
        agent_id: String,
    },
    CreateAgent(CreateAgentPayload),
    KillAgent {
        agent_id: String,
    },
    RemoveAgent {
        agent_id: String,
    },
    RestartAgent {
        agent_id: String,
    },
    /// Text input with submit-queue semantics (text, then Enter after a short delay).
    SendInput {
        agent_id: String,
        input: String,
    },
    /// Raw bytes straight to the PTY (key presses, paste).
    SendRaw {
        agent_id: String,
        #[serde(with = "crate::b64")]
        data: Vec<u8>,
    },
    Resize {
        agent_id: String,
        cols: u16,
        rows: u16,
    },
    /// `SendInput` to every running agent of a project.
    Broadcast {
        project_dir: String,
        input: String,
    },
    RenameAgent {
        agent_id: String,
        name: String,
    },
    SetModel {
        agent_id: String,
        model: ModelId,
    },
    ToggleYolo {
        agent_id: String,
        yolo: bool,
    },
    SetGlobalYolo(bool),
    /// Returns the buffered output and starts streaming `Event::AgentOutput` for this agent
    /// to this connection.
    Subscribe {
        agent_id: String,
    },
    Unsubscribe {
        agent_id: String,
    },
    GetConfig,
    /// JSON merge patch, camelCase keys (see `ConfigStore::update`).
    UpdateConfig(Value),
    Preflight {
        provider: Option<ProviderId>,
    },
    Status,
    /// `stop_agents: true` kills every agent first; `false` is refused while any agent runs.
    Shutdown {
        stop_agents: bool,
    },
    /// The client's terminal gained/lost focus. The daemon sends OS notifications only while no
    /// client is focused (Hydra: native notification when the window is not focused).
    ClientFocus(bool),
    /// Sessions of a provider's catalog (P4), newest first. The daemon adds the config's
    /// `hiddenSessionIds` (hidden ones are left out unless `include_hidden`).
    ListSessions(ListSessionsOptions),
    /// Hide/unhide a session in the lists (persisted in `hiddenSessionIds`); answers `Config`.
    HideSession {
        session_id: String,
        hidden: bool,
    },
    /// Claude profiles (`~/.claude`, `~/.claude-<name>`, `$CLAUDE_CONFIG_DIR`) on this machine.
    ListClaudeProfiles,
    /// Map a project dir to a Claude profile (`claudeProfileByProject`): `Some(config_dir)`,
    /// `Some("")` for the default profile, `None` to remove the mapping. Answers `Config`.
    SetProjectProfile {
        project_dir: String,
        config_dir: Option<String>,
    },
    /// Models of a provider, discovered from its CLI when possible (cached).
    ListProviderModels {
        provider: ProviderId,
    },
    /// The Overview snapshot (Hydra `OVERVIEW_GET`); answers `Overview`.
    GetOverview,
    /// Overview history from `since_ms` (epoch ms) on, default the last 6 h (Hydra
    /// `OVERVIEW_HISTORY`); answers `OverviewHistory`.
    GetOverviewHistory {
        #[serde(default)]
        since_ms: Option<i64>,
    },
    /// The wait ledger the totals are worked out from (Hydra `OVERVIEW_WAITS`); answers
    /// `OverviewWaits`.
    GetOverviewWaits,
    /// Overview events from `since_ms` on, newest `limit` of them, oldest first (Hydra
    /// `OVERVIEW_EVENTS`); answers `OverviewEvents`.
    GetOverviewEvents {
        #[serde(default)]
        since_ms: Option<i64>,
        #[serde(default)]
        limit: Option<usize>,
    },
    /// Last message and token counts of an agent for the details drawer (Hydra
    /// `OVERVIEW_AGENT_DETAILS`); answers `AgentDetails` (`None` for an unknown agent).
    GetAgentDetails {
        agent_id: String,
    },
    /// The usage service state: Claude plan limits per profile, Codex passive limits, indexing
    /// progress, `data_version` (P7; Hydra `USAGE_STATE_GET`); answers `UsageState`.
    GetUsageState,
    /// Token summary of a period, optionally of one project directory (`USAGE_SUMMARY_GET`);
    /// answers `UsageSummary`.
    GetUsageSummary {
        period: UsagePeriod,
        #[serde(default)]
        project_key: Option<String>,
    },
    /// Daily report of the last `days` local days (7-90), for one provider or all
    /// (`USAGE_REPORT_GET`); answers `UsageReport`.
    GetUsageReport {
        #[serde(default)]
        provider: UsageReportProvider,
        days: u32,
    },
    /// Scan the logs and poll the plan limits now (`USAGE_REFRESH`); answers `UsageState`.
    RefreshUsage,
    /// This client shows (or stopped showing) a usage surface: the daemon polls the plan limits
    /// every 60 s instead of every 5 min while any client does (`USAGE_VISIBILITY`).
    UsageVisible(bool),
    /// State of the MCP server and the manager workspace (Hydra `MCP_GET_STATUS`); answers `McpStatus`.
    GetMcpStatus,
    /// Start a headless run (P8); answers `HeadlessRun`.
    StartHeadlessRun(StartHeadlessRunPayload),
    /// Runs newest first (query / status / limit); answers `HeadlessRuns`.
    ListHeadlessRuns(ListHeadlessRunsOptions),
    GetHeadlessRun {
        run_id: String,
    },
    /// Stop a running run; answers `Flag` (false when unknown or not running).
    CancelHeadlessRun {
        run_id: String,
    },
    /// Tail of a run's log; answers `HeadlessRunLog`.
    GetHeadlessRunLog {
        run_id: String,
        #[serde(default)]
        options: HeadlessRunLogOptions,
    },

    // ── P9: files ───────────────────────────────────────────────────────────
    /// One directory of a project (`path` relative, `""` = root); answers `FsEntries`.
    FsReadDir {
        project_dir: String,
        path: String,
        #[serde(default)]
        show_hidden: bool,
    },
    /// Preview of a project file (size cap, binary placeholder); answers `FileContent`.
    FsReadFile {
        project_dir: String,
        path: String,
    },
    /// Modification time (ms) of a file, polled by the viewer; answers `Number`.
    FsMtime {
        project_dir: String,
        path: String,
    },
    /// Replace a project text file (the editor's save): same path protections as the reads, size
    /// cap, atomic, a directory is an error; answers `Ok`.
    FsWriteFile {
        project_dir: String,
        path: String,
        content: String,
    },
    /// Folders matching a typed absolute path (`~` allowed), for the new-agent dialog; answers
    /// `DirSuggestions`. Not confined to a project.
    FsListDirs {
        query: String,
    },
    /// Fuzzy file search (gitignore aware); answers `SearchHits`.
    FsSearch {
        project_dir: String,
        query: String,
        #[serde(default)]
        limit: Option<usize>,
    },

    // ── P9: git (blocking `git` / `gh` calls; mutations are serialized per repository) ──
    GitStatus {
        dir: String,
    },
    GitLog {
        dir: String,
        limit: u32,
    },
    /// Unified diff text (whole tree, or one file; staged = index vs HEAD); answers `Text`.
    GitDiff {
        dir: String,
        #[serde(default)]
        file: Option<String>,
        #[serde(default)]
        staged: bool,
    },
    GitDiffStats {
        dir: String,
    },
    GitStage {
        dir: String,
        files: Vec<String>,
    },
    GitUnstage {
        dir: String,
        files: Vec<String>,
    },
    /// Commit the listed files (all staged changes when empty); answers `Text` (the new hash).
    GitCommit {
        dir: String,
        message: String,
        files: Vec<String>,
    },
    /// Answers `Text` (git's output).
    GitPush {
        dir: String,
    },
    GitBranches {
        dir: String,
    },
    GitCheckout {
        dir: String,
        branch: String,
    },
    GitCreateBranch {
        dir: String,
        name: String,
        #[serde(default)]
        from: Option<String>,
    },
    GitFileContents {
        dir: String,
        path: String,
        #[serde(default)]
        staged: bool,
    },
    GitWorktreeCreate {
        dir: String,
        branch: String,
        #[serde(default)]
        start_point: Option<String>,
    },
    GitWorktreeRemove {
        dir: String,
        path: String,
        #[serde(default)]
        delete_branch: Option<String>,
        #[serde(default)]
        force: bool,
    },
    /// Read-only: `gh pr view/diff`; `pr` is a number or a URL.
    GitPrFetch {
        dir: String,
        pr: String,
    },
    GitPrFileDiff {
        dir: String,
        pr: String,
        path: String,
    },

    // ── P9: free terminals ──────────────────────────────────────────────────
    /// Start a shell for a project (new tab, or a split of `group_id`); answers `TerminalSpawned`.
    /// With `group_id`, the new pane splits `split_pane` (default: the tab's active pane) in
    /// direction `split_dir` (default: `horizontal`, i.e. to the right).
    SpawnTerminal {
        project_dir: String,
        #[serde(default)]
        cwd: Option<String>,
        #[serde(default)]
        group_id: Option<String>,
        #[serde(default)]
        split_pane: Option<String>,
        #[serde(default)]
        split_dir: Option<SplitDir>,
        cols: u16,
        rows: u16,
    },
    TerminalInput {
        terminal_id: String,
        #[serde(with = "crate::b64")]
        data: Vec<u8>,
    },
    TerminalResize {
        terminal_id: String,
        cols: u16,
        rows: u16,
    },
    /// Answers `Flag`.
    KillTerminal {
        terminal_id: String,
    },
    /// Answers `Flag`.
    RenameTerminal {
        terminal_id: String,
        label: String,
    },
    /// Receive `TerminalOutput` of this terminal; answers `Buffer` (the scrollback so far).
    SubscribeTerminal {
        terminal_id: String,
    },
    UnsubscribeTerminal {
        terminal_id: String,
    },
    /// Answers `TerminalLayout`. The first call for a project after a daemon restart spawns fresh
    /// shells into its saved layout (lazily, so a restart starts no shells by itself).
    GetTerminalLayout {
        project_dir: String,
    },
    /// Make a tab / pane the active one; answers `TerminalLayout`.
    ActivateTerminal {
        project_dir: String,
        #[serde(default)]
        group_id: Option<String>,
        #[serde(default)]
        pane_id: Option<String>,
    },
    /// Move the divider nearest above `pane_id` along `axis` by `delta` percentage points
    /// (positive: right / down); answers `TerminalLayout`.
    ResizeTerminalPane {
        project_dir: String,
        pane_id: String,
        axis: SplitDir,
        delta: i16,
    },
    /// Answers `Terminals`.
    ListTerminals,

    // ── P9: skills, open-in, import, diagnostics ────────────────────────────
    ScanSkills,
    /// Answers `Ok` (or an error text).
    ToggleSkill(SkillToggle),
    /// Editors / file manager / terminal that look usable here; answers `OpenTargets`.
    ListOpenTargets,
    OpenIn {
        target: OpenTarget,
        path: String,
    },
    /// What importing from Hydra would change (`dir` overrides the default location); answers `HydraPreview`.
    PreviewHydraImport {
        #[serde(default)]
        dir: Option<String>,
    },
    /// Merge Hydra's data into ours; answers `HydraReport`.
    ApplyHydraImport {
        #[serde(default)]
        dir: Option<String>,
    },
    /// Write a diagnostics folder (logs + snapshot, no secrets); answers `Path`.
    ExportDiagnostics,

    // ── P10: updates ────────────────────────────────────────────────────────
    /// The last known update state (no network); answers `UpdateState`.
    GetUpdateState,
    /// Ask GitHub Releases now (even with `checkForUpdates` off); answers `UpdateState`.
    CheckForUpdates,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
#[allow(clippy::large_enum_variant)] // short-lived values, boxing would only clutter the API
pub enum Response {
    Ok,
    Pong,
    Flag(bool),
    Hello(HelloInfo),
    Agents(Vec<AgentState>),
    Agent(Option<AgentState>),
    Ids(Vec<String>),
    Buffer {
        #[serde(with = "crate::b64")]
        data: Vec<u8>,
    },
    Config(Box<AppConfig>),
    Preflight(Vec<ProviderPreflight>),
    Status(DaemonStatus),
    Sessions(Vec<SessionSummary>),
    Profiles(Vec<ClaudeProfile>),
    Models(Vec<ProviderModelOption>),
    Overview(Box<OverviewSnapshot>),
    OverviewHistory(Box<HistoryResponse>),
    OverviewWaits(Box<WaitLedgerData>),
    OverviewEvents(Vec<OverviewEvent>),
    AgentDetails(Option<Box<AgentDetails>>),
    UsageState(Box<UsageState>),
    UsageSummary(Box<UsageSummary>),
    UsageReport(Box<UsageReport>),
    McpStatus(McpStatus),
    HeadlessRun(Option<Box<HeadlessRun>>),
    HeadlessRuns(Vec<HeadlessRun>),
    HeadlessRunLog(Option<HeadlessRunLog>),
    Text(String),
    GitStatus(Box<GitStatus>),
    GitLog(Vec<GitCommit>),
    GitDiffStats(GitDiffStats),
    GitBranches(Vec<GitBranch>),
    GitFileContents(Box<GitFileContents>),
    GitWorktree(Box<GitWorktree>),
    GitPr(Box<GitPrDiff>),
    FsEntries(Vec<FsEntry>),
    FileContent(FileContent),
    Number(u64),
    SearchHits(Vec<SearchHit>),
    DirSuggestions(Vec<DirSuggestion>),
    TerminalSpawned(SpawnedTerminal),
    TerminalLayout(FreeTerminalLayout),
    Terminals(Vec<TerminalInfo>),
    Skills(SkillScan),
    OpenTargets(Vec<OpenOption>),
    HydraPreview(Box<ImportPreview>),
    HydraReport(Box<ImportReport>),
    Path(String),
    UpdateState(Box<UpdateState>),
}

/// The MCP server of the daemon (Hydra `McpServerStatus`).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct McpStatus {
    pub running: bool,
    pub port: Option<u16>,
    pub error: Option<String>,
    /// Folder of the manager agent (`.mcp.json` + `CLAUDE.md`).
    pub manager_workspace: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HelloInfo {
    pub daemon_version: String,
    pub protocol: u32,
    pub pid: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProviderPreflight {
    pub provider: ProviderId,
    pub result: PreflightResult,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DaemonStatus {
    pub pid: u32,
    pub version: String,
    pub protocol: u32,
    pub uptime_secs: u64,
    pub agent_count: usize,
    /// Agents with a process or starting.
    pub running_count: usize,
    pub clients: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum Event {
    /// PTY bytes; only sent to connections subscribed to `agent_id`.
    AgentOutput {
        agent_id: String,
        #[serde(with = "crate::b64")]
        data: Vec<u8>,
    },
    AgentStatus {
        agent_id: String,
        status: AgentStatus,
        session_id: Option<String>,
        model: ModelId,
    },
    /// A running agent was silent for a while (needs attention).
    AgentWaiting {
        agent_id: String,
    },
    /// Agent list changed (created, removed, renamed): re-run `ListAgents`.
    AgentsChanged,
    ConfigChanged(Box<AppConfig>),
    DaemonShuttingDown,
    /// Something the user should know about (toast in the TUI, OS notification when unfocused).
    Notification(StudioNotification),
    /// The Overview snapshot, pushed only when something meaningful changed (not for timers).
    OverviewSnapshot(Box<OverviewSnapshot>),
    /// New Overview events of a poll, oldest first.
    OverviewEvents(Vec<OverviewEvent>),
    /// The usage state changed (limits, indexing progress, new data); summaries are refetched
    /// when `data_version` moved.
    UsageChanged(Box<UsageState>),
    /// One output line of a headless run (stdout line, or stderr wrapped as JSON).
    HeadlessRunEvent {
        run_id: String,
        data: String,
    },
    /// A headless run started, got its session id, or finished.
    HeadlessRunChanged(Box<HeadlessRun>),
    /// Output of a free terminal this client subscribed to.
    TerminalOutput {
        terminal_id: String,
        #[serde(with = "crate::b64")]
        data: Vec<u8>,
    },
    /// A free terminal's process ended (it is already gone from the layout).
    TerminalExit {
        terminal_id: String,
        project_dir: String,
        exit_code: i32,
    },
    /// Tabs / panes / labels of a project's free terminals changed.
    TerminalLayoutChanged {
        project_dir: String,
        layout: FreeTerminalLayout,
    },
    /// A newer release was found (sent once per version found by a check).
    UpdateAvailable(Box<UpdateState>),
}

/// Hydra `NotificationType` (agent part).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotificationKind {
    /// The agent is waiting for input (needs you).
    AgentWaiting,
    /// The agent finished (status went idle).
    AgentIdle,
    AgentErrored,
    AgentStarted,
    /// A plan window or a daily budget is near its limit (opt-in, `usage.alertsEnabled`).
    UsageLimit,
    /// A headless run finished successfully.
    HeadlessCompleted,
    /// A headless run failed.
    HeadlessErrored,
}

/// Hydra `HydraNotification`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StudioNotification {
    pub id: String,
    pub kind: NotificationKind,
    pub title: String,
    pub body: String,
    pub agent_id: Option<String>,
    /// RFC 3339.
    pub timestamp: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn round_trip<T: Serialize + for<'de> Deserialize<'de> + PartialEq + std::fmt::Debug>(v: T) {
        let text = serde_json::to_string(&v).unwrap();
        let back: T = serde_json::from_str(&text).unwrap();
        assert_eq!(back, v, "{text}");
    }

    #[test]
    fn request_encoding_is_stable() {
        let msg = ClientMsg::Request {
            id: 7,
            req: Request::Resize {
                agent_id: "a1".into(),
                cols: 80,
                rows: 24,
            },
        };
        assert_eq!(
            serde_json::to_value(&msg).unwrap(),
            json!({"type":"request","payload":{"id":7,"req":{"type":"resize","payload":{"agent_id":"a1","cols":80,"rows":24}}}})
        );
        assert_eq!(
            serde_json::to_value(Request::Ping).unwrap(),
            json!({"type":"ping"})
        );
    }

    #[test]
    fn raw_bytes_are_base64() {
        let req = Request::SendRaw {
            agent_id: "a".into(),
            data: vec![0, 255, 0x1b, b'[', 0xc3],
        };
        let v = serde_json::to_value(&req).unwrap();
        assert_eq!(v["payload"]["data"], json!("AP8bW8M="));
        round_trip(req);
        round_trip(Event::AgentOutput {
            agent_id: "a".into(),
            data: vec![0xe2, 0x82],
        });
        round_trip(Response::Buffer { data: Vec::new() });
        assert!(
            serde_json::from_value::<Request>(
                json!({"type":"send_raw","payload":{"agent_id":"a","data":"!!"}})
            )
            .is_err()
        );
    }

    #[test]
    fn terminal_split_requests_and_the_old_spawn_shape() {
        // A client that predates splits sends no direction: still understood.
        let old: Request = serde_json::from_value(json!({
            "type": "spawn_terminal",
            "payload": {"project_dir": "/p", "group_id": "g", "cols": 80, "rows": 24}
        }))
        .unwrap();
        assert!(matches!(
            old,
            Request::SpawnTerminal {
                split_pane: None,
                split_dir: None,
                ..
            }
        ));
        for r in [
            Request::SpawnTerminal {
                project_dir: "/p".into(),
                cwd: None,
                group_id: Some("g".into()),
                split_pane: Some("t1".into()),
                split_dir: Some(SplitDir::Vertical),
                cols: 80,
                rows: 24,
            },
            Request::ResizeTerminalPane {
                project_dir: "/p".into(),
                pane_id: "t1".into(),
                axis: SplitDir::Horizontal,
                delta: -5,
            },
        ] {
            round_trip(ClientMsg::Request { id: 1, req: r });
        }
        let v = serde_json::to_value(Request::ResizeTerminalPane {
            project_dir: "/p".into(),
            pane_id: "t1".into(),
            axis: SplitDir::Vertical,
            delta: 5,
        })
        .unwrap();
        assert_eq!(v["payload"]["axis"], json!("vertical"));
    }

    #[test]
    fn all_requests_round_trip() {
        let all = vec![
            Request::Hello {
                client_version: "1".into(),
            },
            Request::Ping,
            Request::ListAgents,
            Request::GetAgent {
                agent_id: "a".into(),
            },
            Request::CreateAgent(CreateAgentPayload {
                name: "n".into(),
                project_dir: "/p".into(),
                ..Default::default()
            }),
            Request::KillAgent {
                agent_id: "a".into(),
            },
            Request::RemoveAgent {
                agent_id: "a".into(),
            },
            Request::RestartAgent {
                agent_id: "a".into(),
            },
            Request::SendInput {
                agent_id: "a".into(),
                input: "hi".into(),
            },
            Request::Broadcast {
                project_dir: "/p".into(),
                input: "x".into(),
            },
            Request::RenameAgent {
                agent_id: "a".into(),
                name: "b".into(),
            },
            Request::SetModel {
                agent_id: "a".into(),
                model: "opus".into(),
            },
            Request::ToggleYolo {
                agent_id: "a".into(),
                yolo: true,
            },
            Request::SetGlobalYolo(true),
            Request::Subscribe {
                agent_id: "a".into(),
            },
            Request::Unsubscribe {
                agent_id: "a".into(),
            },
            Request::GetConfig,
            Request::UpdateConfig(json!({"maxAgents": 3})),
            Request::Preflight {
                provider: Some(ProviderId::Codex),
            },
            Request::Preflight { provider: None },
            Request::Status,
            Request::Shutdown { stop_agents: true },
            Request::ClientFocus(true),
            Request::GetOverview,
            Request::GetOverviewHistory { since_ms: Some(5) },
            Request::GetOverviewHistory { since_ms: None },
            Request::GetOverviewWaits,
            Request::GetOverviewEvents {
                since_ms: Some(1),
                limit: Some(10),
            },
            Request::GetAgentDetails {
                agent_id: "a".into(),
            },
            Request::GetUsageState,
            Request::GetUsageSummary {
                period: UsagePeriod::Week,
                project_key: Some("/p".into()),
            },
            Request::GetUsageSummary {
                period: UsagePeriod::Today,
                project_key: None,
            },
            Request::GetUsageReport {
                provider: UsageReportProvider::Codex,
                days: 30,
            },
            Request::RefreshUsage,
            Request::UsageVisible(true),
        ];
        for r in all {
            round_trip(ClientMsg::Request { id: 1, req: r });
        }
    }

    #[test]
    fn responses_and_events_round_trip() {
        let agent = AgentState::default();
        for r in [
            Response::Ok,
            Response::Pong,
            Response::Flag(true),
            Response::Hello(HelloInfo {
                daemon_version: "0.1.0".into(),
                protocol: PROTOCOL_VERSION,
                pid: 1,
            }),
            Response::Agents(vec![agent.clone()]),
            Response::Agent(Some(agent)),
            Response::Agent(None),
            Response::Ids(vec!["a".into()]),
            Response::Config(Box::default()),
            Response::Preflight(vec![ProviderPreflight {
                provider: ProviderId::Claude,
                result: PreflightResult {
                    ok: false,
                    path: None,
                    version: None,
                    error: Some("nope".into()),
                },
            }]),
            Response::Overview(Box::default()),
            Response::OverviewHistory(Box::default()),
            Response::OverviewWaits(Box::default()),
            Response::OverviewEvents(Vec::new()),
            Response::AgentDetails(Some(Box::new(AgentDetails::unavailable("x")))),
            Response::AgentDetails(None),
            Response::UsageState(Box::default()),
            Response::UsageSummary(Box::new(as_core::usage::summary::summarize(
                &[],
                UsagePeriod::Today,
                &chrono::Utc::now(),
                None,
            ))),
            Response::UsageReport(Box::new(as_core::usage::report::build_usage_report(
                &[],
                7,
                UsageReportProvider::All,
                &chrono::Utc::now(),
            ))),
            Response::Status(DaemonStatus {
                pid: 1,
                version: "v".into(),
                protocol: 1,
                uptime_secs: 3,
                agent_count: 2,
                running_count: 1,
                clients: 1,
            }),
        ] {
            round_trip(ServerMsg::Response {
                id: 2,
                result: Ok(r),
            });
        }
        round_trip(ServerMsg::Response {
            id: 3,
            result: Err("boom".into()),
        });
        for e in [
            Event::AgentStatus {
                agent_id: "a".into(),
                status: AgentStatus::Running,
                session_id: Some("s".into()),
                model: "opus".into(),
            },
            Event::AgentWaiting {
                agent_id: "a".into(),
            },
            Event::AgentsChanged,
            Event::ConfigChanged(Box::default()),
            Event::DaemonShuttingDown,
            Event::OverviewSnapshot(Box::default()),
            Event::OverviewEvents(Vec::new()),
            Event::UsageChanged(Box::default()),
            Event::Notification(StudioNotification {
                id: "n1".into(),
                kind: NotificationKind::AgentWaiting,
                title: "t".into(),
                body: "b".into(),
                agent_id: Some("a".into()),
                timestamp: "2026-01-01T00:00:00Z".into(),
            }),
        ] {
            round_trip(ServerMsg::Event(e));
        }
    }

    #[test]
    fn usage_requests_have_a_stable_encoding() {
        assert_eq!(
            serde_json::to_value(Request::GetUsageReport {
                provider: UsageReportProvider::Claude,
                days: 30
            })
            .unwrap(),
            json!({"type":"get_usage_report","payload":{"provider":"claude","days":30}})
        );
        assert_eq!(
            serde_json::to_value(Request::UsageVisible(true)).unwrap(),
            json!({"type":"usage_visible","payload":true})
        );
        // `project_key` and `provider` may be left out.
        let r: Request = serde_json::from_value(
            json!({"type":"get_usage_summary","payload":{"period":"month"}}),
        )
        .unwrap();
        assert_eq!(
            r,
            Request::GetUsageSummary {
                period: UsagePeriod::Month,
                project_key: None
            }
        );
        let n = serde_json::to_value(NotificationKind::UsageLimit).unwrap();
        assert_eq!(n, json!("usage_limit"));
    }

    #[test]
    fn unknown_type_is_an_error_not_a_panic() {
        assert!(serde_json::from_str::<ClientMsg>(r#"{"type":"nope"}"#).is_err());
        assert!(serde_json::from_str::<ClientMsg>("garbage").is_err());
    }
}
