//! P9 panels: app-level flows and render snapshots (fixed fake paths, no daemon).

use std::path::PathBuf;

use as_core::fs::{FileContent, FsEntry, SearchHit};
use as_core::git::{
    GitBranch, GitDiffStats, GitFileStatus, GitPrDiff, GitPrFile, GitPrMetadata, GitStatus,
    GitStatusKind,
};
use as_core::hydra_import::ImportPreview;
use as_core::open_in::{OpenOption, OpenTarget};
use as_core::terminals::{FreeTerminalGroup, FreeTerminalLayout, FreeTerminalPane};
use as_core::{AgentState, AgentStatus, AppConfig, ProviderId};
use as_proto::{Event, Request, Response};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::overview_tests::{ch, draw, press, text};
use crate::app::{App, Effect, Modal, Msg};
use crate::keymap::Command;
use crate::theme::ColorMode;

const W: u16 = 110;
const H: u16 = 34;

fn app() -> App {
    let mut a = AgentState::default();
    a.config.id = "a1".into();
    a.config.name = "api".into();
    a.config.project_dir = "/work/shop".into();
    a.config.provider = ProviderId::Claude;
    a.config.model = "sonnet".into();
    a.config.created_at = "2026-10-01T00:00:00Z".into();
    a.status = AgentStatus::Running;
    let mut app = App::new(
        AppConfig::default(),
        vec![a],
        ColorMode::TrueColor,
        PathBuf::from("/work/shop"),
    );
    app.on_resize(W, H);
    app.start();
    app
}

fn reply(app: &mut App, key: &str, r: Response) {
    app.handle_msg(Msg::Reply {
        key: key.into(),
        result: Ok(r),
    });
}

fn file(
    path: &str,
    index: Option<GitStatusKind>,
    worktree: Option<GitStatusKind>,
    untracked: bool,
) -> GitFileStatus {
    GitFileStatus {
        path: path.into(),
        orig_path: None,
        kind: if untracked {
            GitStatusKind::Untracked
        } else {
            index.or(worktree).unwrap_or(GitStatusKind::Modified)
        },
        index,
        worktree,
        staged: index.is_some(),
        unstaged: worktree.is_some(),
        untracked,
        conflicted: false,
    }
}

const DIFF: &str = "diff --git a/src/cart.rs b/src/cart.rs\n--- a/src/cart.rs\n+++ b/src/cart.rs\n@@ -1,4 +1,5 @@ impl Cart\n fn total(&self) -> u32 {\n-    self.items.len() as u32\n+    self.items.iter().map(|i| i.qty).sum()\n }\n+// end\n \n@@ -20,3 +21,3 @@ fn other\n a\n-b\n+c\n";

fn git_app() -> App {
    let mut app = app();
    app.exec(Command::GitPanel);
    reply(
        &mut app,
        "git:status",
        Response::GitStatus(Box::new(GitStatus {
            is_repo: true,
            branch: "feat/cart".into(),
            upstream: Some("origin/feat/cart".into()),
            ahead: 2,
            behind: 1,
            files: vec![
                file("src/cart.rs", None, Some(GitStatusKind::Modified), false),
                file("src/new file.rs", Some(GitStatusKind::Added), None, false),
                file("notes.txt", None, None, true),
            ],
            ..Default::default()
        })),
    );
    reply(
        &mut app,
        "git:stats",
        Response::GitDiffStats(GitDiffStats {
            files: 3,
            additions: 12,
            deletions: 3,
        }),
    );
    let key = format!("git:diff:{}", 1);
    reply(&mut app, &key, Response::Text(DIFF.into()));
    app
}

#[test]
fn prefix_g_opens_the_git_panel_and_asks_for_the_status() {
    let mut app = app();
    ch(&mut app, 'a'); // not a prefix: plain typing goes to the agent
    let fx = {
        app.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL));
        ch(&mut app, 'G')
    };
    assert!(matches!(app.modal, Some(Modal::Git)));
    assert!(fx.iter().any(
        |e| matches!(e, Effect::Req { req: Request::GitStatus { dir }, .. } if dir == "/work/shop")
    ));
    assert!(fx.iter().any(|e| matches!(
        e,
        Effect::Req {
            req: Request::GitDiffStats { .. },
            ..
        }
    )));
    press(&mut app, KeyCode::Esc);
    assert!(app.modal.is_none());
}

