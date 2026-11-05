//! P9 requests: files, free terminals, skills, open-in, Hydra import, diagnostics.
//! Blocking work (file system, git, process spawns) always runs under `spawn_blocking`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use as_core::fs;
use as_core::hydra_import;
use as_core::open_in;
use as_core::skills;
use as_core::terminals::SpawnRequest;
use as_proto::{Request, Response};

use crate::services::Daemon;

type Reply = Result<Response, String>;

/// Run blocking `f` off the async threads.
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Result<T, String> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| format!("task failed: {e}"))
}

/// Run a git/gh call on a blocking thread; the error text is what the user sees.
async fn git(
    f: impl FnOnce(&as_core::git::GitRunner) -> as_core::git::GitResult<Response> + Send + 'static,
) -> Reply {
    blocking(move || f(&as_core::git::GitRunner::new()))
        .await?
        .map_err(|e| e.to_string())
}

const DEFAULT_SEARCH_LIMIT: usize = 200;

pub async fn handle(daemon: &Arc<Daemon>, client: u64, req: Request) -> Reply {
    match req {
        // ── git ─────────────────────────────────────────────────────────────
        Request::GitStatus { dir } => {
            git(move |g| {
                g.status(Path::new(&dir))
                    .map(|s| Response::GitStatus(Box::new(s)))
            })
            .await
        }
        Request::GitLog { dir, limit } => {
            git(move |g| g.log(Path::new(&dir), limit).map(Response::GitLog)).await
        }
        Request::GitDiff { dir, file, staged } => {
            git(move |g| {
                g.diff(Path::new(&dir), file.as_deref(), staged)
                    .map(Response::Text)
            })
            .await
        }
        Request::GitDiffStats { dir } => {
            git(move |g| g.diff_stats(Path::new(&dir)).map(Response::GitDiffStats)).await
        }
        Request::GitStage { dir, files } => {
            git(move |g| g.stage(Path::new(&dir), &files).map(|()| Response::Ok)).await
        }
        Request::GitUnstage { dir, files } => {
            git(move |g| g.unstage(Path::new(&dir), &files).map(|()| Response::Ok)).await
        }
        Request::GitCommit {
            dir,
            message,
            files,
        } => {
            git(move |g| {
                g.commit(Path::new(&dir), &message, &files)
                    .map(Response::Text)
            })
            .await
        }
        Request::GitPush { dir } => git(move |g| g.push(Path::new(&dir)).map(Response::Text)).await,
        Request::GitBranches { dir } => {
            git(move |g| g.list_branches(Path::new(&dir)).map(Response::GitBranches)).await
        }
        Request::GitCheckout { dir, branch } => {
            git(move |g| g.checkout(Path::new(&dir), &branch).map(|()| Response::Ok)).await
        }
        Request::GitCreateBranch { dir, name, from } => {
            git(move |g| {
                g.create_branch(Path::new(&dir), &name, from.as_deref())
                    .map(|()| Response::Ok)
            })
            .await
        }
        Request::GitFileContents { dir, path, staged } => {
            git(move |g| {
                g.file_contents(Path::new(&dir), &path, staged)
                    .map(|c| Response::GitFileContents(Box::new(c)))
            })
            .await
        }
        Request::GitWorktreeCreate {
            dir,
            branch,
            start_point,
        } => {
            git(move |g| {
                g.worktree_create(Path::new(&dir), &branch, start_point.as_deref())
                    .map(|w| Response::GitWorktree(Box::new(w)))
            })
            .await
        }
        Request::GitWorktreeRemove {
            dir,
            path,
            delete_branch,
            force,
        } => {
            git(move |g| {
                g.worktree_remove(
                    Path::new(&dir),
                    Path::new(&path),
                    delete_branch.as_deref(),
                    force,
                )
                .map(|()| Response::Ok)
            })
            .await
        }
        Request::GitPrFetch { dir, pr } => {
            git(move |g| {
                g.pr_fetch(Path::new(&dir), &pr)
                    .map(|p| Response::GitPr(Box::new(p)))
            })
            .await
        }
        Request::GitPrFileDiff { dir, pr, path } => {
            git(move |g| {
                g.pr_file_diff(Path::new(&dir), &pr, &path)
                    .map(Response::Text)
            })
            .await
        }

        // ── files ───────────────────────────────────────────────────────────
        Request::FsReadDir {
            project_dir,
            path,
            show_hidden,
        } => {
            let entries =
                blocking(move || fs::list_dir(Path::new(&project_dir), &path, show_hidden)).await?;
            entries.map(Response::FsEntries).map_err(|e| e.to_string())
        }
        Request::FsReadFile { project_dir, path } => {
            let content = blocking(move || {
                fs::read_file(Path::new(&project_dir), &path, fs::DEFAULT_MAX_BYTES)
            })
            .await?;
            content
                .map(Response::FileContent)
                .map_err(|e| e.to_string())
        }
        Request::FsMtime { project_dir, path } => {
            let m = blocking(move || fs::file_mtime_ms(Path::new(&project_dir), &path)).await?;
            m.map(Response::Number).map_err(|e| e.to_string())
        }
        Request::FsWriteFile {
            project_dir,
            path,
            content,
        } => {
            let done =
                blocking(move || fs::write_file(Path::new(&project_dir), &path, &content)).await?;
            done.map(|()| Response::Ok).map_err(|e| e.to_string())
        }
        Request::FsListDirs { query } => {
            let dirs = blocking(move || fs::list_dirs(&query, &as_core::paths::home_dir())).await?;
            Ok(Response::DirSuggestions(dirs))
        }
        Request::FsSearch {
            project_dir,
            query,
            limit,
        } => {
            let limit = limit.unwrap_or(DEFAULT_SEARCH_LIMIT).clamp(1, 1000);
            let hits =
                blocking(move || fs::search_files(Path::new(&project_dir), &query, limit)).await?;
            Ok(Response::SearchHits(hits))
        }

        // ── free terminals ──────────────────────────────────────────────────
        Request::SpawnTerminal {
            project_dir,
            cwd,
            group_id,
            split_pane,
            split_dir,
            cols,
            rows,
        } => {
            let terminals = daemon.terminals.clone();
            let spawned = blocking(move || {
                terminals.spawn_with(SpawnRequest {
                    project_dir,
                    cwd,
                    group_id,
                    split_pane,
                    split_dir,
                    cols,
                    rows,
                })
            })
            .await?;
            spawned
                .map(Response::TerminalSpawned)
                .map_err(|e| format!("{e:#}"))
        }
        Request::TerminalInput { terminal_id, data } => {
            Ok(Response::Flag(daemon.terminals.input(&terminal_id, &data)))
        }
        Request::TerminalResize {
            terminal_id,
            cols,
            rows,
        } => {
            daemon.terminals.resize(&terminal_id, cols, rows);
            Ok(Response::Ok)
        }
        Request::KillTerminal { terminal_id } => {
            Ok(Response::Flag(daemon.terminals.kill(&terminal_id)))
        }
        Request::RenameTerminal { terminal_id, label } => Ok(Response::Flag(
            daemon.terminals.rename(&terminal_id, &label),
        )),
        Request::SubscribeTerminal { terminal_id } => {
            // Subscribe first, then snapshot: a chunk in the gap may repeat, never gets lost.
            daemon.registry.subscribe_terminal(client, &terminal_id);
            match daemon.terminals.buffer(&terminal_id) {
                Some(data) => Ok(Response::Buffer { data }),
                None => {
                    daemon.registry.unsubscribe_terminal(client, &terminal_id);
                    Err(format!("terminal {terminal_id} not found"))
                }
            }
        }
        Request::UnsubscribeTerminal { terminal_id } => {
            daemon.registry.unsubscribe_terminal(client, &terminal_id);
            Ok(Response::Ok)
        }
        Request::GetTerminalLayout { project_dir } => {
            // After a restart the saved shells are spawned here, on first use (they start in a
            // blocking thread: spawning a PTY can take a moment).
            let terminals = daemon.terminals.clone();
            let layout = blocking(move || terminals.restore_project(&project_dir)).await?;
            Ok(Response::TerminalLayout(layout))
        }
        Request::ResizeTerminalPane {
            project_dir,
            pane_id,
            axis,
            delta,
        } => Ok(Response::TerminalLayout(daemon.terminals.resize_pane(
            &project_dir,
            &pane_id,
            axis,
            delta,
        ))),
        Request::ActivateTerminal {
            project_dir,
            group_id,
            pane_id,
        } => Ok(Response::TerminalLayout(daemon.terminals.activate(
            &project_dir,
            group_id.as_deref(),
            pane_id.as_deref(),
        ))),
        Request::ListTerminals => Ok(Response::Terminals(daemon.terminals.list())),

        // ── skills, open-in ─────────────────────────────────────────────────
        Request::ScanSkills => {
            let opts = daemon.skills.clone();
            Ok(Response::Skills(
                blocking(move || skills::scan(&opts)).await?,
            ))
        }
        Request::ToggleSkill(toggle) => {
            let opts = daemon.skills.clone();
            blocking(move || skills::toggle(&opts, &toggle))
                .await?
                .map(|()| Response::Ok)
                .map_err(|e| e.to_string())
        }
        Request::ListOpenTargets => Ok(Response::OpenTargets(
            blocking(|| open_in::detect_installed(as_core::agents::shell::Os::current())).await?,
        )),
        Request::OpenIn { target, path } => blocking(move || open_in::open(target, &path))
            .await?
            .map(|()| Response::Ok),

        // ── Hydra import ────────────────────────────────────────────────────
        Request::PreviewHydraImport { dir } => {
            let dir = hydra_dir(daemon, dir)?;
            let (config, agents) = (daemon.config.get(), daemon.workspace.get_agents());
            let live = daemon.manager.export_workspace_agents(false);
            let known: Vec<_> = agents.into_iter().chain(live).collect();
            blocking(move || hydra_import::preview(&dir, &config, &known))
                .await?
                .map(|p| Response::HydraPreview(Box::new(p)))
                .map_err(|e| e.to_string())
        }
        Request::ApplyHydraImport { dir } => {
            let dir = hydra_dir(daemon, dir)?;
            // The workspace file mirrors the live agents, so write the current state first.
            daemon.persist(false);
            let d = daemon.clone();
            let report = blocking(move || {
                hydra_import::apply(&dir, &d.config, &d.workspace, &d.paths.keybindings_file())
            })
            .await?
            .map_err(|e| e.to_string())?;
            if !report.agents_added.is_empty() {
                daemon
                    .manager
                    .hydrate_workspace_agents(&report.agents_added);
                daemon.request_save();
            }
            Ok(Response::HydraReport(Box::new(report)))
        }

        Request::ExportDiagnostics => {
            let d = daemon.clone();
            blocking(move || export_diagnostics(&d))
                .await?
                .map(Response::Path)
        }
        _ => Err("unsupported request".into()),
    }
}

