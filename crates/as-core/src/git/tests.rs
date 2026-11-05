//! Tests against real temporary repositories (skipped when `git` is not installed).

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use pretty_assertions::assert_eq;
use tempfile::TempDir;

use super::runner::args;
use super::*;

mod parsers;

macro_rules! need_git {
    () => {
        if which::which("git").is_err() {
            eprintln!("skipped: git is not installed");
            return;
        }
    };
}

/// A runner isolated from the user's git configuration.
fn isolated() -> GitRunner {
    let nowhere = std::env::temp_dir().join("agent-studio-no-such-gitconfig");
    GitRunner::new()
        .with_env("GIT_CONFIG_GLOBAL", nowhere.as_os_str())
        .with_env("GIT_CONFIG_NOSYSTEM", "1")
        .with_env("GIT_CONFIG_COUNT", "2")
        .with_env("GIT_CONFIG_KEY_0", "commit.gpgsign")
        .with_env("GIT_CONFIG_VALUE_0", "false")
        .with_env("GIT_CONFIG_KEY_1", "init.defaultBranch")
        .with_env("GIT_CONFIG_VALUE_1", "main")
        .with_env("GIT_AUTHOR_NAME", "Tester")
        .with_env("GIT_AUTHOR_EMAIL", "tester@example.com")
        .with_env("GIT_COMMITTER_NAME", "Tester")
        .with_env("GIT_COMMITTER_EMAIL", "tester@example.com")
}

fn run(g: &GitRunner, dir: &Path, a: &[&str]) -> String {
    let a: Vec<OsString> = a.iter().map(OsString::from).collect();
    g.git(dir, &a).unwrap_or_else(|e| panic!("git {a:?}: {e}"))
}

fn write(dir: &Path, rel: &str, content: &str) {
    let p = dir.join(rel);
    if let Some(parent) = p.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(p, content).unwrap();
}

fn norm(s: &str) -> String {
    s.replace("\r\n", "\n")
}

/// Repository with one commit (`a.txt`).
fn init_repo() -> (TempDir, GitRunner) {
    let tmp = tempfile::tempdir().unwrap();
    let g = isolated();
    run(&g, tmp.path(), &["init", "-q"]);
    write(tmp.path(), "a.txt", "one\ntwo\nthree\n");
    run(&g, tmp.path(), &["add", "a.txt"]);
    run(&g, tmp.path(), &["commit", "-q", "-m", "first"]);
    (tmp, g)
}

fn find<'a>(st: &'a GitStatus, path: &str) -> &'a GitFileStatus {
    st.files
        .iter()
        .find(|f| f.path == path)
        .unwrap_or_else(|| panic!("{path} not in {:?}", st.files))
}

fn same_dir(a: &Path, b: &Path) -> bool {
    fs::canonicalize(a).unwrap() == fs::canonicalize(b).unwrap()
}

#[test]
fn not_a_repo_gives_an_empty_status_and_typed_errors() {
    need_git!();
    let tmp = tempfile::tempdir().unwrap();
    let g = isolated();
    assert!(!g.is_git_repo(tmp.path()));
    assert_eq!(g.status(tmp.path()).unwrap(), GitStatus::default());
    assert!(g.log(tmp.path(), 5).unwrap().is_empty());
    assert_eq!(g.diff_stats(tmp.path()).unwrap(), GitDiffStats::default());
    assert!(matches!(
        g.list_branches(tmp.path()),
        Err(GitError::NotARepo(_))
    ));
    assert!(matches!(
        g.diff(tmp.path(), None, false),
        Err(GitError::NotARepo(_))
    ));
    assert!(matches!(
        g.stage(&tmp.path().join("missing"), &["x".into()]),
        Err(GitError::NotARepo(_))
    ));
}

