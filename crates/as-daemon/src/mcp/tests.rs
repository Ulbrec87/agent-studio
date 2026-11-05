use std::time::Duration;

use as_core::agents::fake_pty::FakePtyBackend;
use as_core::agents::{AgentManager, ManagerOptions};
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolRequestParams, CallToolResult, ClientConfig, ProtocolVersion};
use rmcp::transport::StreamableHttpClientTransport;
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use rmcp::{ClientLifecycleMode, ClientServiceExt};

use super::tools::*;
use super::*;

fn fake_manager() -> (AgentManager, FakePtyBackend) {
    let backend = FakePtyBackend::new();
    let manager = AgentManager::with_options(ManagerOptions {
        backend: Arc::new(backend.clone()),
        ..Default::default()
    });
    (manager, backend)
}

fn tools(manager: &AgentManager) -> StudioTools {
    StudioTools::new(manager.clone(), || 8)
}

fn text(result: &CallToolResult) -> String {
    serde_json::to_value(&result.content).unwrap()[0]["text"]
        .as_str()
        .unwrap()
        .to_string()
}

fn is_error(result: &CallToolResult) -> bool {
    result.is_error == Some(true)
}

fn create_params(dir: &str) -> CreateAgentParams {
    serde_json::from_value(serde_json::json!({
        "name": "worker", "projectDir": dir, "model": "sonnet", "initialPrompt": ""
    }))
    .unwrap()
}

