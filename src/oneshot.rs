//! One-shot CLI commands: connect to the daemon (starting it when needed), do one thing, print.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use as_core::paths::Paths;
use as_core::{
    AgentState, CreateAgentPayload, ProviderId, get_default_model_for_provider,
    is_valid_model_for_provider,
};
use as_proto::DaemonClient;
use as_proto::lifecycle::{ensure_daemon, stop_daemon};

use crate::Command;

pub fn run(paths: &Paths, command: Command) -> Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("building the tokio runtime")?;
    runtime.block_on(execute(paths, command))
}

/// `agent-studio update-check`: one request to GitHub Releases, no daemon involved.
async fn update_check() -> Result<()> {
    use as_core::updates::{CheckStatus, UpdateOptions, UpdateService};
    let state = tokio::task::spawn_blocking(|| {
        let http = std::sync::Arc::new(as_core::usage::limits_client::ReqwestHttp::new());
        UpdateService::new(UpdateOptions::production(http)).check()
    })
    .await
    .context("update check task")?;
    outln!("{}", state.summary());
    if let Some(info) = state.available() {
        outln!("{}", state.hint);
        outln!("Release notes: {}", info.url);
    }
    match state.status {
        CheckStatus::Offline | CheckStatus::RateLimited | CheckStatus::Failed => {
            bail!("the update check did not complete")
        }
        _ => Ok(()),
    }
}

async fn execute(paths: &Paths, command: Command) -> Result<()> {
    if let Command::Stop { force } = command {
        return if stop_daemon(paths, force).await? {
            outln!("daemon stopped");
            Ok(())
        } else {
            outln!("daemon is not running");
            Ok(())
        };
    }
    if matches!(command, Command::UpdateCheck) {
        return update_check().await;
    }
    let client = ensure_daemon(paths).await?;
    match command {
        Command::Ls => {
            let agents = client.list_agents().await?;
            out!("{}", agents_table(&agents));
        }
        Command::New {
            project,
            provider,
            model,
            name,
            yolo,
            prompt,
            worktree,
        } => {
            let mut payload =
                new_payload(&client, &project, provider, model, name, yolo, prompt).await?;
            if worktree {
                payload.work_mode = as_core::WorkMode::Worktree;
            }
            let agent = client.create_agent(payload).await?;
            outln!("{}  {}  {}", agent.id, agent.name, agent.status_label());
        }
        Command::Send { id, text } => {
            if !client.send_input(&id, &text.join(" ")).await? {
                bail!("agent {id} not found or not running");
            }
        }
        Command::Kill { id } => found(client.kill_agent(&id).await?, &id)?,
        Command::Rm { id } => found(client.remove_agent(&id).await?, &id)?,
        Command::Restart { id } => {
            client
                .restart_agent(&id)
                .await?
                .ok_or_else(|| anyhow!("agent {id} not found"))?;
        }
        Command::Status => {
            let s = client.status().await?;
            outln!("daemon   running (pid {})", s.pid);
            outln!("version  {} (protocol {})", s.version, s.protocol);
            outln!("uptime   {}", human_duration(s.uptime_secs));
            outln!("agents   {} ({} running)", s.agent_count, s.running_count);
            outln!("clients  {}", s.clients);
            outln!("home     {}", paths.root().display());
        }
        Command::ImportHydra { dir, yes } => import_hydra(&client, dir, yes).await?,
        Command::Stop { .. } | Command::UpdateCheck | Command::Version | Command::Daemon { .. } => {
        }
    }
    Ok(())
}

/// Preview what a Hydra import would change, ask, then apply it through the daemon.
async fn import_hydra(client: &DaemonClient, dir: Option<PathBuf>, yes: bool) -> Result<()> {
    use as_proto::{Request, Response};
    let dir = dir.map(|d| d.to_string_lossy().into_owned());
    let Response::HydraPreview(p) = client
        .request(Request::PreviewHydraImport { dir: dir.clone() })
        .await?
    else {
        bail!("unexpected answer from the daemon");
    };
    outln!("Hydra data: {}", p.dir);
    outln!("agents:       {} new of {}", p.agents_new, p.agents_total);
    outln!(
        "keybindings:  {} of {} importable",
        p.keybindings_importable,
        p.keybindings_total
    );
    outln!("config values that differ: {}", p.config_changes.len());
    for c in &p.config_changes {
        outln!("  {}: {} -> {}", c.key, c.current, c.incoming);
    }
    for w in &p.warnings {
        outln!("note: {w}");
    }
    if !yes {
        use std::io::Write;
        out!("Import now? [y/N] ");
        std::io::stdout().flush()?;
        let mut answer = String::new();
        std::io::stdin().read_line(&mut answer)?;
        if !answer.trim().eq_ignore_ascii_case("y") {
            outln!("nothing imported");
            return Ok(());
        }
    }
    let Response::HydraReport(r) = client.request(Request::ApplyHydraImport { dir }).await? else {
        bail!("unexpected answer from the daemon");
    };
    outln!(
        "imported {} config value(s), {} agent(s), {} keybinding(s)",
        r.config_keys_applied,
        r.agents_added.len(),
        r.keybindings_added
    );
    for w in &r.warnings {
        outln!("note: {w}");
    }
    Ok(())
}