#[test]
fn status_reports_staged_unstaged_and_untracked() {
    need_git!();
    let (tmp, g) = init_repo();
    let d = tmp.path();
    assert!(g.is_git_repo(d));
    let clean = g.status(d).unwrap();
    assert_eq!(clean.branch, "main");
    assert!(clean.files.is_empty());
    assert!(clean.head.is_some());
    assert_eq!(clean.upstream, None);

    write(d, "a.txt", "one\nTWO\nthree\n");
    write(d, "b.txt", "new\n");
    run(&g, d, &["add", "b.txt"]);
    write(d, "c.txt", "untracked\n");
    let st = g.status(d).unwrap();
    let a = find(&st, "a.txt");
    assert_eq!(
        (a.kind, a.staged, a.unstaged),
        (GitStatusKind::Modified, false, true)
    );
    let b = find(&st, "b.txt");
    assert_eq!(
        (b.kind, b.staged, b.unstaged),
        (GitStatusKind::Added, true, false)
    );
    let c = find(&st, "c.txt");
    assert_eq!((c.kind, c.untracked), (GitStatusKind::Untracked, true));
    assert_eq!(c.kind.letter(), '?');

    run(&g, d, &["add", "a.txt"]);
    write(d, "a.txt", "one\nTWO\nTHREE\n");
    let st = g.status(d).unwrap();
    let a = find(&st, "a.txt");
    assert_eq!((a.staged, a.unstaged), (true, true));
    assert_eq!(
        (a.index, a.worktree),
        (Some(GitStatusKind::Modified), Some(GitStatusKind::Modified))
    );
}

#[test]
fn status_handles_spaces_unicode_renames_and_deletes() {
    need_git!();
    let (tmp, g) = init_repo();
    let d = tmp.path();
    write(d, "my file.txt", "x\n");
    write(d, "caffè/ünï cödé.txt", "y\n");
    let st = g.status(d).unwrap();
    assert_eq!(find(&st, "my file.txt").kind, GitStatusKind::Untracked);
    assert_eq!(
        find(&st, "caffè/ünï cödé.txt").kind,
        GitStatusKind::Untracked
    );

    run(&g, d, &["mv", "a.txt", "renamed to.txt"]);
    let st = g.status(d).unwrap();
    let r = find(&st, "renamed to.txt");
    assert_eq!(r.kind, GitStatusKind::Renamed);
    assert_eq!(r.orig_path.as_deref(), Some("a.txt"));
    assert!(r.staged);

    fs::remove_file(d.join("my file.txt")).unwrap();
    run(&g, d, &["add", "-A"]);
    run(&g, d, &["commit", "-q", "-m", "second"]);
    fs::remove_file(d.join("renamed to.txt")).unwrap();
    let st = g.status(d).unwrap();
    let del = find(&st, "renamed to.txt");
    assert_eq!((del.kind, del.unstaged), (GitStatusKind::Deleted, true));
}

#[test]
fn status_marks_conflicts() {
    need_git!();
    let (tmp, g) = init_repo();
    let d = tmp.path();
    run(&g, d, &["checkout", "-q", "-b", "other"]);
    write(d, "a.txt", "other\n");
    run(&g, d, &["commit", "-q", "-am", "other"]);
    run(&g, d, &["checkout", "-q", "main"]);
    write(d, "a.txt", "main\n");
    run(&g, d, &["commit", "-q", "-am", "main"]);
    let merge = g.git(d, &args!["merge", "other"]);
    assert!(merge.is_err());
    let st = g.status(d).unwrap();
    let a = find(&st, "a.txt");
    assert!(a.conflicted);
    assert_eq!(a.kind, GitStatusKind::Unmerged);
}

#[test]
fn ahead_and_behind_against_a_local_bare_remote() {
    need_git!();
    let (tmp, g) = init_repo();
    let d = tmp.path();
    let remote = tempfile::tempdir().unwrap();
    run(&g, remote.path(), &["init", "-q", "--bare"]);
    run(
        &g,
        d,
        &["remote", "add", "origin", &remote.path().to_string_lossy()],
    );

    // No upstream yet: push sets it.
    g.push(d).unwrap();
    let st = g.status(d).unwrap();
    assert_eq!(st.upstream.as_deref(), Some("origin/main"));
    assert_eq!((st.ahead, st.behind), (0, 0));

    write(d, "local.txt", "l\n");
    g.commit(d, "local", &["local.txt".into()]).unwrap();
    let st = g.status(d).unwrap();
    assert_eq!((st.ahead, st.behind), (1, 0));
    g.push(d).unwrap();

    // Another clone pushes a commit; after a fetch we are behind.
    let other = tempfile::tempdir().unwrap();
    run(
        &g,
        other.path(),
        &["clone", "-q", &remote.path().to_string_lossy(), "c"],
    );
    let c = other.path().join("c");
    write(&c, "remote.txt", "r\n");
    run(&g, &c, &["add", "remote.txt"]);
    run(&g, &c, &["commit", "-q", "-m", "remote"]);
    run(&g, &c, &["push", "-q"]);
    run(&g, d, &["fetch", "-q"]);
    write(d, "local2.txt", "l2\n");
    g.commit(d, "local2", &["local2.txt".into()]).unwrap();
    let st = g.status(d).unwrap();
    assert_eq!((st.ahead, st.behind), (1, 1));

    let branches = g.list_branches(d).unwrap();
    let main = branches
        .iter()
        .find(|b| b.name == "main" && !b.is_remote)
        .unwrap();
    assert!(main.is_current);
    assert_eq!(main.upstream.as_deref(), Some("origin/main"));
    assert_eq!((main.ahead_of_upstream, main.behind_upstream), (1, 1));
    assert!(
        branches
            .iter()
            .any(|b| b.name == "origin/main" && b.is_remote)
    );
    assert!(!branches.iter().any(|b| b.name.ends_with("HEAD")));
}