async fn wait_for(mut cond: impl FnMut() -> bool) {
    for _ in 0..200 {
        if cond() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("condition never became true");
}

#[tokio::test]
async fn create_list_and_defaults() {
    let (manager, backend) = fake_manager();
    let t = tools(&manager);
    let dir = tempfile::tempdir().unwrap();
    let dir = dir.path().to_string_lossy().to_string();

    let listed = t.list_agents(Parameters(EmptyParams {})).await;
    assert_eq!(text(&listed).trim(), "[]");

    let created = t.create_agent(Parameters(create_params(&dir))).await;
    assert!(!is_error(&created), "{}", text(&created));
    assert!(text(&created).starts_with("Agent created successfully:"));
    assert_eq!(backend.spawn_count(), 1);
    let agent = manager.list().remove(0);
    // Hydra defaults: provider claude, no yolo, never a manager.
    assert_eq!(agent.provider, as_core::ProviderId::Claude);
    assert!(!agent.yolo && !agent.is_manager);

    let listed: serde_json::Value =
        serde_json::from_str(&text(&t.list_agents(Parameters(EmptyParams {})).await)).unwrap();
    assert_eq!(listed[0]["name"], "worker");
    assert_eq!(listed[0]["projectDir"], dir.as_str());
    assert_eq!(listed[0]["isManager"], false);
}

#[tokio::test]
async fn create_validates_its_input() {
    let (manager, backend) = fake_manager();
    let t = tools(&manager);
    let mut p = create_params("relative/path");
    let r = t.create_agent(Parameters(p)).await;
    assert!(
        is_error(&r) && text(&r).contains("absolute"),
        "{}",
        text(&r)
    );

    let missing = std::env::temp_dir().join("as-mcp-definitely-missing-dir");
    let r = t
        .create_agent(Parameters(create_params(&missing.to_string_lossy())))
        .await;
    assert!(is_error(&r) && text(&r).contains("does not exist"));

    p = create_params("/");
    p.name = String::new();
    let r = t.create_agent(Parameters(p)).await;
    assert!(is_error(&r) && text(&r).contains("name"));

    p = create_params("/");
    p.name = "x".repeat(121);
    assert!(is_error(&t.create_agent(Parameters(p)).await));
    assert_eq!(backend.spawn_count(), 0);
}

#[tokio::test]
async fn create_respects_the_agent_limit() {
    let (manager, _backend) = fake_manager();
    let t = StudioTools::new(manager.clone(), || 1);
    let dir = tempfile::tempdir().unwrap();
    let dir = dir.path().to_string_lossy().to_string();
    assert!(!is_error(
        &t.create_agent(Parameters(create_params(&dir))).await
    ));
    let r = t.create_agent(Parameters(create_params(&dir))).await;
    assert!(is_error(&r) && text(&r).contains("maximum"), "{}", text(&r));
}

#[tokio::test]
async fn prompt_broadcast_kill_restart() {
    let (manager, backend) = fake_manager();
    let t = tools(&manager);
    let dir = tempfile::tempdir().unwrap();
    let dir = dir.path().to_string_lossy().to_string();
    t.create_agent(Parameters(create_params(&dir))).await;
    let id = manager.list()[0].id.clone();

    let sent = t
        .send_prompt(Parameters(SendPromptParams {
            agent_id: id.clone(),
            prompt: "hello there".into(),
        }))
        .await;
    assert_eq!(text(&sent), format!("Prompt sent to agent {id}"));
    wait_for(|| backend.ptys()[0].written().contains("hello there")).await;

    let missing = t
        .send_prompt(Parameters(SendPromptParams {
            agent_id: "nope".into(),
            prompt: "x".into(),
        }))
        .await;
    assert!(is_error(&missing));
    let empty = t
        .send_prompt(Parameters(SendPromptParams {
            agent_id: id.clone(),
            prompt: String::new(),
        }))
        .await;
    assert!(is_error(&empty));

    let b = t
        .broadcast(Parameters(BroadcastParams {
            project_dir: dir.clone(),
            prompt: "all hands".into(),
        }))
        .await;
    assert!(
        text(&b).starts_with("Broadcast sent to 1 agent(s)"),
        "{}",
        text(&b)
    );
    let none = t
        .broadcast(Parameters(BroadcastParams {
            project_dir: "/nowhere".into(),
            prompt: "x".into(),
        }))
        .await;
    assert!(is_error(&none) && text(&none).contains("No agents found"));

    let restarted = t
        .restart_agent(Parameters(AgentIdParams {
            agent_id: id.clone(),
        }))
        .await;
    assert!(
        text(&restarted).contains("Restart count: 1"),
        "{}",
        text(&restarted)
    );
    assert!(is_error(
        &t.restart_agent(Parameters(AgentIdParams {
            agent_id: "nope".into()
        }))
        .await
    ));

    let killed = t
        .kill_agent(Parameters(AgentIdParams {
            agent_id: id.clone(),
        }))
        .await;
    assert_eq!(text(&killed), format!("Kill signal sent to agent {id}"));
    assert!(is_error(
        &t.kill_agent(Parameters(AgentIdParams {
            agent_id: "nope".into()
        }))
        .await
    ));
}

#[tokio::test]
async fn get_output_is_plain_text_with_a_line_limit() {
    let (manager, backend) = fake_manager();
    let t = tools(&manager);
    let dir = tempfile::tempdir().unwrap();
    let dir = dir.path().to_string_lossy().to_string();
    t.create_agent(Parameters(create_params(&dir))).await;
    let id = manager.list()[0].id.clone();
    let get = |lines| {
        let t = t.clone();
        let id = id.clone();
        async move {
            t.get_output(Parameters(GetOutputParams {
                agent_id: id,
                lines,
            }))
            .await
        }
    };

    let r = get(100).await;
    assert!(text(&r).contains("has no output yet"), "{}", text(&r));

    backend.ptys()[0].emit_data(b"\x1b[1;32mone\x1b[0m\r\ntwo\r\n\x1b]0;t\x07three\r\n");
    wait_for(|| manager.get_recent_output(&id, 1000).contains("three")).await;
    let r = get(2).await;
    assert!(!is_error(&r));
    assert_eq!(
        text(&r),
        format!("Output from agent {id} (last 2 of 3 lines):\n\ntwo\nthree")
    );
    assert!(!text(&get(100).await).contains('\x1b'));
    assert!(is_error(&get(0).await));
    assert!(is_error(&get(5001).await));
    let unknown = t
        .get_output(Parameters(GetOutputParams {
            agent_id: "nope".into(),
            lines: 5,
        }))
        .await;
    assert_eq!(text(&unknown), "Agent nope not found");
    assert!(is_error(&unknown));
}

#[tokio::test]
async fn get_output_caps_the_size() {
    let (manager, backend) = fake_manager();
    let t = tools(&manager);
    let dir = tempfile::tempdir().unwrap();
    t.create_agent(Parameters(create_params(&dir.path().to_string_lossy())))
        .await;
    let id = manager.list()[0].id.clone();
    let line = format!("{}\r\n", "x".repeat(900));
    for _ in 0..200 {
        backend.ptys()[0].emit_data(line.as_bytes());
    }
    wait_for(|| manager.get_recent_output(&id, 1_000_000).len() > 170_000).await;
    let r = t
        .get_output(Parameters(GetOutputParams {
            agent_id: id,
            lines: 5000,
        }))
        .await;
    let body = text(&r);
    assert!(body.contains("[... truncated ...]"));
    assert!(body.chars().count() < OUTPUT_MAX_CHARS + 200);
}

#[tokio::test]
async fn tool_list_matches_hydra() {
    let (manager, _) = fake_manager();
    let info = rmcp::ServerHandler::get_info(&tools(&manager));
    assert!(info.capabilities.tools.is_some());
}

async fn connect(
    server: &McpServer,
    token: Option<&str>,
) -> anyhow::Result<rmcp::service::RunningService<rmcp::RoleClient, ClientConfig>> {
    let mut config = StreamableHttpClientTransportConfig::with_uri(server.url());
    if let Some(token) = token {
        config = config.auth_header(token);
    }
    let client = ClientConfig::default()
        .serve_with_lifecycle(
            StreamableHttpClientTransport::from_config(config),
            ClientLifecycleMode::Auto {
                preferred_versions: vec![ProtocolVersion::V_2026_07_28],
                legacy_version: Some(ProtocolVersion::V_2025_11_25),
            },
        )
        .await?;
    Ok(client)
}

#[tokio::test]
async fn end_to_end_over_http_with_a_bearer_token() {
    let (manager, backend) = fake_manager();
    let home = tempfile::tempdir().unwrap();
    let workspace = home.path().join("manager-workspace");
    let server = McpServer::start(manager.clone(), workspace.clone(), || 8)
        .await
        .unwrap();
    assert!(server.port > 0);

    // The workspace points at the running server.
    let json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(workspace.join(".mcp.json")).unwrap())
            .unwrap();
    assert_eq!(json["mcpServers"]["studio"]["url"], server.url());
    assert_eq!(
        json["mcpServers"]["studio"]["headers"]["Authorization"],
        format!("Bearer {}", server.token)
    );

    // Without the token nothing gets through.
    let raw = reqwest_status(&server.url(), None).await;
    assert_eq!(raw, 401);
    assert_eq!(reqwest_status(&server.url(), Some("wrong")).await, 401);
    assert!(connect(&server, None).await.is_err());

    let client = connect(&server, Some(&server.token)).await.unwrap();
    let listed = client.list_tools(None).await.unwrap();
    let mut names: Vec<String> = listed.tools.iter().map(|t| t.name.to_string()).collect();
    names.sort();
    assert_eq!(
        names,
        [
            "studio_broadcast",
            "studio_create_agent",
            "studio_get_output",
            "studio_kill_agent",
            "studio_list_agents",
            "studio_restart_agent",
            "studio_send_prompt",
        ]
    );
    let create = listed
        .tools
        .iter()
        .find(|t| t.name == "studio_create_agent")
        .unwrap();
    assert!(
        create
            .description
            .as_deref()
            .unwrap()
            .contains("Agent Studio")
    );

    let work = tempfile::tempdir().unwrap();
    let args = serde_json::json!({
        "name": "via-http", "projectDir": work.path(), "model": "haiku"
    });
    let created = client
        .call_tool(
            CallToolRequestParams::new("studio_create_agent")
                .with_arguments(args.as_object().unwrap().clone()),
        )
        .await
        .unwrap();
    assert!(text(&created).starts_with("Agent created successfully"));
    assert_eq!(backend.spawn_count(), 1);

    let listed = client
        .call_tool(CallToolRequestParams::new("studio_list_agents"))
        .await
        .unwrap();
    let agents: serde_json::Value = serde_json::from_str(&text(&listed)).unwrap();
    assert_eq!(agents.as_array().unwrap().len(), 1);
    assert_eq!(agents[0]["name"], "via-http");
    assert_eq!(agents[0]["model"], "haiku");

    client.cancel().await.unwrap();
    server.stop();
}

/// Status of a bare POST (the client libraries hide 401s behind their own errors).
async fn reqwest_status(url: &str, token: Option<&str>) -> u16 {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let rest = url.strip_prefix("http://").unwrap();
    let (authority, path) = rest.split_once('/').unwrap();
    let mut stream = tokio::net::TcpStream::connect(authority).await.unwrap();
    let auth = token
        .map(|t| format!("Authorization: Bearer {t}\r\n"))
        .unwrap_or_default();
    let body = "{}";
    let request = format!(
        "POST /{path} HTTP/1.1\r\nHost: {authority}\r\n{auth}Content-Type: application/json\r\nAccept: application/json, text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).await.unwrap();
    response.split_whitespace().nth(1).unwrap().parse().unwrap()
}
