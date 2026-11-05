//! P9 requests against an in-process daemon: files, free terminals, skills, open-in, Hydra import,
//! diagnostics and git (real temp repository).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use as_core::agents::fake_pty::FakePtyBackend;
use as_core::fs::FileContent;
use as_core::paths::Paths;
use as_core::sessions::{ClaudeSessionCatalog, CodexSessionCatalog};
use as_core::skills::{SkillScanOptions, SkillToggle};
use as_core::{ManagerOptions, ProviderId};
use as_daemon::{DaemonOptions, SessionCatalogs, serve};
use as_proto::{DaemonClient, Event, Request, Response, transport};
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

struct Harness {
    dir: tempfile::TempDir,
    paths: Paths,
    backend: FakePtyBackend,
    stop: CancellationToken,
    task: JoinHandle<anyhow::Result<()>>,
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

async fn start() -> Harness {
    start_at(tempfile::tempdir().unwrap()).await
}

/// A daemon whose data directory is `dir`.
async fn start_at(dir: tempfile::TempDir) -> Harness {
    let paths = Paths::at(dir.path().join("data"));
    let backend = FakePtyBackend::new();
    let home = dir.path().join("home");
    let options = DaemonOptions {
        manager: ManagerOptions {
            backend: Arc::new(backend.clone()),
            kill_grace: Duration::from_millis(50),
            ..Default::default()
        },
        catalogs: SessionCatalogs {
            claude: Arc::new(ClaudeSessionCatalog::with_dirs(Vec::new(), 0)),
            codex: Arc::new(CodexSessionCatalog::with_dir(home.join("none"), 0)),
        },
        catalog_locator: true,
        import_sessions: false,
        preflight_on_create: false,
        apply_fixed_path: false,
        os_notifications: false,
        overview: false,
        overview_config_dirs: Some(Vec::new()),
        usage: false,
        mcp: false,
        updates: false,
        update_http: None,
        headless_override: None,
        usage_options: as_daemon::UsageOptions {
            sources: Some(Vec::new()),
            profiles: Some(Vec::new()),
            ..Default::default()
        },
        skills: Some(SkillScanOptions {
            claude_config_dirs: vec![home.join(".claude")],
            home: home.clone(),
        }),
        hydra_dir: Some(dir.path().join("Hydra")),
    };
    let stop = CancellationToken::new();
    let task = tokio::spawn(serve(paths.clone(), options, stop.clone()));
    for _ in 0..400 {
        if transport::connect(&paths).await.is_ok() {
            return Harness {
                dir,
                paths,
                backend,
                stop,
                task,
            };
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("daemon did not come up");
}

impl Harness {
    /// Stop the daemon and hand back its directory (kept alive: it holds the project folders).
    async fn stop(self, client: DaemonClient) -> tempfile::TempDir {
        drop(client);
        self.stop.cancel();
        let _ = tokio::time::timeout(Duration::from_secs(10), self.task).await;
        self.dir
    }

    async fn finish(self, client: DaemonClient) {
        drop(client);
        self.stop.cancel();
        let _ = tokio::time::timeout(Duration::from_secs(10), self.task).await;
    }
}

async fn ask(c: &DaemonClient, req: Request) -> Result<Response, String> {
    match c.request(req).await {
        Ok(r) => Ok(r),
        Err(e) => Err(format!("{e:#}")),
    }
}

async fn wait_event(
    rx: &mut UnboundedReceiver<Event>,
    what: &str,
    mut pick: impl FnMut(&Event) -> bool,
) -> Event {
    let fut = async {
        loop {
            match rx.recv().await {
                Some(e) if pick(&e) => return e,
                Some(_) => {}
                None => panic!("event stream closed waiting for {what}"),
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(5), fut)
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for {what}"))
}

fn project(h: &Harness) -> PathBuf {
    let p = h.dir.path().join("proj");
    write(&p.join("src").join("main.rs"), "fn main() {}\n");
    write(&p.join("README.md"), "hello\n");
    write(&p.join("blob.bin"), "a\0b");
    p
}

#[tokio::test]
async fn files_requests_list_read_search_and_refuse_traversal() {
    let h = start().await;
    let client = DaemonClient::connect(&h.paths).await.unwrap();
    let dir = project(&h).to_string_lossy().into_owned();
    let Ok(Response::FsEntries(entries)) = ask(
        &client,
        Request::FsReadDir {
            project_dir: dir.clone(),
            path: String::new(),
            show_hidden: false,
        },
    )
    .await
    else {
        panic!("entries")
    };
    assert_eq!(
        entries.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(),
        ["src", "blob.bin", "README.md"]
    );
    let Ok(Response::FileContent(FileContent::Text { text, .. })) = ask(
        &client,
        Request::FsReadFile {
            project_dir: dir.clone(),
            path: "src/main.rs".into(),
        },
    )
    .await
    else {
        panic!("text")
    };
    assert_eq!(text.replace("\r\n", "\n"), "fn main() {}\n");
    assert!(matches!(
        ask(
            &client,
            Request::FsReadFile {
                project_dir: dir.clone(),
                path: "blob.bin".into()
            }
        )
        .await,
        Ok(Response::FileContent(FileContent::Binary { .. }))
    ));
    let Ok(Response::SearchHits(hits)) = ask(
        &client,
        Request::FsSearch {
            project_dir: dir.clone(),
            query: "main".into(),
            limit: None,
        },
    )
    .await
    else {
        panic!("hits")
    };
    assert_eq!(hits[0].path, "src/main.rs");
    assert!(
        matches!(ask(&client, Request::FsMtime { project_dir: dir.clone(), path: "README.md".into() }).await, Ok(Response::Number(n)) if n > 0)
    );
    let err = ask(
        &client,
        Request::FsReadFile {
            project_dir: dir.clone(),
            path: "../secret".into(),
        },
    )
    .await
    .unwrap_err();
    assert!(err.contains("outside the project"), "{err}");
    h.finish(client).await;
}

#[tokio::test]
async fn write_file_request_saves_and_refuses_escapes() {
    let h = start().await;
    let client = DaemonClient::connect(&h.paths).await.unwrap();
    let root = project(&h);
    let dir = root.to_string_lossy().into_owned();
    let write_req = |path: &str, content: &str| Request::FsWriteFile {
        project_dir: dir.clone(),
        path: path.into(),
        content: content.into(),
    };
    assert!(matches!(
        ask(&client, write_req("README.md", "saved\n")).await,
        Ok(Response::Ok)
    ));
    let on_disk = std::fs::read_to_string(root.join("README.md")).unwrap();
    assert_eq!(on_disk, "saved\n");
    let err = ask(&client, write_req("../evil.txt", "x"))
        .await
        .unwrap_err();
    assert!(err.contains("outside the project"), "{err}");
    assert!(!h.dir.path().join("evil.txt").exists());
    let err = ask(&client, write_req("src", "x")).await.unwrap_err();
    assert!(err.contains("is a directory"), "{err}");
    let Ok(Response::DirSuggestions(found)) = ask(
        &client,
        Request::FsListDirs {
            query: format!("{}{}", h.dir.path().display(), std::path::MAIN_SEPARATOR),
        },
    )
    .await
    else {
        panic!("suggestions")
    };
    assert!(found.iter().any(|d| d.name == "proj"), "{found:?}");
    h.finish(client).await;
}

#[tokio::test]
async fn the_free_terminal_layout_survives_a_daemon_restart() {
    use as_core::terminal_tree::SplitDir;
    let h = start().await;
    let client = DaemonClient::connect(&h.paths).await.unwrap();
    let dir = project(&h).to_string_lossy().into_owned();
    let spawn = |group_id: Option<String>, split_pane: Option<String>, split_dir| {
        client.request(Request::SpawnTerminal {
            project_dir: dir.clone(),
            cwd: None,
            group_id,
            split_pane,
            split_dir,
            cols: 80,
            rows: 24,
        })
    };
    let Ok(Response::TerminalSpawned(a)) = spawn(None, None, None).await else {
        panic!("first shell")
    };
    let Ok(Response::TerminalSpawned(b)) = spawn(
        Some(a.group_id.clone()),
        Some(a.terminal_id.clone()),
        Some(SplitDir::Horizontal),
    )
    .await
    else {
        panic!("split right")
    };
    let Ok(Response::TerminalSpawned(c)) = spawn(
        Some(a.group_id.clone()),
        Some(b.terminal_id.clone()),
        Some(SplitDir::Vertical),
    )
    .await
    else {
        panic!("split down")
    };
    ask(
        &client,
        Request::RenameTerminal {
            terminal_id: b.terminal_id.clone(),
            label: "build".into(),
        },
    )
    .await
    .unwrap();
    let Ok(Response::TerminalLayout(before)) = ask(
        &client,
        Request::ResizeTerminalPane {
            project_dir: dir.clone(),
            pane_id: a.terminal_id.clone(),
            axis: SplitDir::Horizontal,
            delta: 20,
        },
    )
    .await
    else {
        panic!("resize")
    };
    assert_eq!(before.groups[0].pane_ids.len(), 3);
    assert_eq!(c.layout.groups[0].active_pane_id, c.terminal_id);

    // Stop the daemon: the layout is written before the shells are closed.
    let data_dir = h.paths.terminals_file();
    let home = h.stop(client).await;
    let saved = std::fs::read_to_string(&data_dir).unwrap();
    assert!(saved.contains("\"build\""), "{saved}");

    // The next daemon starts with that file in its data folder (a fresh folder rather than the
    // same one: on Windows a named pipe that was just closed can stay busy for a moment).
    // Reading the layout starts no shell...
    let next = tempfile::tempdir().unwrap();
    write(&next.path().join("data").join("terminals.json"), &saved);
    let h2 = start_at(next).await;
    let client = DaemonClient::connect(&h2.paths).await.unwrap();
    assert_eq!(h2.backend.spawn_count(), 0, "lazy");
    // ...until the project's panel asks for its layout.
    let Ok(Response::TerminalLayout(after)) = ask(
        &client,
        Request::GetTerminalLayout {
            project_dir: dir.clone(),
        },
    )
    .await
    else {
        panic!("layout")
    };
    assert_eq!(h2.backend.spawn_count(), 3);
    assert_eq!(after.groups.len(), 1);
    assert_eq!(
        after
            .panes
            .iter()
            .map(|p| p.label.as_str())
            .collect::<Vec<_>>(),
        ["Terminal 1", "build", "Terminal 3"]
    );
    let shape = |l: &as_core::terminals::FreeTerminalLayout| {
        let g = &l.groups[0];
        let ids = g.pane_ids.clone();
        let mut t = g.tree.clone().unwrap();
        t.map_panes(&mut |id| ids.iter().position(|p| p == id).unwrap().to_string());
        (
            serde_json::to_value(&t).unwrap(),
            ids.iter().position(|p| *p == g.active_pane_id),
        )
    };
    assert_eq!(shape(&after), shape(&before));
    for i in 0..3 {
        assert_eq!(
            h2.backend.request(i).map(|r| PathBuf::from(r.cwd)),
            Some(PathBuf::from(&dir)),
            "fresh shell in the same folder"
        );
    }
    assert!(
        after
            .panes
            .iter()
            .all(|p| before.panes.iter().all(|o| o.id != p.id))
    );
    h2.finish(client).await;
    drop(home);
}

#[tokio::test]
async fn free_terminals_spawn_type_output_layout_kill_and_exit() {
    let h = start().await;
    let client = DaemonClient::connect(&h.paths).await.unwrap();
    let mut events = client.take_events().unwrap();
    let dir = project(&h).to_string_lossy().into_owned();
    let Ok(Response::TerminalSpawned(t)) = ask(
        &client,
        Request::SpawnTerminal {
            project_dir: dir.clone(),
            cwd: None,
            group_id: None,
            split_pane: None,
            split_dir: None,
            cols: 90,
            rows: 20,
        },
    )
    .await
    else {
        panic!("spawned")
    };
    assert_eq!(t.layout.groups.len(), 1);
    assert_eq!(
        h.backend.request(0).map(|r| (r.cols, r.rows)),
        Some((90, 20))
    );
    wait_event(
        &mut events,
        "layout event",
        |e| matches!(e, Event::TerminalLayoutChanged { layout, .. } if layout.panes.len() == 1),
    )
    .await;

    h.backend.ptys()[0].emit_data(b"early ");
    // Subscribe once the daemon buffered the first chunk: it comes back as the buffer.
    let mut buffered = Vec::new();
    for _ in 0..200 {
        let Ok(Response::Buffer { data }) = ask(
            &client,
            Request::SubscribeTerminal {
                terminal_id: t.terminal_id.clone(),
            },
        )
        .await
        else {
            panic!("buffer")
        };
        buffered = data;
        if !buffered.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(buffered, b"early ");
    h.backend.ptys()[0].emit_data(b"live");
    let ev = wait_event(
        &mut events,
        "output",
        |e| matches!(e, Event::TerminalOutput { data, .. } if data == b"live"),
    )
    .await;
    assert!(
        matches!(ev, Event::TerminalOutput { terminal_id, .. } if terminal_id == t.terminal_id)
    );

    assert!(matches!(
        ask(
            &client,
            Request::TerminalInput {
                terminal_id: t.terminal_id.clone(),
                data: b"echo hi\r".to_vec()
            }
        )
        .await,
        Ok(Response::Flag(true))
    ));
    assert_eq!(h.backend.ptys()[0].written(), "echo hi\r");
    ask(
        &client,
        Request::TerminalResize {
            terminal_id: t.terminal_id.clone(),
            cols: 100,
            rows: 30,
        },
    )
    .await
    .unwrap();
    assert_eq!(h.backend.ptys()[0].resizes(), vec![(100, 30)]);

    // A split joins the group; renaming shows in the layout.
    let Ok(Response::TerminalSpawned(s)) = ask(
        &client,
        Request::SpawnTerminal {
            project_dir: dir.clone(),
            cwd: None,
            group_id: Some(t.group_id.clone()),
            split_pane: None,
            split_dir: None,
            cols: 40,
            rows: 10,
        },
    )
    .await
    else {
        panic!("split")
    };
    assert_eq!(s.layout.groups[0].pane_ids.len(), 2);
    assert!(matches!(
        ask(
            &client,
            Request::RenameTerminal {
                terminal_id: s.terminal_id.clone(),
                label: "build".into()
            }
        )
        .await,
        Ok(Response::Flag(true))
    ));
    let Ok(Response::TerminalLayout(l)) = ask(
        &client,
        Request::GetTerminalLayout {
            project_dir: dir.clone(),
        },
    )
    .await
    else {
        panic!("layout")
    };
    assert!(l.panes.iter().any(|p| p.label == "build"));

    // The first shell exits by itself, the second is killed.
    h.backend.ptys()[0].emit_exit(2);
    let ev = wait_event(&mut events, "exit", |e| {
        matches!(e, Event::TerminalExit { .. })
    })
    .await;
    assert!(matches!(ev, Event::TerminalExit { exit_code: 2, .. }));
    assert!(matches!(
        ask(
            &client,
            Request::KillTerminal {
                terminal_id: s.terminal_id.clone()
            }
        )
        .await,
        Ok(Response::Flag(true))
    ));
    let Ok(Response::Terminals(list)) = ask(&client, Request::ListTerminals).await else {
        panic!("list")
    };
    assert!(list.is_empty());
    assert!(
        ask(
            &client,
            Request::SubscribeTerminal {
                terminal_id: "nope".into()
            }
        )
        .await
        .is_err()
    );
    h.finish(client).await;
}

#[tokio::test]
async fn skills_scan_and_toggle_use_the_configured_locations() {
    let h = start().await;
    let client = DaemonClient::connect(&h.paths).await.unwrap();
    let home = h.dir.path().join("home");
    write(
        &home
            .join(".codex")
            .join("skills")
            .join("tdd")
            .join("SKILL.md"),
        "---\nname: tdd\n---\n",
    );
    write(
        &home
            .join(".claude")
            .join("skills")
            .join("mine")
            .join("SKILL.md"),
        "---\nname: mine\n---\n",
    );
    let Ok(Response::Skills(scan)) = ask(&client, Request::ScanSkills).await else {
        panic!("skills")
    };
    assert_eq!(scan.codex.len(), 1);
    assert_eq!(scan.claude.len(), 1);
    let toggle = SkillToggle {
        provider: ProviderId::Codex,
        enabled: false,
        group: String::new(),
        config_dir: String::new(),
        path: scan.codex[0].path.clone(),
    };
    assert!(matches!(
        ask(&client, Request::ToggleSkill(toggle)).await,
        Ok(Response::Ok)
    ));
    assert!(
        home.join(".codex")
            .join("skills")
            .join("tdd")
            .join("SKILL.md.disabled")
            .exists()
    );
    let bad = SkillToggle {
        provider: ProviderId::Codex,
        enabled: false,
        group: String::new(),
        config_dir: String::new(),
        path: h
            .dir
            .path()
            .join("x")
            .join("SKILL.md")
            .to_string_lossy()
            .into_owned(),
    };
    assert!(ask(&client, Request::ToggleSkill(bad)).await.is_err());
    h.finish(client).await;
}

#[tokio::test]
async fn open_targets_always_include_the_file_manager() {
    let h = start().await;
    let client = DaemonClient::connect(&h.paths).await.unwrap();
    let Ok(Response::OpenTargets(list)) = ask(&client, Request::ListOpenTargets).await else {
        panic!("targets")
    };
    assert!(
        list.iter()
            .any(|o| o.target == as_core::open_in::OpenTarget::Files)
    );
    h.finish(client).await;
}

#[tokio::test]
async fn hydra_import_previews_then_merges_and_never_touches_hydra() {
    let h = start().await;
    let client = DaemonClient::connect(&h.paths).await.unwrap();
    let hydra = h.dir.path().join("Hydra");
    write(
        &hydra.join("config.json"),
        r#"{"theme":"light","maxAgents":3,"notAKey":1}"#,
    );
    write(
        &hydra.join("workspace.json"),
        r#"{"schemaVersion":1,"agents":[{"id":"hx1","name":"old","projectDir":"/p","model":"sonnet","yolo":false,"sessionId":"s-9","createdAt":"2026-01-01T00:00:00Z"}]}"#,
    );
    let before = std::fs::read(hydra.join("config.json")).unwrap();
    let Ok(Response::HydraPreview(p)) =
        ask(&client, Request::PreviewHydraImport { dir: None }).await
    else {
        panic!("preview")
    };
    assert_eq!((p.agents_total, p.agents_new), (1, 1));
    assert!(p.config_changes.iter().any(|c| c.key == "theme"));
    let Ok(Response::HydraReport(r)) = ask(&client, Request::ApplyHydraImport { dir: None }).await
    else {
        panic!("report")
    };
    assert_eq!(r.agents_added.len(), 1);
    let Ok(Response::Config(c)) = ask(&client, Request::GetConfig).await else {
        panic!("config")
    };
    assert_eq!((c.max_agents, c.theme), (3, as_core::ThemeId::Light));
    let Ok(Response::Agents(agents)) = ask(&client, Request::ListAgents).await else {
        panic!("agents")
    };
    assert!(
        agents.iter().any(|a| a.id == "hx1"),
        "the imported agent shows up at once"
    );
    assert_eq!(std::fs::read(hydra.join("config.json")).unwrap(), before);
    assert!(
        ask(
            &client,
            Request::PreviewHydraImport {
                dir: Some(h.dir.path().join("nope").to_string_lossy().into_owned())
            }
        )
        .await
        .is_err()
    );
    h.finish(client).await;
}

#[tokio::test]
async fn diagnostics_bundle_has_a_snapshot_without_secrets() {
    let h = start().await;
    let client = DaemonClient::connect(&h.paths).await.unwrap();
    write(
        &h.paths.logs_dir().join("daemon.log"),
        "line one\nline two\n",
    );
    let Ok(Response::Path(dir)) = ask(&client, Request::ExportDiagnostics).await else {
        panic!("path")
    };
    let dir = PathBuf::from(dir);
    let snapshot = std::fs::read_to_string(dir.join("snapshot.json")).unwrap();
    let v: serde_json::Value = serde_json::from_str(&snapshot).unwrap();
    assert_eq!(v["version"], as_core::VERSION);
    assert!(v["config"].is_object());
    assert!(!snapshot.to_lowercase().contains("bearer"));
    assert_eq!(
        std::fs::read_to_string(dir.join("logs").join("daemon.log"))
            .unwrap()
            .replace("\r\n", "\n"),
        "line one\nline two\n"
    );
    h.finish(client).await;
}

fn git_available() -> bool {
    std::process::Command::new("git")
        .arg("--version")
        .output()
        .is_ok()
}

fn git(dir: &Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args([
            "-c",
            "user.name=T",
            "-c",
            "user.email=t@example.com",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "init.defaultBranch=main",
        ])
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env(
            "GIT_CONFIG_GLOBAL",
            if cfg!(windows) { "NUL" } else { "/dev/null" },
        )
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[tokio::test]
async fn git_requests_status_stage_commit_diff_and_branches() {
    if !git_available() {
        eprintln!("git not installed, skipping");
        return;
    }
    let h = start().await;
    let client = DaemonClient::connect(&h.paths).await.unwrap();
    let repo = h.dir.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q"]);
    // The daemon runs plain `git commit`, so the identity must live in the
    // repo's own config (CI runners have no global one).
    git(&repo, &["config", "user.name", "T"]);
    git(&repo, &["config", "user.email", "t@example.com"]);
    git(&repo, &["config", "commit.gpgsign", "false"]);
    write(&repo.join("a.txt"), "one\n");
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-q", "-m", "init"]);
    write(&repo.join("a.txt"), "one\ntwo\n");
    write(&repo.join("new file.txt"), "x\n");
    let dir = repo.to_string_lossy().into_owned();

    let Ok(Response::GitStatus(s)) = ask(&client, Request::GitStatus { dir: dir.clone() }).await
    else {
        panic!("status")
    };
    assert!(s.is_repo);
    assert_eq!(s.branch, "main");
    assert_eq!(s.files.len(), 2);
    let Ok(Response::Text(diff)) = ask(
        &client,
        Request::GitDiff {
            dir: dir.clone(),
            file: Some("a.txt".into()),
            staged: false,
        },
    )
    .await
    else {
        panic!("diff")
    };
    assert!(diff.contains("+two"));
    let Ok(Response::GitDiffStats(stats)) =
        ask(&client, Request::GitDiffStats { dir: dir.clone() }).await
    else {
        panic!("stats")
    };
    assert!(stats.additions >= 1);
    assert!(matches!(
        ask(
            &client,
            Request::GitStage {
                dir: dir.clone(),
                files: vec!["a.txt".into()]
            }
        )
        .await,
        Ok(Response::Ok)
    ));
    let Ok(Response::Text(hash)) = ask(
        &client,
        Request::GitCommit {
            dir: dir.clone(),
            message: "add two".into(),
            files: Vec::new(),
        },
    )
    .await
    else {
        panic!("commit")
    };
    assert!(!hash.trim().is_empty());
    let Ok(Response::GitLog(log)) = ask(
        &client,
        Request::GitLog {
            dir: dir.clone(),
            limit: 5,
        },
    )
    .await
    else {
        panic!("log")
    };
    assert_eq!(log[0].message, "add two");
    assert!(matches!(
        ask(
            &client,
            Request::GitCreateBranch {
                dir: dir.clone(),
                name: "feat/x".into(),
                from: None
            }
        )
        .await,
        Ok(Response::Ok)
    ));
    assert!(matches!(
        ask(
            &client,
            Request::GitCheckout {
                dir: dir.clone(),
                branch: "feat/x".into()
            }
        )
        .await,
        Ok(Response::Ok)
    ));
    let Ok(Response::GitBranches(b)) =
        ask(&client, Request::GitBranches { dir: dir.clone() }).await
    else {
        panic!("branches")
    };
    assert!(b.iter().any(|b| b.name == "feat/x" && b.is_current));
    // A folder that is not a repository is reported, not an error.
    let plain = h.dir.path().join("plain");
    std::fs::create_dir_all(&plain).unwrap();
    let Ok(Response::GitStatus(none)) = ask(
        &client,
        Request::GitStatus {
            dir: plain.to_string_lossy().into_owned(),
        },
    )
    .await
    else {
        panic!("status")
    };
    assert!(!none.is_repo);
    h.finish(client).await;
}