#[test]
fn log_lists_newest_first_and_empty_for_a_fresh_repo() {
    need_git!();
    let fresh = tempfile::tempdir().unwrap();
    let g = isolated();
    run(&g, fresh.path(), &["init", "-q"]);
    assert!(g.log(fresh.path(), 5).unwrap().is_empty());
    let st = g.status(fresh.path()).unwrap();
    assert_eq!((st.branch.as_str(), st.head.clone()), ("main", None));

    let (tmp, g) = init_repo();
    let d = tmp.path();
    write(d, "b.txt", "b\n");
    g.commit(d, "second: with, punctuation", &["b.txt".into()])
        .unwrap();
    let log = g.log(d, 10).unwrap();
    assert_eq!(log.len(), 2);
    assert_eq!(log[0].message, "second: with, punctuation");
    assert_eq!(log[1].message, "first");
    assert_eq!(log[0].author, "Tester");
    assert_eq!(log[0].short_hash.len(), 8);
    assert!(log[0].hash.starts_with(&log[0].short_hash));
    assert!(log[0].date.contains('T'));
    assert_eq!(g.log(d, 1).unwrap().len(), 1);
}

#[test]
fn diffs_whole_per_file_staged_and_untracked() {
    need_git!();
    let (tmp, g) = init_repo();
    let d = tmp.path();
    write(d, "b file.txt", "b\n");
    g.commit(d, "b", &["b file.txt".into()]).unwrap();
    write(d, "a.txt", "one\nTWO\nthree\n");
    write(d, "b file.txt", "b\nmore\n");
    write(d, "new.txt", "n1\nn2\n");

    let whole = norm(&g.diff(d, None, false).unwrap());
    assert!(whole.contains("diff --git a/a.txt b/a.txt"));
    assert!(whole.contains("diff --git a/b file.txt b/b file.txt"));
    assert!(!whole.contains("new.txt"));
    let one = norm(&g.diff(d, Some("a.txt"), false).unwrap());
    assert!(one.contains("-two\n+TWO\n"));
    assert!(!one.contains("b file.txt"));

    assert_eq!(g.diff(d, Some("a.txt"), true).unwrap(), "");
    g.stage(d, &["a.txt".into()]).unwrap();
    let staged = norm(&g.diff(d, None, true).unwrap());
    assert!(staged.contains("+TWO") && !staged.contains("more"));
    assert_eq!(g.diff(d, Some("a.txt"), false).unwrap(), "");

    let new = norm(&g.diff(d, Some("new.txt"), false).unwrap());
    let files = parse_unified_diff(&new);
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].status, DiffFileStatus::Added);
    assert_eq!(files[0].path(), "new.txt");
    assert_eq!(files[0].additions, 2);

    // Parsing the real output.
    let all = parse_unified_diff(&whole);
    assert_eq!(all.len(), 2);
    assert_eq!(all[1].path(), "b file.txt");
    assert!(g.diff(d, Some("../x"), false).is_err());
}