#[test]
fn git_changes_tab_snapshot() {
    let mut app = git_app();
    let t = draw(&mut app, W, H);
    insta::assert_snapshot!(text(&t));
}

#[test]
fn git_diff_focus_scrolls_and_jumps_hunks() {
    let mut app = git_app();
    app.on_resize(W, 14);
    press(&mut app, KeyCode::Enter);
    assert!(app.git.diff_focus);
    draw(&mut app, W, 14); // sets the diff height
    let (second, max) = (
        app.git.diff.hunks[1],
        app.git.diff.max_scroll(app.git.diff_height),
    );
    ch(&mut app, 'n');
    assert_eq!(app.git.diff.scroll, second.min(max));
    ch(&mut app, 'N');
    assert_eq!(app.git.diff.scroll, 0);
    ch(&mut app, 'j');
    assert_eq!(app.git.diff.scroll, 1.min(max));
    press(&mut app, KeyCode::Esc);
    assert!(!app.git.diff_focus);
    assert!(app.modal.is_some(), "Esc leaves the diff first");
}

#[test]
fn git_branches_tab_snapshot() {
    let mut app = git_app();
    ch(&mut app, '2');
    let b = |n: &str, cur: bool, remote: bool, a: u32, bh: u32| GitBranch {
        name: n.into(),
        is_current: cur,
        is_remote: remote,
        upstream: None,
        ahead_of_upstream: a,
        behind_upstream: bh,
    };
    reply(
        &mut app,
        "git:branches",
        Response::GitBranches(vec![
            b("main", false, false, 0, 0),
            b("feat/cart", true, false, 2, 1),
            b("origin/main", false, true, 0, 0),
        ]),
    );
    let t = draw(&mut app, W, H);
    insta::assert_snapshot!(text(&t));
}

#[test]
fn git_pr_tab_snapshot_and_gh_error() {
    let mut app = git_app();
    ch(&mut app, '3');
    ch(&mut app, '/');
    for c in "17".chars() {
        ch(&mut app, c);
    }
    let fx = press(&mut app, KeyCode::Enter);
    assert!(fx.iter().any(
        |e| matches!(e, Effect::Req { req: Request::GitPrFetch { pr, .. }, .. } if pr == "17")
    ));
    app.handle_msg(Msg::Reply {
        key: "git:pr".into(),
        result: Err("the GitHub CLI (gh) is not installed or not on PATH".into()),
    });
    let t = draw(&mut app, W, H);
    assert!(text(&t).contains("gh) is not installed"));
    let meta = GitPrMetadata {
        number: 17,
        title: "Fix cart total".into(),
        author: "ana".into(),
        state: "OPEN".into(),
        base_ref: "main".into(),
        head_ref: "fix/cart".into(),
        body: String::new(),
        url: String::new(),
        additions: 3,
        deletions: 1,
        changed_files: 1,
        created_at: String::new(),
        updated_at: String::new(),
    };
    reply(
        &mut app,
        "git:pr",
        Response::GitPr(Box::new(GitPrDiff {
            metadata: meta,
            files: vec![GitPrFile {
                path: "src/cart.rs".into(),
                status: "modified".into(),
                additions: 3,
                deletions: 1,
                patch: DIFF.into(),
            }],
        })),
    );
    let t = draw(&mut app, W, H);
    insta::assert_snapshot!(text(&t));
}

#[test]
fn git_not_a_repo_message() {
    let mut app = app();
    app.exec(Command::GitPanel);
    reply(&mut app, "git:status", Response::GitStatus(Box::default()));
    let t = draw(&mut app, W, H);
    assert!(text(&t).contains("not a git repository"));
}

fn entry(path: &str, dir: bool) -> FsEntry {
    FsEntry {
        name: path.rsplit('/').next().unwrap_or(path).into(),
        path: path.into(),
        is_dir: dir,
        is_symlink: false,
        size: 10,
    }
}

