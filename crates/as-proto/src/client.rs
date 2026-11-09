//! [`DaemonClient`]: one connection to the daemon.
//!
//! A background task reads frames: responses complete the matching `request` future (by id),
//! events go to the receiver returned by [`DaemonClient::take_events`]. When the connection
//! drops, pending requests fail with "daemon connection closed" and the event receiver yields
//! `None`; build a new client with [`crate::lifecycle::ensure_daemon`] to reconnect (and
//! re-`Subscribe`: subscriptions belong to the connection).

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use anyhow::{Result, anyhow, bail};
use as_core::headless::{
    HeadlessRun, HeadlessRunLog, HeadlessRunLogOptions, ListHeadlessRunsOptions,
    StartHeadlessRunPayload,
};
use as_core::overview::{
    AgentDetails, HistoryResponse, OverviewEvent, OverviewSnapshot, WaitLedgerData,
};
use as_core::paths::Paths;
use as_core::sessions::{ClaudeProfile, ListSessionsOptions, SessionSummary};
use as_core::usage::report::{UsageReport, UsageReportProvider};
use as_core::usage::{UsagePeriod, UsageState, UsageSummary};
use as_core::{AppConfig, CreateAgentPayload, ModelId, ProviderId, ProviderModelOption};
use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use tokio::sync::{mpsc, oneshot};

use crate::codec;
use crate::message::*;

/// Default time to wait for a response.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

type Pending = StdMutex<HashMap<u64, oneshot::Sender<Result<Response, String>>>>;

struct Shared {
    next_id: AtomicU64,
    pending: Pending,
    closed: AtomicBool,
}

pub struct DaemonClient {
    out: mpsc::UnboundedSender<ClientMsg>,
    shared: Arc<Shared>,
    events: StdMutex<Option<mpsc::UnboundedReceiver<Event>>>,
}

impl DaemonClient {
    /// Connect to the daemon socket of `paths` (no handshake, no spawning:
    /// see [`crate::lifecycle::ensure_daemon`]).
    pub async fn connect(paths: &Paths) -> Result<Self> {
        let conn = crate::transport::connect(paths).await?;
        Ok(Self::from_stream(conn))
    }

    /// Wrap an already-open stream (spawns the reader/writer tasks on the current runtime).
    pub fn from_stream<T>(io: T) -> Self
    where
        T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Send + 'static,
    {
        let (mut sink, mut stream) = codec::framed(io).split();
        let (out, mut out_rx) = mpsc::unbounded_channel::<ClientMsg>();
        let (event_tx, event_rx) = mpsc::unbounded_channel::<Event>();
        let shared = Arc::new(Shared {
            next_id: AtomicU64::new(1),
            pending: StdMutex::new(HashMap::new()),
            closed: AtomicBool::new(false),
        });

        tokio::spawn(async move {
            while let Some(msg) = out_rx.recv().await {
                let Ok(frame) = codec::encode(&msg) else {
                    continue;
                };
                if sink.send(frame).await.is_err() {
                    break;
                }
            }
        });

        let reader_shared = shared.clone();
        tokio::spawn(async move {
            while let Some(Ok(frame)) = stream.next().await {
                match codec::decode::<ServerMsg>(&frame) {
                    Ok(ServerMsg::Response { id, result }) => {
                        let waiter = reader_shared
                            .pending
                            .lock()
                            .ok()
                            .and_then(|mut p| p.remove(&id));
                        if let Some(tx) = waiter {
                            let _ = tx.send(result);
                        }
                    }
                    Ok(ServerMsg::Event(event)) => {
                        let _ = event_tx.send(event);
                    }
                    Err(err) => tracing::warn!(%err, "undecodable frame from daemon"),
                }
            }
            reader_shared.closed.store(true, Ordering::SeqCst);
            // Dropping the senders wakes every waiter with a "closed" error.
            if let Ok(mut p) = reader_shared.pending.lock() {
                p.clear();
            }
        });

        Self {
            out,
            shared,
            events: StdMutex::new(Some(event_rx)),
        }
    }

    /// The event stream; can be taken once. `recv()` returning `None` means the connection closed.
    /// The channel is unbounded: drain it promptly.
    pub fn take_events(&self) -> Option<mpsc::UnboundedReceiver<Event>> {
        self.events.lock().ok()?.take()
    }

    /// False once the daemon closed the connection.
    pub fn is_connected(&self) -> bool {
        !self.shared.closed.load(Ordering::SeqCst)
    }

    pub async fn request(&self, req: Request) -> Result<Response> {
        self.request_timeout(req, REQUEST_TIMEOUT).await
    }