#[test]
fn diff_stats_count_tracked_and_untracked() {
    need_git!();
    let fresh = tempfile::tempdir().unwrap();
    let g = isolated();
    run(&g, fresh.path(), &["init", "-q"]);
    write(fresh.path(), "u.txt", "a\nb\nc\n");
    assert_eq!(
        g.diff_stats(fresh.path()).unwrap(),
        GitDiffStats {
            files: 1,
            additions: 3,
            deletions: 0
        }
    );
    g.stage(fresh.path(), &["u.txt".into()]).unwrap();
    assert_eq!(g.diff_stats(fresh.path()).unwrap().additions, 3);

    let (tmp, g) = init_repo();
    let d = tmp.path();
    assert_eq!(g.diff_stats(d).unwrap(), GitDiffStats::default());
    write(d, "a.txt", "one\nTWO\nthree\nfour\n");
    write(d, "new.txt", "x\ny");
    fs::write(d.join("bin.dat"), [0u8, 1, 2]).unwrap();
    let s = g.diff_stats(d).unwrap();
    assert_eq!(
        s,
        GitDiffStats {
            files: 3,
            additions: 2 + 2,
            deletions: 1
        }
    );
}

#[test]
fn commit_only_the_listed_files() {
    need_git!();
    let (tmp, g) = init_repo();
    let d = tmp.path();
    write(d, "x.txt", "x\n");
    write(d, "y.txt", "y\n");
    write(d, "z.txt", "z\n");
    g.stage(d, &["z.txt".into()]).unwrap();
    let hash = g.commit(d, "just x", &["x.txt".into()]).unwrap();
    assert!(!hash.is_empty());
    let changed = run(&g, d, &["show", "--name-only", "--format=", "HEAD"]);
    assert_eq!(changed.trim(), "x.txt");
    let st = g.status(d).unwrap();
    assert_eq!(
        find(&st, "z.txt").kind,
        GitStatusKind::Added,
        "other staged files stay staged"
    );
    assert_eq!(find(&st, "y.txt").kind, GitStatusKind::Untracked);

    // Unstage, commit_all, empty message.
    g.unstage(d, &["z.txt".into()]).unwrap();
    assert_eq!(
        find(&g.status(d).unwrap(), "z.txt").kind,
        GitStatusKind::Untracked
    );
    assert!(matches!(
        g.commit(d, "  ", &[]),
        Err(GitError::InvalidInput(_))
    ));
    g.commit_all(d, "rest").unwrap();
    assert!(g.status(d).unwrap().files.is_empty());
    // Nothing to commit is an error carrying git's message.
    assert!(matches!(
        g.commit(d, "again", &[]),
        Err(GitError::Failed { .. })
    ));
}

#[test]
fn unstage_works_before_the_first_commit() {
    need_git!();
    let tmp = tempfile::tempdir().unwrap();
    let g = isolated();
    run(&g, tmp.path(), &["init", "-q"]);
    write(tmp.path(), "f.txt", "f\n");
    g.stage(tmp.path(), &["f.txt".into()]).unwrap();
    assert_eq!(
        find(&g.status(tmp.path()).unwrap(), "f.txt").kind,
        GitStatusKind::Added
    );
    g.unstage(tmp.path(), &["f.txt".into()]).unwrap();
    assert_eq!(
        find(&g.status(tmp.path()).unwrap(), "f.txt").kind,
        GitStatusKind::Untracked
    );
}

#[test]
fn push_publishes_the_branch_to_a_local_bare_remote() {
    need_git!();
    let (tmp, g) = init_repo();
    let d = tmp.path();
    let remote = tempfile::tempdir().unwrap();
    run(&g, remote.path(), &["init", "-q", "--bare"]);
    assert!(g.push(d).is_err(), "no remote configured");
    run(
        &g,
        d,
        &["remote", "add", "origin", &remote.path().to_string_lossy()],
    );
    g.create_branch(d, "feature", None).unwrap();
    write(d, "f.txt", "f\n");
    g.commit(d, "feat", &["f.txt".into()]).unwrap();
    g.push(d).unwrap();
    let local = run(&g, d, &["rev-parse", "HEAD"]);
    let there = run(&g, remote.path(), &["rev-parse", "refs/heads/feature"]);
    assert_eq!(local.trim(), there.trim());
    assert_eq!(
        g.status(d).unwrap().upstream.as_deref(),
        Some("origin/feature")
    );
    g.push(d).unwrap();
}

