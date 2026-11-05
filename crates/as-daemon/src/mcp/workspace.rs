//! The manager agent's folder: `.mcp.json` (server URL + bearer token) and `CLAUDE.md`
//! (Hydra `electron/mcp/manager-workspace.ts`). Rewritten at every daemon start because the port
//! and the token change.

use std::path::Path;

use anyhow::{Context, Result};

/// Name of the server in `.mcp.json` (the tools show up as `mcp__studio__studio_*` in Claude).
pub const SERVER_NAME: &str = "studio";

pub fn mcp_json(port: u16, token: &str) -> String {
    let config = serde_json::json!({
        "mcpServers": {
            SERVER_NAME: {
                "type": "http",
                "url": format!("http://127.0.0.1:{port}/mcp"),
                "headers": { "Authorization": format!("Bearer {token}") },
            }
        }
    });
    serde_json::to_string_pretty(&config).unwrap_or_default()
}

/// Pre-approves the `studio` server of this folder's `.mcp.json` (project-local settings).
pub const SETTINGS_LOCAL: &str = "{\n  \"enabledMcpjsonServers\": [\"studio\"]\n}\n";

pub const CLAUDE_MD: &str = "# Agent Studio Manager Agent

You are a **manager agent** with access to orchestrate other agents running inside Agent Studio.

## Available MCP Tools

| Tool | Purpose |
|------|---------|
| `studio_list_agents` | List all agents with their current status |
| `studio_create_agent` | Create a new agent in a project directory |
| `studio_send_prompt` | Send a prompt to a specific agent |
| `studio_get_output` | Get recent output from an agent's terminal buffer |
| `studio_broadcast` | Send the same prompt to all agents in a project |
| `studio_kill_agent` | Kill a running agent |
| `studio_restart_agent` | Restart an agent |

## Guidelines

- Your role is to **orchestrate** other agents, not to do implementation work yourself.
- Break complex tasks into agent-specific assignments and delegate them.
- Monitor agent outputs with `studio_get_output` and adjust strategy based on progress.
- Use `studio_broadcast` to coordinate agents working on the same project.
- You **cannot** create other manager agents — only regular working agents.
- When creating agents, always specify a real project directory path and a descriptive name.
- Agents run CLI sessions (Claude, Codex or OpenCode) with full tool access (Read, Write, Edit, Bash, etc.).
";

/// Create the folder and write both files. `.mcp.json` holds the token, so it is private (0600).
pub fn setup(dir: &Path, port: u16, token: &str) -> Result<()> {
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    write_private(&dir.join(".mcp.json"), mcp_json(port, token).as_bytes())?;
    as_core::util::write_atomic(&dir.join("CLAUDE.md"), CLAUDE_MD.as_bytes())?;
    // Without this Claude asks "New MCP server found in .mcp.json" at start, and the manager's
    // first prompt (typed by the daemon) would answer that dialog instead.
    as_core::util::write_atomic(
        &dir.join(".claude").join("settings.local.json"),
        SETTINGS_LOCAL.as_bytes(),
    )?;
    Ok(())
}

fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    let tmp = path.with_extension("json.tmp");
    {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(&tmp)
            .with_context(|| format!("writing {}", tmp.display()))?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    std::fs::rename(&tmp, path).with_context(|| format!("writing {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_the_workspace_files() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().join("manager-workspace");
        setup(&ws, 4321, "tok").unwrap();
        let json: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(ws.join(".mcp.json")).unwrap()).unwrap();
        let server = &json["mcpServers"]["studio"];
        assert_eq!(server["type"], "http");
        assert_eq!(server["url"], "http://127.0.0.1:4321/mcp");
        assert_eq!(server["headers"]["Authorization"], "Bearer tok");
        let md = std::fs::read_to_string(ws.join("CLAUDE.md")).unwrap();
        for tool in [
            "studio_list_agents",
            "studio_create_agent",
            "studio_send_prompt",
            "studio_get_output",
            "studio_broadcast",
            "studio_kill_agent",
            "studio_restart_agent",
        ] {
            assert!(md.contains(tool), "{tool}");
        }
        assert!(!md.contains("hydra"));
        let settings: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(ws.join(".claude").join("settings.local.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(settings["enabledMcpjsonServers"][0], SERVER_NAME);
        // A new start replaces the port and the token.
        setup(&ws, 99, "other").unwrap();
        let text = std::fs::read_to_string(ws.join(".mcp.json")).unwrap();
        assert!(text.contains("127.0.0.1:99/mcp") && text.contains("Bearer other"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(ws.join(".mcp.json"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }
}
