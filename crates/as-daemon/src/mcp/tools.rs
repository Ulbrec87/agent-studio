//! The `studio_*` MCP tools (Hydra `electron/mcp/McpServer.ts` `registerTools`): same names
//! (with the `studio_` prefix), parameters, limits and messages.

use as_core::agents::ManagerError;
use as_core::{AgentManager, CreateAgentPayload, ProviderId};
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock, Implementation, ServerCapabilities, ServerConfig};
use rmcp::{ServerHandler, schemars, tool, tool_handler, tool_router};
use serde::Deserialize;

use super::text::plain_lines;

/// Bytes of raw terminal output looked at for `studio_get_output`.
const RAW_OUTPUT_BYTES: usize = 1_000_000;
/// Characters returned at most by `studio_get_output` (the end of the output is kept).
pub const OUTPUT_MAX_CHARS: usize = 100_000;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct EmptyParams {}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateAgentParams {
    /// Agent name
    #[schemars(length(min = 1, max = 120))]
    pub name: String,
    /// Absolute path to the project directory
    #[schemars(length(min = 1, max = 4096))]
    pub project_dir: String,
    /// CLI provider to use (claude, codex, or opencode)
    #[serde(default)]
    pub provider: ProviderParam,
    /// Model to use (e.g. opus, sonnet, haiku, gpt-5.3-codex)
    #[schemars(length(min = 1, max = 128))]
    pub model: String,
    /// Reasoning effort level for Codex (low, medium, high, extra_high)
    #[schemars(length(max = 32))]
    pub reasoning_effort: Option<String>,
    /// Initial prompt to send after startup
    #[serde(default)]
    #[schemars(length(max = 20000))]
    pub initial_prompt: String,
    /// Skip all permission prompts
    #[serde(default)]
    pub yolo: bool,
}

#[derive(Debug, Default, Clone, Copy, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ProviderParam {
    #[default]
    Claude,
    Codex,
    Opencode,
}