    pub async fn request_timeout(&self, req: Request, timeout: Duration) -> Result<Response> {
        if !self.is_connected() {
            bail!("daemon connection closed");
        }
        let id = self.shared.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel();
        self.shared
            .pending
            .lock()
            .map_err(|_| anyhow!("client state poisoned"))?
            .insert(id, tx);
        if self.out.send(ClientMsg::Request { id, req }).is_err() {
            self.forget(id);
            bail!("daemon connection closed");
        }
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(result)) => result.map_err(|e| anyhow!(e)),
            Ok(Err(_)) => bail!("daemon connection closed"),
            Err(_) => {
                self.forget(id);
                bail!("daemon request timed out")
            }
        }
    }

    fn forget(&self, id: u64) {
        if let Ok(mut p) = self.shared.pending.lock() {
            p.remove(&id);
        }
    }

    async fn expect_ok(&self, req: Request) -> Result<()> {
        match self.request(req).await? {
            Response::Ok => Ok(()),
            other => unexpected(other),
        }
    }

    async fn expect_flag(&self, req: Request) -> Result<bool> {
        match self.request(req).await? {
            Response::Flag(v) => Ok(v),
            other => unexpected(other),
        }
    }

    async fn expect_agent(&self, req: Request) -> Result<Option<AgentState>> {
        match self.request(req).await? {
            Response::Agent(a) => Ok(a),
            other => unexpected(other),
        }
    }

    // ── typed helpers ───────────────────────────────────────────────────────

    pub async fn hello(&self) -> Result<HelloInfo> {
        match self
            .request(Request::Hello {
                client_version: as_core::VERSION.to_string(),
            })
            .await?
        {
            Response::Hello(info) => Ok(info),
            other => unexpected(other),
        }
    }

    pub async fn ping(&self) -> Result<()> {
        match self.request(Request::Ping).await? {
            Response::Pong => Ok(()),
            other => unexpected(other),
        }
    }

    pub async fn list_agents(&self) -> Result<Vec<AgentState>> {
        match self.request(Request::ListAgents).await? {
            Response::Agents(a) => Ok(a),
            other => unexpected(other),
        }
    }

    pub async fn get_agent(&self, agent_id: &str) -> Result<Option<AgentState>> {
        self.expect_agent(Request::GetAgent {
            agent_id: agent_id.into(),
        })
        .await
    }

    pub async fn create_agent(&self, payload: CreateAgentPayload) -> Result<AgentState> {
        self.expect_agent(Request::CreateAgent(payload))
            .await?
            .ok_or_else(|| anyhow!("daemon returned no agent"))
    }

    /// False when the agent is unknown.
    pub async fn kill_agent(&self, agent_id: &str) -> Result<bool> {
        self.expect_flag(Request::KillAgent {
            agent_id: agent_id.into(),
        })
        .await
    }

    pub async fn restart_agent(&self, agent_id: &str) -> Result<Option<AgentState>> {
        self.expect_agent(Request::RestartAgent {
            agent_id: agent_id.into(),
        })
        .await
    }

    pub async fn send_input(&self, agent_id: &str, input: &str) -> Result<bool> {
        self.expect_flag(Request::SendInput {
            agent_id: agent_id.into(),
            input: input.into(),
        })
        .await
    }

    pub async fn send_raw(&self, agent_id: &str, data: Vec<u8>) -> Result<bool> {
        self.expect_flag(Request::SendRaw {
            agent_id: agent_id.into(),
            data,
        })
        .await
    }

    pub async fn resize(&self, agent_id: &str, cols: u16, rows: u16) -> Result<()> {
        self.expect_ok(Request::Resize {
            agent_id: agent_id.into(),
            cols,
            rows,
        })
        .await
    }

    /// Ids of the agents that received the input.
    pub async fn broadcast(&self, project_dir: &str, input: &str) -> Result<Vec<String>> {
        match self
            .request(Request::Broadcast {
                project_dir: project_dir.into(),
                input: input.into(),
            })
            .await?
        {
            Response::Ids(ids) => Ok(ids),
            other => unexpected(other),
        }
    }

    pub async fn rename_agent(&self, agent_id: &str, name: &str) -> Result<Option<AgentState>> {
        self.expect_agent(Request::RenameAgent {
            agent_id: agent_id.into(),
            name: name.into(),
        })
        .await
    }

    pub async fn set_model(&self, agent_id: &str, model: ModelId) -> Result<Option<AgentState>> {
        self.expect_agent(Request::SetModel {
            agent_id: agent_id.into(),
            model,
        })
        .await
    }

    pub async fn toggle_yolo(&self, agent_id: &str, yolo: bool) -> Result<Option<AgentState>> {
        self.expect_agent(Request::ToggleYolo {
            agent_id: agent_id.into(),
            yolo,
        })
        .await
    }

    pub async fn set_global_yolo(&self, yolo: bool) -> Result<Vec<AgentState>> {
        match self.request(Request::SetGlobalYolo(yolo)).await? {
            Response::Agents(a) => Ok(a),
            other => unexpected(other),
        }
    }

    /// Buffered output so far; live chunks then arrive as `Event::AgentOutput`.
    pub async fn subscribe(&self, agent_id: &str) -> Result<Vec<u8>> {
        match self
            .request(Request::Subscribe {
                agent_id: agent_id.into(),
            })
            .await?
        {
            Response::Buffer { data } => Ok(data),
            other => unexpected(other),
        }
    }

    pub async fn unsubscribe(&self, agent_id: &str) -> Result<()> {
        self.expect_ok(Request::Unsubscribe {
            agent_id: agent_id.into(),
        })
        .await
    }

    pub async fn update_config(&self, patch: Value) -> Result<AppConfig> {
        match self.request(Request::UpdateConfig(patch)).await? {
            Response::Config(c) => Ok(*c),
            other => unexpected(other),
        }
    }

    pub async fn preflight(&self, provider: Option<ProviderId>) -> Result<Vec<ProviderPreflight>> {
        match self.request(Request::Preflight { provider }).await? {
            Response::Preflight(r) => Ok(r),
            other => unexpected(other),
        }
    }

    pub async fn status(&self) -> Result<DaemonStatus> {
        match self.request(Request::Status).await? {
            Response::Status(s) => Ok(s),
            other => unexpected(other),
        }
    }

    pub async fn list_sessions(&self, options: ListSessionsOptions) -> Result<Vec<SessionSummary>> {
        match self.request(Request::ListSessions(options)).await? {
            Response::Sessions(s) => Ok(s),
            other => unexpected(other),
        }
    }

    /// Returns the updated config.
    pub async fn hide_session(&self, session_id: &str, hidden: bool) -> Result<AppConfig> {
        match self
            .request(Request::HideSession {
                session_id: session_id.into(),
                hidden,
            })
            .await?
        {
            Response::Config(c) => Ok(*c),
            other => unexpected(other),
        }
    }

    pub async fn list_claude_profiles(&self) -> Result<Vec<ClaudeProfile>> {
        match self.request(Request::ListClaudeProfiles).await? {
            Response::Profiles(p) => Ok(p),
            other => unexpected(other),
        }
    }

    /// Returns the updated config.
    pub async fn set_project_profile(
        &self,
        project_dir: &str,
        config_dir: Option<String>,
    ) -> Result<AppConfig> {
        match self
            .request(Request::SetProjectProfile {
                project_dir: project_dir.into(),
                config_dir,
            })
            .await?
        {
            Response::Config(c) => Ok(*c),
            other => unexpected(other),
        }
    }

    pub async fn list_provider_models(
        &self,
        provider: ProviderId,
    ) -> Result<Vec<ProviderModelOption>> {
        match self
            .request(Request::ListProviderModels { provider })
            .await?
        {
            Response::Models(m) => Ok(m),
            other => unexpected(other),
        }
    }

    pub async fn get_overview(&self) -> Result<OverviewSnapshot> {
        match self.request(Request::GetOverview).await? {
            Response::Overview(o) => Ok(*o),
            other => unexpected(other),
        }
    }

    /// History from `since_ms` (epoch ms) on; `None` = the daemon's default (last 6 h).
    pub async fn get_overview_history(&self, since_ms: Option<i64>) -> Result<HistoryResponse> {
        match self
            .request(Request::GetOverviewHistory { since_ms })
            .await?
        {
            Response::OverviewHistory(h) => Ok(*h),
            other => unexpected(other),
        }
    }

    pub async fn get_overview_waits(&self) -> Result<WaitLedgerData> {
        match self.request(Request::GetOverviewWaits).await? {
            Response::OverviewWaits(w) => Ok(*w),
            other => unexpected(other),
        }
    }

    pub async fn get_overview_events(
        &self,
        since_ms: Option<i64>,
        limit: Option<usize>,
    ) -> Result<Vec<OverviewEvent>> {
        match self
            .request(Request::GetOverviewEvents { since_ms, limit })
            .await?
        {
            Response::OverviewEvents(e) => Ok(e),
            other => unexpected(other),
        }
    }

    /// `None` when the daemon does not know the agent.
    pub async fn get_agent_details(&self, agent_id: &str) -> Result<Option<AgentDetails>> {
        match self
            .request(Request::GetAgentDetails {
                agent_id: agent_id.into(),
            })
            .await?
        {
            Response::AgentDetails(d) => Ok(d.map(|d| *d)),
            other => unexpected(other),
        }
    }

    pub async fn get_usage_state(&self) -> Result<UsageState> {
        match self.request(Request::GetUsageState).await? {
            Response::UsageState(s) => Ok(*s),
            other => unexpected(other),
        }
    }

    pub async fn get_usage_summary(
        &self,
        period: UsagePeriod,
        project_key: Option<String>,
    ) -> Result<UsageSummary> {
        match self
            .request(Request::GetUsageSummary {
                period,
                project_key,
            })
            .await?
        {
            Response::UsageSummary(s) => Ok(*s),
            other => unexpected(other),
        }
    }

    pub async fn get_usage_report(
        &self,
        provider: UsageReportProvider,
        days: u32,
    ) -> Result<UsageReport> {
        match self
            .request(Request::GetUsageReport { provider, days })
            .await?
        {
            Response::UsageReport(r) => Ok(*r),
            other => unexpected(other),
        }
    }

    /// Scan the logs and poll the limits now; can take a few seconds.
    pub async fn refresh_usage(&self) -> Result<UsageState> {
        match self
            .request_timeout(Request::RefreshUsage, Duration::from_secs(60))
            .await?
        {
            Response::UsageState(s) => Ok(*s),
            other => unexpected(other),
        }
    }

    /// Tell the daemon this client shows a usage surface (faster limits polling).
    pub async fn set_usage_visible(&self, visible: bool) -> Result<()> {
        self.expect_ok(Request::UsageVisible(visible)).await
    }

    pub async fn get_mcp_status(&self) -> Result<McpStatus> {
        match self.request(Request::GetMcpStatus).await? {
            Response::McpStatus(s) => Ok(s),
            other => unexpected(other),
        }
    }

    pub async fn start_headless_run(
        &self,
        payload: StartHeadlessRunPayload,
    ) -> Result<HeadlessRun> {
        match self.request(Request::StartHeadlessRun(payload)).await? {
            Response::HeadlessRun(Some(run)) => Ok(*run),
            other => unexpected(other),
        }
    }

    pub async fn list_headless_runs(
        &self,
        options: ListHeadlessRunsOptions,
    ) -> Result<Vec<HeadlessRun>> {
        match self.request(Request::ListHeadlessRuns(options)).await? {
            Response::HeadlessRuns(runs) => Ok(runs),
            other => unexpected(other),
        }
    }

    pub async fn get_headless_run(&self, run_id: &str) -> Result<Option<HeadlessRun>> {
        match self
            .request(Request::GetHeadlessRun {
                run_id: run_id.into(),
            })
            .await?
        {
            Response::HeadlessRun(run) => Ok(run.map(|r| *r)),
            other => unexpected(other),
        }
    }

    pub async fn cancel_headless_run(&self, run_id: &str) -> Result<bool> {
        match self
            .request(Request::CancelHeadlessRun {
                run_id: run_id.into(),
            })
            .await?
        {
            Response::Flag(f) => Ok(f),
            other => unexpected(other),
        }
    }

    pub async fn get_headless_run_log(
        &self,
        run_id: &str,
        options: HeadlessRunLogOptions,
    ) -> Result<Option<HeadlessRunLog>> {
        match self
            .request(Request::GetHeadlessRunLog {
                run_id: run_id.into(),
                options,
            })
            .await?
        {
            Response::HeadlessRunLog(log) => Ok(log),
            other => unexpected(other),
        }
    }

    /// Report whether this client's terminal is focused (drives OS notifications).
    pub async fn set_focus(&self, focused: bool) -> Result<()> {
        self.expect_ok(Request::ClientFocus(focused)).await
    }

    /// Ask the daemon to exit. Waits for the agents to stop when `stop_agents` is set.
    pub async fn shutdown(&self, stop_agents: bool) -> Result<()> {
        match self
            .request_timeout(Request::Shutdown { stop_agents }, Duration::from_secs(60))
            .await
        {
            Ok(Response::Ok) => Ok(()),
            Ok(other) => unexpected(other),
            Err(err) => Err(err),
        }
    }
}

fn unexpected<T>(response: Response) -> Result<T> {
    Err(anyhow!("unexpected daemon response: {response:?}"))
}