fn files_app() -> App {
    let mut app = app();
    app.exec(Command::ToggleFiles);
    reply(
        &mut app,
        "fs:dir:",
        Response::FsEntries(vec![
            entry("src", true),
            entry("Cargo.toml", false),
            entry("README.md", false),
        ]),
    );
    press(&mut app, KeyCode::Enter); // expand src
    reply(
        &mut app,
        "fs:dir:src",
        Response::FsEntries(vec![
            entry("src/main.rs", false),
            entry("src/cart.rs", false),
        ]),
    );
    press(&mut app, KeyCode::Down);
    let fx = press(&mut app, KeyCode::Enter); // open src/main.rs
    let key = fx
        .iter()
        .find_map(|e| match e {
            Effect::Req {
                key,
                req: Request::FsReadFile { .. },
            } => Some(key.clone()),
            _ => None,
        })
        .expect("a read request");
    reply(
        &mut app,
        &key,
        Response::FileContent(FileContent::Text {
            text: "fn main() {\n\tprintln!(\"hi\");\n}\n".into(),
            truncated: false,
            size: 30,
            mtime_ms: 5,
        }),
    );
    app
}

#[test]
fn files_panel_snapshot() {
    let mut app = files_app();
    let t = draw(&mut app, W, H);
    insta::assert_snapshot!(text(&t));
}

#[test]
fn file_search_popup_snapshot() {
    let mut app = files_app();
    app.exec(Command::FileSearch);
    assert!(app.files.search.active);
    let fx = ch(&mut app, 'c');
    let key = match &fx[..] {
        [Effect::Req { key, .. }] => key.clone(),
        other => panic!("{other:?}"),
    };
    reply(
        &mut app,
        &key,
        Response::SearchHits(vec![
            SearchHit {
                path: "src/cart.rs".into(),
                name: "cart.rs".into(),
                score: 90,
            },
            SearchHit {
                path: "Cargo.toml".into(),
                name: "Cargo.toml".into(),
                score: 50,
            },
        ]),
    );
    let t = draw(&mut app, W, H);
    insta::assert_snapshot!(text(&t));
    let fx = press(&mut app, KeyCode::Enter);
    assert!(fx.iter().any(|e| matches!(e, Effect::Req { req: Request::FsReadFile { path, .. }, .. } if path == "src/cart.rs")));
    assert!(!app.files.search.active);
}

#[test]
fn files_keys_open_the_editor_copy_the_path_and_open_in() {
    let mut app = files_app();
    let fx = ch(&mut app, 'E');
    assert!(
        matches!(&fx[..], [Effect::Suspend(c)] if c.args.last().is_some_and(|a| a.ends_with("main.rs")))
    );
    let fx = ch(&mut app, 'y');
    assert!(matches!(&fx[..], [Effect::Clipboard(p)] if p.ends_with("main.rs")));
    let fx = ch(&mut app, 'o');
    assert!(matches!(app.modal, Some(Modal::OpenIn)));
    assert!(fx.iter().any(|e| matches!(
        e,
        Effect::Req {
            req: Request::ListOpenTargets,
            ..
        }
    )));
}

#[test]
fn open_in_picker_snapshot() {
    let mut app = app();
    app.exec(Command::OpenIn);
    reply(
        &mut app,
        "open:list",
        Response::OpenTargets(vec![
            OpenOption {
                target: OpenTarget::Vscode,
                label: "VS Code".into(),
            },
            OpenOption {
                target: OpenTarget::Editor,
                label: "$EDITOR (in a terminal)".into(),
            },
            OpenOption {
                target: OpenTarget::Files,
                label: "File manager".into(),
            },
        ]),
    );
    let t = draw(&mut app, W, H);
    insta::assert_snapshot!(text(&t));
    let fx = press(&mut app, KeyCode::Enter);
    assert!(fx.iter().any(|e| matches!(
        e,
        Effect::Req {
            req: Request::OpenIn {
                target: OpenTarget::Vscode,
                ..
            },
            ..
        }
    )));
    assert!(app.modal.is_none());
}