impl From<ProviderParam> for ProviderId {
    fn from(p: ProviderParam) -> Self {
        match p {
            ProviderParam::Claude => ProviderId::Claude,
            ProviderParam::Codex => ProviderId::Codex,
            ProviderParam::Opencode => ProviderId::Opencode,
        }
    }
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SendPromptParams {
    /// Agent ID
    #[schemars(length(min = 1, max = 128))]
    pub agent_id: String,
    /// Prompt text to send
    #[schemars(length(min = 1, max = 20000))]
    pub prompt: String,
}

fn default_lines() -> usize {
    100
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct GetOutputParams {
    /// Agent ID
    #[schemars(length(min = 1, max = 128))]
    pub agent_id: String,
    /// Number of lines to retrieve
    #[serde(default = "default_lines")]
    #[schemars(range(min = 1, max = 5000))]
    pub lines: usize,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct BroadcastParams {
    /// Project directory path
    #[schemars(length(min = 1, max = 4096))]
    pub project_dir: String,
    /// Prompt to broadcast
    #[schemars(length(min = 1, max = 20000))]
    pub prompt: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AgentIdParams {
    /// Agent ID
    #[schemars(length(min = 1, max = 128))]
    pub agent_id: String,
}

fn ok(text: impl Into<String>) -> CallToolResult {
    CallToolResult::success(vec![ContentBlock::text(text)])
}

fn fail(text: impl Into<String>) -> CallToolResult {
    CallToolResult::error(vec![ContentBlock::text(text)])
}

fn check_len(field: &str, value: &str, min: usize, max: usize) -> Result<(), CallToolResult> {
    let n = value.chars().count();
    if n < min || n > max {
        return Err(fail(format!(
            "Invalid input: {field} must be between {min} and {max} characters"
        )));
    }
    Ok(())
}

#[derive(Clone)]
pub struct StudioTools {
    manager: AgentManager,
    /// Agents allowed at once (the config's `maxAgents`).
    max_agents: std::sync::Arc<dyn Fn() -> usize + Send + Sync>,
}

impl StudioTools {
    pub fn new(
        manager: AgentManager,
        max_agents: impl Fn() -> usize + Send + Sync + 'static,
    ) -> Self {
        Self {
            manager,
            max_agents: std::sync::Arc::new(max_agents),
        }
    }
}

#[tool_router]
impl StudioTools {
    #[tool(
        name = "studio_list_agents",
        description = "List all Agent Studio agents with their current status, model, project directory, and session info"
    )]
    pub async fn list_agents(&self, _params: Parameters<EmptyParams>) -> CallToolResult {
        let summary: Vec<_> = self
            .manager
            .list()
            .iter()
            .map(|a| {
                serde_json::json!({
                    "id": a.id,
                    "name": a.name,
                    "status": a.status,
                    "model": a.model,
                    "projectDir": a.project_dir,
                    "isManager": a.is_manager,
                    "yolo": a.yolo,
                    "sessionId": a.session_id,
                })
            })
            .collect();
        ok(serde_json::to_string_pretty(&summary).unwrap_or_default())
    }

    #[tool(
        name = "studio_create_agent",
        description = "Create a new Agent Studio agent in a project directory. The agent will start a CLI session of the chosen provider (Claude by default)."
    )]
    pub async fn create_agent(
        &self,
        Parameters(p): Parameters<CreateAgentParams>,
    ) -> CallToolResult {
        let checks = [
            check_len("name", &p.name, 1, 120),
            check_len("projectDir", &p.project_dir, 1, 4096),
            check_len("model", &p.model, 1, 128),
            check_len(
                "reasoningEffort",
                p.reasoning_effort.as_deref().unwrap_or(""),
                0,
                32,
            ),
            check_len("initialPrompt", &p.initial_prompt, 0, 20_000),
        ];
        if let Some(Err(e)) = checks.into_iter().find(Result::is_err) {
            return e;
        }
        let dir = std::path::Path::new(&p.project_dir);
        if !dir.is_absolute() {
            return fail("Failed to create agent: projectDir must be an absolute path");
        }
        if !dir.is_dir() {
            return fail(format!(
                "Failed to create agent: project directory does not exist: {}",
                p.project_dir
            ));
        }
        let max = (self.max_agents)();
        if self.manager.active_count() >= max {
            return fail(format!(
                "Failed to create agent: maximum concurrent agents ({max}) reached"
            ));
        }
        let payload = CreateAgentPayload {
            name: p.name,
            project_dir: p.project_dir,
            provider: p.provider.into(),
            model: p.model,
            reasoning_effort: p.reasoning_effort.filter(|e| !e.is_empty()),
            yolo: p.yolo,
            initial_prompt: p.initial_prompt,
            // A manager cannot create managers.
            is_manager: false,
            ..Default::default()
        };
        match self.manager.create(payload).await {
            Ok(state) => ok(format!(
                "Agent created successfully:\n{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "id": state.id,
                    "name": state.name,
                    "status": state.status,
                    "projectDir": state.project_dir,
                }))
                .unwrap_or_default()
            )),
            Err(err) => fail(format!("Failed to create agent: {}", describe(&err))),
        }
    }

    #[tool(
        name = "studio_send_prompt",
        description = "Send a prompt/message to a specific agent. The agent must be running."
    )]
    pub async fn send_prompt(&self, Parameters(p): Parameters<SendPromptParams>) -> CallToolResult {
        if let Err(e) =
            check_len("agentId", &p.agent_id, 1, 128).and(check_len("prompt", &p.prompt, 1, 20_000))
        {
            return e;
        }
        if self.manager.send_input(&p.agent_id, &p.prompt) {
            ok(format!("Prompt sent to agent {}", p.agent_id))
        } else {
            fail(format!(
                "Failed to send prompt to agent {}. Agent may not exist or not be running.",
                p.agent_id
            ))
        }
    }

    #[tool(
        name = "studio_get_output",
        description = "Get recent terminal output from an agent as plain text (escape sequences removed). Returns the last N lines of the output buffer."
    )]
    pub async fn get_output(&self, Parameters(p): Parameters<GetOutputParams>) -> CallToolResult {
        if let Err(e) = check_len("agentId", &p.agent_id, 1, 128) {
            return e;
        }
        if !(1..=5000).contains(&p.lines) {
            return fail("Invalid input: lines must be between 1 and 5000");
        }
        let Some(agent) = self.manager.get(&p.agent_id) else {
            return fail(format!("Agent {} not found", p.agent_id));
        };
        let lines = plain_lines(
            &self
                .manager
                .get_recent_output(&p.agent_id, RAW_OUTPUT_BYTES),
        );
        if lines.is_empty() {
            return ok(format!(
                "Agent {} ({}) has no output yet. Status: {}",
                p.agent_id,
                agent.name,
                status_str(agent.status)
            ));
        }
        let total = lines.len();
        let tail = &lines[total.saturating_sub(p.lines)..];
        let mut body = tail.join("\n");
        let chars = body.chars().count();
        if chars > OUTPUT_MAX_CHARS {
            body = format!(
                "[... truncated ...]\n{}",
                body.chars()
                    .skip(chars - OUTPUT_MAX_CHARS)
                    .collect::<String>()
            );
        }
        ok(format!(
            "Output from agent {} (last {} of {} lines):\n\n{}",
            p.agent_id,
            tail.len(),
            total,
            body
        ))
    }

    #[tool(
        name = "studio_broadcast",
        description = "Send the same prompt to all agents working in a specific project directory"
    )]
    pub async fn broadcast(&self, Parameters(p): Parameters<BroadcastParams>) -> CallToolResult {
        if let Err(e) = check_len("projectDir", &p.project_dir, 1, 4096)
            .and(check_len("prompt", &p.prompt, 1, 20_000))
        {
            return e;
        }
        let sent = self.manager.broadcast(&p.project_dir, &p.prompt);
        if sent.is_empty() {
            return fail(format!(
                "No agents found for project directory: {}",
                p.project_dir
            ));
        }
        ok(format!(
            "Broadcast sent to {} agent(s): {}",
            sent.len(),
            sent.join(", ")
        ))
    }

    #[tool(
        name = "studio_kill_agent",
        description = "Kill a running agent. Sends SIGTERM followed by SIGKILL after timeout."
    )]
    pub async fn kill_agent(&self, Parameters(p): Parameters<AgentIdParams>) -> CallToolResult {
        if let Err(e) = check_len("agentId", &p.agent_id, 1, 128) {
            return e;
        }
        if self.manager.kill(&p.agent_id) {
            ok(format!("Kill signal sent to agent {}", p.agent_id))
        } else {
            fail(format!("Agent {} not found", p.agent_id))
        }
    }

    #[tool(
        name = "studio_restart_agent",
        description = "Restart an agent. Kills the current process and spawns a new one, preserving the session."
    )]
    pub async fn restart_agent(&self, Parameters(p): Parameters<AgentIdParams>) -> CallToolResult {
        if let Err(e) = check_len("agentId", &p.agent_id, 1, 128) {
            return e;
        }
        match self.manager.restart(&p.agent_id) {
            Some(state) => ok(format!(
                "Agent {} restarted. Status: {}, Restart count: {}",
                p.agent_id,
                status_str(state.status),
                state.restart_count
            )),
            None => fail(format!("Agent {} not found", p.agent_id)),
        }
    }
}

fn status_str(status: as_core::AgentStatus) -> String {
    serde_json::to_value(status)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default()
}

fn describe(err: &ManagerError) -> String {
    err.to_string()
}

#[tool_handler]
impl ServerHandler for StudioTools {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("agent-studio", as_core::VERSION))
            .with_instructions(
                "Orchestrate Agent Studio agents: list, create, prompt, read output, broadcast, kill and restart.",
            )
    }
}