fn found(ok: bool, id: &str) -> Result<()> {
    if ok {
        Ok(())
    } else {
        bail!("agent {id} not found")
    }
}

async fn new_payload(
    client: &DaemonClient,
    project: &Path,
    provider: Option<String>,
    model: Option<String>,
    name: Option<String>,
    yolo: bool,
    prompt: Option<String>,
) -> Result<CreateAgentPayload> {
    let config = client.get_config().await?;
    let provider: ProviderId = match provider {
        Some(p) => serde_json::from_value(serde_json::Value::String(p.to_lowercase()))
            .map_err(|_| anyhow!("unknown provider {p} (claude, codex or opencode)"))?,
        None => config.default_provider,
    };
    let model = model.unwrap_or_else(|| {
        if is_valid_model_for_provider(provider, &config.default_model) {
            config.default_model.clone()
        } else {
            get_default_model_for_provider(provider).to_string()
        }
    });
    let project = std::fs::canonicalize(project)
        .with_context(|| format!("project directory {} not found", project.display()))?;
    Ok(CreateAgentPayload {
        name: name.unwrap_or_default(),
        project_dir: project.to_string_lossy().into_owned(),
        provider,
        model,
        yolo,
        initial_prompt: prompt.unwrap_or_default(),
        ..Default::default()
    })
}

trait StatusLabel {
    fn status_label(&self) -> String;
}
impl StatusLabel for AgentState {
    fn status_label(&self) -> String {
        serde_json::to_value(self.status)
            .ok()
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_default()
    }
}

fn agents_table(agents: &[AgentState]) -> String {
    let rows: Vec<[String; 6]> = agents
        .iter()
        .map(|a| {
            [
                a.id.clone(),
                a.name.clone(),
                a.provider.as_str().to_string(),
                a.model.clone(),
                a.status_label(),
                as_core::paths::tildify(Path::new(&a.project_dir)),
            ]
        })
        .collect();
    let header = ["ID", "NAME", "PROVIDER", "MODEL", "STATUS", "PROJECT"].map(String::from);
    let mut widths = header.each_ref().map(|h| h.chars().count());
    for row in &rows {
        for (w, cell) in widths.iter_mut().zip(row) {
            *w = (*w).max(cell.chars().count());
        }
    }
    let mut out = String::new();
    for row in std::iter::once(&header).chain(rows.iter()) {
        let line: Vec<String> = row
            .iter()
            .zip(&widths)
            .enumerate()
            .map(|(i, (cell, w))| {
                if i == row.len() - 1 {
                    cell.clone()
                } else {
                    format!("{cell:<w$}")
                }
            })
            .collect();
        out.push_str(&line.join("  "));
        out.push('\n');
    }
    out
}

fn human_duration(secs: u64) -> String {
    let (h, m, s) = (secs / 3600, secs % 3600 / 60, secs % 60);
    if h > 0 {
        format!("{h}h {m}m")
    } else if m > 0 {
        format!("{m}m {s}s")
    } else {
        format!("{s}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_aligns_columns() {
        let mut a = AgentState::default();
        a.config.id = "abc12345".into();
        a.config.name = "x".into();
        a.config.model = "opus".into();
        a.config.project_dir = "/tmp/p".into();
        let table = agents_table(&[a]);
        let lines: Vec<&str> = table.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].starts_with("ID        NAME  PROVIDER"));
        assert!(lines[1].starts_with("abc12345  x     claude"));
    }

    #[test]
    fn durations() {
        assert_eq!(human_duration(5), "5s");
        assert_eq!(human_duration(65), "1m 5s");
        assert_eq!(human_duration(7300), "2h 1m");
    }
}