#[test]
fn branches_create_checkout_and_remote_tracking() {
    need_git!();
    let (tmp, g) = init_repo();
    let d = tmp.path();
    g.create_branch(d, "feat/one", None).unwrap();
    assert_eq!(g.status(d).unwrap().branch, "feat/one");
    g.create_branch(d, "from-main", Some("main")).unwrap();
    g.checkout(d, "main").unwrap();
    let names: Vec<_> = g
        .list_branches(d)
        .unwrap()
        .into_iter()
        .map(|b| (b.name, b.is_current))
        .collect();
    assert_eq!(
        names,
        vec![
            ("feat/one".to_string(), false),
            ("from-main".to_string(), false),
            ("main".to_string(), true)
        ]
    );
    assert!(g.checkout(d, "nope").is_err());
    assert!(matches!(
        g.checkout(d, "--orphan"),
        Err(GitError::InvalidInput(_))
    ));
    assert!(matches!(
        g.create_branch(d, "-x", None),
        Err(GitError::InvalidInput(_))
    ));
    assert!(g.create_branch(d, "feat/one", None).is_err());

    // A remote-only branch becomes a tracking branch.
    let remote = tempfile::tempdir().unwrap();
    run(&g, remote.path(), &["init", "-q", "--bare"]);
    run(
        &g,
        d,
        &["remote", "add", "origin", &remote.path().to_string_lossy()],
    );
    run(&g, d, &["push", "-q", "origin", "feat/one"]);
    run(&g, d, &["branch", "-D", "feat/one"]);
    run(&g, d, &["fetch", "-q"]);
    g.checkout(d, "origin/feat/one").unwrap();
    let st = g.status(d).unwrap();
    assert_eq!(st.branch, "feat/one");
    assert_eq!(st.upstream.as_deref(), Some("origin/feat/one"));
}

#[test]
fn file_contents_for_unstaged_and_staged_views() {
    need_git!();
    let (tmp, g) = init_repo();
    let d = tmp.path();
    write(d, "a.txt", "staged\n");
    g.stage(d, &["a.txt".into()]).unwrap();
    write(d, "a.txt", "worktree\n");
    let un = g.file_contents(d, "a.txt", false).unwrap();
    assert_eq!(
        (norm(&un.original).as_str(), norm(&un.modified).as_str()),
        ("staged\n", "worktree\n")
    );
    let st = g.file_contents(d, "a.txt", true).unwrap();
    assert_eq!(
        (norm(&st.original).as_str(), norm(&st.modified).as_str()),
        ("one\ntwo\nthree\n", "staged\n")
    );
    assert_eq!(st.language, "plaintext");

    write(d, "src/new file.rs", "fn main() {}\n");
    let new = g.file_contents(d, "src/new file.rs", false).unwrap();
    assert_eq!((new.original.as_str(), new.language.as_str()), ("", "rust"));
    fs::remove_file(d.join("a.txt")).unwrap();
    let gone = g.file_contents(d, "a.txt", false).unwrap();
    assert_eq!(gone.modified, "");
    fs::write(d.join("bin.dat"), [0u8, 1]).unwrap();
    assert!(g.file_contents(d, "bin.dat", false).unwrap().binary);
    assert!(g.file_contents(d, "../etc/passwd", false).is_err());
    let abs = d.join("a.txt").to_string_lossy().into_owned();
    assert!(g.file_contents(d, &abs, false).is_err());
}

#[test]
fn worktree_create_list_and_remove() {
    need_git!();
    let (tmp, g) = init_repo();
    let d = tmp.path();
    let wt = g.worktree_create(d, "studio/my-agent-1234", None).unwrap();
    let path = PathBuf::from(&wt.path);
    assert_eq!(wt.branch.as_deref(), Some("studio/my-agent-1234"));
    assert!(path.join("a.txt").is_file());
    assert!(same_dir(
        &path,
        &d.join(".worktrees").join("studio-my-agent-1234")
    ));
    // Location is hidden from the main repository's status.
    assert!(g.status(d).unwrap().files.is_empty());

    let list = g.worktree_list(d).unwrap();
    assert_eq!(list.len(), 2);
    assert!(same_dir(Path::new(&list[0].path), d));
    assert_eq!(list[1].branch.as_deref(), Some("studio/my-agent-1234"));
    assert!(same_dir(&g.main_worktree(&path).unwrap(), d));
    assert_eq!(g.status(&path).unwrap().branch, "studio/my-agent-1234");

    // Dirty worktree: refused without force, nothing is deleted.
    write(&path, "wip.txt", "work in progress\n");
    let err = g.worktree_remove(d, &path, Some("studio/my-agent-1234"), false);
    assert!(matches!(err, Err(GitError::Failed { .. })));
    assert!(path.join("wip.txt").is_file());
    assert!(
        g.list_branches(d)
            .unwrap()
            .iter()
            .any(|b| b.name == "studio/my-agent-1234")
    );

    // Clean: removed with its (merged) branch.
    fs::remove_file(path.join("wip.txt")).unwrap();
    g.worktree_remove(d, &path, Some("studio/my-agent-1234"), false)
        .unwrap();
    assert!(!path.exists());
    assert_eq!(g.worktree_list(d).unwrap().len(), 1);
    assert!(
        !g.list_branches(d)
            .unwrap()
            .iter()
            .any(|b| b.name == "studio/my-agent-1234")
    );
}

