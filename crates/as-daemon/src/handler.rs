//! Request -> response.

use std::sync::Arc;

use as_core::sessions::profiles;
use as_core::usage::report::{REPORT_MAX_DAYS, REPORT_MIN_DAYS};
use as_core::{AgentManager, MAX_CONCURRENT_AGENTS_HARD_LIMIT, ProviderId};
use as_proto::message::PROTOCOL_VERSION;
use as_proto::{DaemonStatus, HelloInfo, ProviderPreflight, Request, Response};

use crate::services::Daemon;

mod p9;

type Reply = Result<Response, String>;

pub async fn handle(daemon: &Arc<Daemon>, client: u64, req: Request) -> Reply {
    let m = &daemon.manager;
    match req {
        Request::Hello { client_version } => {
            tracing::debug!(client, %client_version, "hello");
            Ok(Response::Hello(HelloInfo {
                daemon_version: as_core::VERSION.to_string(),
                protocol: PROTOCOL_VERSION,
                pid: std::process::id(),
            }))
        }
        Request::Ping => Ok(Response::Pong),
        Request::ListAgents => Ok(Response::Agents(m.list())),
        Request::GetAgent { agent_id } => Ok(Response::Agent(m.get(&agent_id))),
        Request::CreateAgent(mut payload) => {
            if payload.is_manager {
                // The manager lives in its workspace (`.mcp.json` + `CLAUDE.md`) and is a Claude
                // agent (Hydra: the folder comes from the MCP status, the dialog hides resume).
                let status = daemon.mcp_status();
                let Some(workspace) = status.manager_workspace.filter(|_| status.running) else {
                    return Err(match status.error {
                        Some(e) => format!(
                            "MCP server is not running ({e}): cannot create a manager agent"
                        ),
                        None => "MCP server is not running: cannot create a manager agent".into(),
                    });
                };
                payload.project_dir = workspace;
                payload.provider = ProviderId::Claude;
                payload.resume_session_id = None;
            }
            if payload.name.trim().is_empty() {
                payload.name =
                    AgentManager::generate_name(&payload.initial_prompt, &payload.project_dir);
            }
            if daemon.preflight_on_create {
                let check = m.preflight(payload.provider).await;
                if !check.ok {
                    return Err(check.error.unwrap_or_else(|| "Preflight failed".into()));
                }
            }
            let max = daemon
                .config
                .get()
                .max_agents
                .min(MAX_CONCURRENT_AGENTS_HARD_LIMIT);
            if m.active_count() >= max {
                return Err(format!("Maximum concurrent agents ({max}) reached"));
            }
            let created = m.create(payload).await.map_err(|e| e.to_string())?;
            daemon.request_save();
            Ok(Response::Agent(Some(created)))
        }
        Request::KillAgent { agent_id } => Ok(Response::Flag(m.kill(&agent_id))),
        Request::RemoveAgent { agent_id } => {
            let removed = m.remove(&agent_id).await;
            if removed {
                daemon.registry.drop_agent(&agent_id);
                daemon.notifications.forget(&agent_id);
                daemon.request_save();
            }
            Ok(Response::Flag(removed))
        }
        Request::RestartAgent { agent_id } => Ok(Response::Agent(m.restart(&agent_id))),
        Request::SendInput { agent_id, input } => {
            Ok(Response::Flag(m.send_input(&agent_id, &input)))
        }
        Request::SendRaw { agent_id, data } => {
            Ok(Response::Flag(m.send_raw_input(&agent_id, &data)))
        }
        Request::Resize {
            agent_id,
            cols,
            rows,
        } => {
            m.resize(&agent_id, cols, rows);
            Ok(Response::Ok)
        }
        Request::Broadcast { project_dir, input } => {
            Ok(Response::Ids(m.broadcast(&project_dir, &input)))
        }
        Request::RenameAgent { agent_id, name } => {
            let agent = m.rename_agent(&agent_id, &name);
            daemon.request_save();
            Ok(Response::Agent(agent))
        }
        Request::SetModel { agent_id, model } => {
            let agent = m.set_model(&agent_id, &model);
            daemon.request_save();
            Ok(Response::Agent(agent))
        }
        Request::ToggleYolo { agent_id, yolo } => {
            let agent = m.toggle_yolo(&agent_id, yolo);
            daemon.request_save();
            Ok(Response::Agent(agent))
        }
        Request::SetGlobalYolo(yolo) => {
            let agents = m.set_global_yolo(yolo);
            daemon.request_save();
            Ok(Response::Agents(agents))
        }
        Request::Subscribe { agent_id } => {
            if m.get(&agent_id).is_none() {
                return Err(format!("agent {agent_id} not found"));
            }
            match daemon.subscribe(client, &agent_id).await {
                Some(data) => Ok(Response::Buffer { data }),
                None => Err("daemon is shutting down".into()),
            }
        }
        Request::Unsubscribe { agent_id } => {
            daemon.registry.unsubscribe(client, &agent_id);
            Ok(Response::Ok)
        }
        Request::GetConfig => Ok(Response::Config(Box::new(daemon.config.get()))),
        Request::UpdateConfig(patch) => daemon
            .config
            .update(&patch)
            .map(|c| Response::Config(Box::new(c)))
            .map_err(|e| e.to_string()),
        Request::Preflight { provider } => {
            let providers = provider.map_or_else(|| ProviderId::ALL.to_vec(), |p| vec![p]);
            let mut out = Vec::new();
            for provider in providers {
                out.push(ProviderPreflight {
                    provider,
                    result: m.preflight(provider).await,
                });
            }
            Ok(Response::Preflight(out))
        }
        Request::Status => Ok(Response::Status(DaemonStatus {
            pid: std::process::id(),
            version: as_core::VERSION.to_string(),
            protocol: PROTOCOL_VERSION,
            uptime_secs: daemon.started.elapsed().as_secs(),
            agent_count: m.list().len(),
            running_count: m.active_count(),
            clients: daemon.registry.len(),
        })),
        Request::ClientFocus(focused) => {
            daemon.registry.set_focus(client, focused);
            Ok(Response::Ok)
        }
        Request::ListSessions(options) => {
            let daemon = daemon.clone();
            let sessions =
                tokio::task::spawn_blocking(move || daemon.list_sessions_blocking(&options))
                    .await
                    .map_err(|e| format!("session scan failed: {e}"))?;
            Ok(Response::Sessions(sessions))
        }
        Request::HideSession { session_id, hidden } => {
            let session_id = session_id.trim().to_string();
            if session_id.is_empty() {
                return Err("session id is empty".into());
            }
            daemon
                .config
                .modify(|c| {
                    c.hidden_session_ids.retain(|id| *id != session_id);
                    if hidden {
                        c.hidden_session_ids.push(session_id.clone());
                    }
                })
                .map(|c| Response::Config(Box::new(c)))
                .map_err(|e| e.to_string())
        }
        Request::ListClaudeProfiles => {
            let profiles = tokio::task::spawn_blocking(profiles::list_profiles_from_env)
                .await
                .map_err(|e| e.to_string())?;
            Ok(Response::Profiles(profiles))
        }
        Request::SetProjectProfile {
            project_dir,
            config_dir,
        } => {
            let dir = project_dir.trim();
            let dir = if dir.len() > 1 {
                dir.trim_end_matches(['/', '\\'])
            } else {
                dir
            };
            if dir.is_empty() {
                return Err("project directory is empty".into());
            }
            daemon
                .config
                .modify(|c| {
                    c.claude_profile_by_project = profiles::with_profile(
                        &c.claude_profile_by_project,
                        dir,
                        config_dir.as_deref(),
                    );
                })
                .map(|c| Response::Config(Box::new(c)))
                .map_err(|e| e.to_string())
        }
        Request::ListProviderModels { provider } => {
            Ok(Response::Models(daemon.models.list(provider).await))
        }
        Request::GetOverview => {
            let daemon = daemon.clone();
            let snapshot =
                tokio::task::spawn_blocking(move || daemon.overview.lock().get_snapshot())
                    .await
                    .map_err(|e| format!("overview failed: {e}"))?;
            Ok(Response::Overview(Box::new(snapshot)))
        }
        Request::GetOverviewHistory { since_ms } => Ok(Response::OverviewHistory(Box::new(
            daemon.history_since(since_ms),
        ))),
        Request::GetOverviewWaits => Ok(Response::OverviewWaits(Box::new(
            daemon.history.lock().waits(),
        ))),
        Request::GetOverviewEvents { since_ms, limit } => {
            let mut events = daemon.history_since(Some(since_ms.unwrap_or(0))).events;
            if let Some(limit) = limit
                && events.len() > limit
            {
                events.drain(..events.len() - limit);
            }
            Ok(Response::OverviewEvents(events))
        }
        Request::GetAgentDetails { agent_id } => {
            let daemon = daemon.clone();
            let details =
                tokio::task::spawn_blocking(move || daemon.overview.lock().get_details(&agent_id))
                    .await
                    .map_err(|e| format!("agent details failed: {e}"))?;
            Ok(Response::AgentDetails(details.map(Box::new)))
        }
        Request::GetUsageState => Ok(Response::UsageState(Box::new(daemon.usage.state()))),
        Request::GetUsageSummary {
            period,
            project_key,
        } => {
            if project_key.as_ref().is_some_and(|k| k.len() > 4096) {
                return Err("project_key is too long".into());
            }
            let daemon = daemon.clone();
            let summary = tokio::task::spawn_blocking(move || {
                daemon
                    .usage
                    .summary(period, project_key.as_deref().filter(|k| !k.is_empty()))
            })
            .await
            .map_err(|e| format!("usage summary failed: {e}"))?;
            Ok(Response::UsageSummary(Box::new(summary)))
        }
        Request::GetUsageReport { provider, days } => {
            if !(REPORT_MIN_DAYS..=REPORT_MAX_DAYS).contains(&days) {
                return Err(format!(
                    "days must be an integer from {REPORT_MIN_DAYS} to {REPORT_MAX_DAYS}"
                ));
            }
            let daemon = daemon.clone();
            let report = tokio::task::spawn_blocking(move || daemon.usage.report(days, provider))
                .await
                .map_err(|e| format!("usage report failed: {e}"))?;
            Ok(Response::UsageReport(Box::new(report)))
        }
        Request::RefreshUsage => {
            let daemon = daemon.clone();
            let state = tokio::task::spawn_blocking(move || daemon.usage.refresh())
                .await
                .map_err(|e| format!("usage refresh failed: {e}"))?;
            Ok(Response::UsageState(Box::new(state)))
        }
        Request::UsageVisible(visible) => {
            daemon.registry.set_usage_visible(client, visible);
            daemon.sync_usage_visible();
            Ok(Response::Ok)
        }
        Request::GetMcpStatus => Ok(Response::McpStatus(daemon.mcp_status())),
        Request::StartHeadlessRun(mut payload) => {
            payload.prompt = payload.prompt.trim().to_string();
            payload.project_dir = payload.project_dir.trim().to_string();
            payload.model = payload.model.trim().to_string();
            if payload.prompt.is_empty() {
                return Err("prompt is empty".into());
            }
            if payload.model.is_empty() {
                return Err("model is empty".into());
            }
            if !std::path::Path::new(&payload.project_dir).is_dir() {
                return Err(format!(
                    "project directory does not exist: {}",
                    payload.project_dir
                ));
            }
            if daemon.preflight_on_create {
                let check = m.preflight(payload.provider).await;
                if !check.ok {
                    return Err(check.error.unwrap_or_else(|| "Preflight failed".into()));
                }
            }
            Ok(Response::HeadlessRun(Some(Box::new(
                daemon.headless.start(payload),
            ))))
        }
        Request::ListHeadlessRuns(options) => {
            Ok(Response::HeadlessRuns(daemon.headless.list(&options)))
        }
        Request::GetHeadlessRun { run_id } => Ok(Response::HeadlessRun(
            daemon.headless.get(&run_id).map(Box::new),
        )),
        Request::CancelHeadlessRun { run_id } => {
            Ok(Response::Flag(daemon.headless.cancel(&run_id)))
        }
        Request::GetHeadlessRunLog { run_id, options } => {
            let daemon = daemon.clone();
            let log =
                tokio::task::spawn_blocking(move || daemon.headless.get_log(&run_id, &options))
                    .await
                    .map_err(|e| format!("reading the log failed: {e}"))?;
            Ok(Response::HeadlessRunLog(log))
        }
        req @ (Request::GitStatus { .. }
        | Request::GitLog { .. }
        | Request::GitDiff { .. }
        | Request::GitDiffStats { .. }
        | Request::GitStage { .. }
        | Request::GitUnstage { .. }
        | Request::GitCommit { .. }
        | Request::GitPush { .. }
        | Request::GitBranches { .. }
        | Request::GitCheckout { .. }
        | Request::GitCreateBranch { .. }
        | Request::GitFileContents { .. }
        | Request::GitWorktreeCreate { .. }
        | Request::GitWorktreeRemove { .. }
        | Request::GitPrFetch { .. }
        | Request::GitPrFileDiff { .. }
        | Request::FsReadDir { .. }
        | Request::FsReadFile { .. }
        | Request::FsMtime { .. }
        | Request::FsWriteFile { .. }
        | Request::FsListDirs { .. }
        | Request::FsSearch { .. }
        | Request::SpawnTerminal { .. }
        | Request::TerminalInput { .. }
        | Request::TerminalResize { .. }
        | Request::KillTerminal { .. }
        | Request::RenameTerminal { .. }
        | Request::SubscribeTerminal { .. }
        | Request::UnsubscribeTerminal { .. }
        | Request::GetTerminalLayout { .. }
        | Request::ActivateTerminal { .. }
        | Request::ResizeTerminalPane { .. }
        | Request::ListTerminals
        | Request::ScanSkills
        | Request::ToggleSkill(_)
        | Request::ListOpenTargets
        | Request::OpenIn { .. }
        | Request::PreviewHydraImport { .. }
        | Request::ApplyHydraImport { .. }
        | Request::ExportDiagnostics
        | Request::GetGuiKeybindings
        | Request::CreateGuiKeybindingsFile { .. }) => p9::handle(daemon, client, req).await,
        Request::GetUpdateState => Ok(Response::UpdateState(Box::new(daemon.update_state()))),
        Request::CheckForUpdates => Ok(Response::UpdateState(Box::new(
            daemon.check_updates().await,
        ))),
        Request::Shutdown { stop_agents } => {
            if !stop_agents && m.active_count() > 0 {
                return Err(format!(
                    "{} agents are running; stop them first or force the shutdown",
                    m.active_count()
                ));
            }
            daemon.shutdown_services(stop_agents).await;
            daemon.request_exit();
            Ok(Response::Ok)
        }
    }
}