fn layout(groups: &[(&str, &[&str])], active: &str) -> FreeTerminalLayout {
    FreeTerminalLayout {
        project_dir: "/work/shop".into(),
        groups: groups
            .iter()
            .map(|(g, p)| FreeTerminalGroup {
                id: (*g).into(),
                pane_ids: p.iter().map(|x| (*x).into()).collect(),
                active_pane_id: p.last().map(|x| (*x).into()).unwrap_or_default(),
            })
            .collect(),
        active_group_id: Some(active.into()),
        panes: groups
            .iter()
            .flat_map(|(_, p)| p.iter())
            .enumerate()
            .map(|(i, p)| FreeTerminalPane {
                id: (*p).into(),
                label: format!("Terminal {}", i + 1),
                exited: false,
            })
            .collect(),
    }
}

#[test]
fn free_terminals_open_type_and_render() {
    let mut app = app();
    let fx = app.exec(Command::TerminalToggle);
    assert!(app.free_terms.open);
    assert!(
        matches!(&fx[..], [Effect::Req { req: Request::GetTerminalLayout { project_dir }, .. }] if project_dir == "/work/shop")
    );
    // The empty layout makes the app ask for a first shell.
    let fx = app.handle_event(Event::TerminalLayoutChanged {
        project_dir: "/work/shop".into(),
        layout: layout(&[], ""),
    });
    assert!(fx.iter().any(|e| matches!(e, Effect::Req { req: Request::SpawnTerminal { cols, rows, .. }, .. } if *cols > 20 && *rows > 3)));
    let fx = app.handle_event(Event::TerminalLayoutChanged {
        project_dir: "/work/shop".into(),
        layout: layout(&[("g1", &["t1", "t2"]), ("g2", &["t3"])], "g1"),
    });
    assert!(fx.iter().any(|e| matches!(e, Effect::Req { req: Request::SubscribeTerminal { terminal_id }, .. } if terminal_id == "t1")));
    for id in ["t1", "t2", "t3"] {
        reply(
            &mut app,
            &format!("term:sub:{id}"),
            Response::Buffer { data: Vec::new() },
        );
    }
    app.handle_event(Event::TerminalOutput {
        terminal_id: "t1".into(),
        data: b"$ ls\r\nCargo.toml  src\r\n$ ".to_vec(),
    });
    app.handle_event(Event::TerminalOutput {
        terminal_id: "t2".into(),
        data: b"$ echo hi\r\nhi\r\n$ ".to_vec(),
    });
    // Typing goes to the active (last) pane of the tab, not to the agent.
    let fx = ch(&mut app, 'x');
    assert_eq!(fx, vec![Effect::TermInput("t2".into(), b"x".to_vec())]);
    let t = draw(&mut app, W, H);
    insta::assert_snapshot!(text(&t));
    // The prefix still works: `prefix t` hides the panel, keys go back to the agent.
    app.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL));
    ch(&mut app, 't');
    assert!(!app.free_terms.open);
    let fx = ch(&mut app, 'x');
    assert!(matches!(&fx[..], [Effect::SendRaw(id, _)] if id == "a1"));
}

#[test]
fn free_terminal_commands_split_tab_and_kill() {
    let mut app = app();
    app.exec(Command::TerminalToggle);
    app.handle_event(Event::TerminalLayoutChanged {
        project_dir: "/work/shop".into(),
        layout: layout(&[("g1", &["t1"])], "g1"),
    });
    let fx = app.exec(Command::TerminalSplit);
    assert!(fx.iter().any(|e| matches!(e, Effect::Req { req: Request::SpawnTerminal { group_id: Some(g), .. }, .. } if g == "g1")));
    let fx = app.exec(Command::TerminalNew);
    assert!(fx.iter().any(|e| matches!(
        e,
        Effect::Req {
            req: Request::SpawnTerminal { group_id: None, .. },
            ..
        }
    )));
    let fx = app.exec(Command::TerminalKill);
    assert!(fx.iter().any(|e| matches!(e, Effect::Req { req: Request::KillTerminal { terminal_id }, .. } if terminal_id == "t1")));
}