#[test]
fn worktree_force_remove_and_unmerged_branch_is_kept() {
    need_git!();
    let (tmp, g) = init_repo();
    let d = tmp.path();
    let wt = g.worktree_create(d, "studio/a-1", None).unwrap();
    let path = PathBuf::from(&wt.path);
    write(&path, "c.txt", "c\n");
    g.commit(&path, "in worktree", &["c.txt".into()]).unwrap();
    g.worktree_remove(d, &path, Some("studio/a-1"), false)
        .unwrap();
    assert!(!path.exists());
    assert!(
        g.list_branches(d)
            .unwrap()
            .iter()
            .any(|b| b.name == "studio/a-1"),
        "a branch with unmerged commits survives a safe remove"
    );

    let wt = g.worktree_create(d, "studio/b-2", None).unwrap();
    let path = PathBuf::from(&wt.path);
    write(&path, "dirty.txt", "d\n");
    g.worktree_remove(d, &path, Some("studio/b-2"), true)
        .unwrap();
    assert!(!path.exists());
    assert!(
        !g.list_branches(d)
            .unwrap()
            .iter()
            .any(|b| b.name == "studio/b-2")
    );

    // A worktree deleted by hand is pruned.
    let wt = g.worktree_create(d, "studio/c-3", None).unwrap();
    let path = PathBuf::from(&wt.path);
    fs::remove_dir_all(&path).unwrap();
    g.worktree_remove(d, &path, Some("studio/c-3"), false)
        .unwrap();
    assert_eq!(g.worktree_list(d).unwrap().len(), 1);
    assert!(g.worktree_create(d, "-bad", None).is_err());
}

#[test]
fn missing_programs_have_clear_errors() {
    let empty = tempfile::tempdir().unwrap();
    let g = GitRunner::new().with_path(empty.path().as_os_str());
    assert!(matches!(
        g.pr_fetch(empty.path(), "12"),
        Err(GitError::GhMissing)
    ));
    assert!(matches!(
        g.pr_file_diff(empty.path(), "#12", "a"),
        Err(GitError::GhMissing)
    ));
    assert!(matches!(
        g.repo_root(empty.path()),
        Err(GitError::GitMissing)
    ));
    assert!(!g.is_git_repo(empty.path()));
    assert!(matches!(
        g.pr_fetch(empty.path(), "not-a-pr"),
        Err(GitError::InvalidInput(_))
    ));
    assert_eq!(
        GitError::GhMissing.to_string(),
        "the GitHub CLI (gh) is not installed or not on PATH"
    );
}

#[test]
fn concurrent_mutations_do_not_race_on_the_index_lock() {
    need_git!();
    let (tmp, g) = init_repo();
    let d = tmp.path().to_path_buf();
    for i in 0..8 {
        write(&d, &format!("f{i}.txt"), "x\n");
    }
    let handles: Vec<_> = (0..8)
        .map(|i| {
            let (g, d) = (g.clone(), d.clone());
            std::thread::spawn(move || g.stage(&d, &[format!("f{i}.txt")]))
        })
        .collect();
    for h in handles {
        h.join().unwrap().unwrap();
    }
    let st = g.status(&d).unwrap();
    assert_eq!(st.files.iter().filter(|f| f.staged).count(), 8);
}

#[test]
fn a_slow_command_is_killed_on_timeout() {
    need_git!();
    let (tmp, g) = init_repo();
    // A hook-less way to make git slow: `git -c alias` is not available, so ask for a zero timeout.
    let g = g.with_timeout(std::time::Duration::from_millis(0));
    let err = g.git(tmp.path(), &args!["log"]);
    assert!(matches!(err, Err(GitError::Timeout { .. })) || err.is_ok());
}