fn hydra_dir(daemon: &Daemon, given: Option<String>) -> Result<PathBuf, String> {
    given
        .map(PathBuf::from)
        .or_else(|| daemon.hydra_dir.clone())
        .or_else(hydra_import::default_hydra_dir)
        .ok_or_else(|| "cannot locate Hydra's data directory".to_string())
}

/// Keys whose values never go into a bundle.
fn is_secret_key(key: &str) -> bool {
    let k = key.to_ascii_lowercase();
    [
        "token",
        "secret",
        "password",
        "credential",
        "apikey",
        "api_key",
        "authorization",
    ]
    .iter()
    .any(|s| k.contains(s))
}

fn redact(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(map) => {
            for (k, v) in map.iter_mut() {
                if is_secret_key(k) {
                    *v = serde_json::Value::String("<redacted>".into());
                } else {
                    redact(v);
                }
            }
        }
        serde_json::Value::Array(a) => a.iter_mut().for_each(redact),
        _ => {}
    }
}

/// `<root>/diagnostics/<timestamp>/{snapshot.json, logs/*}`; returns the folder.
fn export_diagnostics(daemon: &Daemon) -> Result<String, String> {
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
    let dir = daemon.paths.root().join("diagnostics").join(stamp);
    let logs = dir.join("logs");
    std::fs::create_dir_all(&logs).map_err(|e| e.to_string())?;
    // Newest logs only, each capped at its last 1 MiB.
    let mut files: Vec<_> = std::fs::read_dir(daemon.paths.logs_dir())
        .map(|rd| rd.flatten().filter(|e| e.path().is_file()).collect())
        .unwrap_or_default();
    files.sort_by_key(|e| std::cmp::Reverse(e.metadata().and_then(|m| m.modified()).ok()));
    for entry in files.into_iter().take(5) {
        let Ok(bytes) = std::fs::read(entry.path()) else {
            continue;
        };
        let tail = &bytes[bytes.len().saturating_sub(1 << 20)..];
        let _ = std::fs::write(logs.join(entry.file_name()), tail);
    }
    let mut config = serde_json::to_value(daemon.config.get()).unwrap_or_default();
    redact(&mut config);
    let agents: Vec<_> = daemon
        .manager
        .list()
        .into_iter()
        .map(|a| serde_json::json!({"id": a.id, "provider": a.provider, "status": a.status, "model": a.model}))
        .collect();
    let mut snapshot = serde_json::json!({
        "version": as_core::VERSION,
        "os": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
        "pid": std::process::id(),
        "uptimeSecs": daemon.started.elapsed().as_secs(),
        "mcp": daemon.mcp_status().running,
        "agents": agents,
        "terminals": daemon.terminals.list().len(),
        "config": config,
    });
    redact(&mut snapshot);
    let text = serde_json::to_string_pretty(&snapshot).map_err(|e| e.to_string())?;
    std::fs::write(dir.join("snapshot.json"), text).map_err(|e| e.to_string())?;
    Ok(dir.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_secrets_recursively() {
        let mut v = serde_json::json!({"a": 1, "apiToken": "x", "n": {"password": "p", "ok": [{"secretKey": "s", "v": 2}]}});
        redact(&mut v);
        assert_eq!(v["apiToken"], "<redacted>");
        assert_eq!(v["n"]["password"], "<redacted>");
        assert_eq!(v["n"]["ok"][0]["secretKey"], "<redacted>");
        assert_eq!(v["n"]["ok"][0]["v"], 2);
        assert_eq!(v["a"], 1);
    }
}