fn settings_app() -> App {
    let mut app = app();
    app.exec(Command::Settings);
    app
}

#[test]
fn settings_general_snapshot_and_edit_persists_through_a_patch() {
    let mut app = settings_app();
    let t = draw(&mut app, W, H);
    insta::assert_snapshot!(text(&t));
    press(&mut app, KeyCode::Enter); // into the values
    press(&mut app, KeyCode::Down);
    press(&mut app, KeyCode::Down);
    let fx = press(&mut app, KeyCode::Right); // theme
    assert!(
        matches!(&fx[..], [Effect::PatchConfig(p)] if p == &serde_json::json!({"theme": "light"}))
    );
}

#[test]
fn settings_other_sections_snapshots() {
    let mut app = settings_app();
    for _ in 0..3 {
        press(&mut app, KeyCode::Down); // usage
    }
    let t = draw(&mut app, W, H);
    insta::assert_snapshot!("settings_usage", text(&t));
    for _ in 0..2 {
        press(&mut app, KeyCode::Down); // skills
    }
    press(&mut app, KeyCode::Enter);
    reply(
        &mut app,
        "set:skills",
        Response::Skills(as_core::skills::SkillScan {
            claude: vec![as_core::skills::SkillInfo {
                id: "brainstorming".into(),
                name: "Brainstorming".into(),
                description: "Explore intent before building".into(),
                provider: ProviderId::Claude,
                group: "superpowers@market".into(),
                enabled: true,
                path: "/x/SKILL.md".into(),
                config_dir: "/c".into(),
            }],
            ..Default::default()
        }),
    );
    let t = draw(&mut app, W, H);
    insta::assert_snapshot!("settings_skills", text(&t));
}

#[test]
fn hydra_import_preview_snapshot() {
    let mut app = app();
    let fx = app.exec(Command::ImportHydra);
    assert!(fx.iter().any(|e| matches!(
        e,
        Effect::Req {
            req: Request::PreviewHydraImport { .. },
            ..
        }
    )));
    reply(
        &mut app,
        "set:import-preview",
        Response::HydraPreview(Box::new(ImportPreview {
            dir: "/home/ana/.config/Hydra".into(),
            config_changes: vec![as_core::hydra_import::ConfigChange {
                key: "theme".into(),
                current: "\"midnight\"".into(),
                incoming: "\"light\"".into(),
            }],
            agents_total: 5,
            agents_new: 4,
            keybindings_total: 3,
            keybindings_importable: 1,
            warnings: vec!["2 Hydra shortcut(s) use mod+ keys and were skipped".into()],
        })),
    );
    let t = draw(&mut app, W, H);
    insta::assert_snapshot!(text(&t));
    let fx = press(&mut app, KeyCode::Enter);
    assert!(fx.iter().any(|e| matches!(
        e,
        Effect::Req {
            req: Request::ApplyHydraImport { .. },
            ..
        }
    )));
}

#[test]
fn new_agent_dialog_offers_worktree_only_in_git_repos() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join(".git")).unwrap();
    let sub = dir.path().join("pkg");
    std::fs::create_dir(&sub).unwrap();
    let plain = tempfile::tempdir().unwrap();
    let mut form = crate::new_agent::NewAgentForm::new(
        &AppConfig::default(),
        &sub.to_string_lossy(),
        dir.path(),
    );
    assert!(form.dir_is_git_repo());
    assert!(form.fields().contains(&crate::new_agent::Field::WorkMode));
    form.worktree = true;
    assert_eq!(
        form.build_payload().unwrap().work_mode,
        as_core::WorkMode::Worktree
    );
    let mut form = crate::new_agent::NewAgentForm::new(
        &AppConfig::default(),
        &plain.path().to_string_lossy(),
        plain.path(),
    );
    form.worktree = true;
    assert!(!form.fields().contains(&crate::new_agent::Field::WorkMode));
    assert_eq!(
        form.build_payload().unwrap().work_mode,
        as_core::WorkMode::Local,
        "ignored outside a repo"
    );
}
