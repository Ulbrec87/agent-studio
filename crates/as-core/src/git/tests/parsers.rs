//! Pure parser tests (no `git` needed).

use pretty_assertions::assert_eq;

use crate::git::*;

#[test]
fn parses_hunks_with_line_numbers_and_the_no_newline_marker() {
    let diff = "diff --git a/f.txt b/f.txt\nindex 111..222 100644\n--- a/f.txt\n+++ b/f.txt\n@@ -1,3 +1,3 @@ fn section\n keep\n-old\n+new\n same\n@@ -10 +10,2 @@\n-x\n\\ No newline at end of file\n+y\n+z\n\\ No newline at end of file\n";
    let files = parse_unified_diff(diff);
    assert_eq!(files.len(), 1);
    let f = &files[0];
    assert_eq!(
        (f.old_path.as_deref(), f.new_path.as_deref()),
        (Some("f.txt"), Some("f.txt"))
    );
    assert_eq!(f.status, DiffFileStatus::Modified);
    assert_eq!((f.additions, f.deletions), (3, 2));
    assert_eq!(f.hunks.len(), 2);
    let h = &f.hunks[0];
    assert_eq!(h.header, "@@ -1,3 +1,3 @@ fn section");
    assert_eq!(
        (h.old_start, h.old_lines, h.new_start, h.new_lines),
        (1, 3, 1, 3)
    );
    let l = &h.lines;
    assert_eq!(
        (l[0].kind, l[0].old_no, l[0].new_no, l[0].text.as_str()),
        (DiffLineKind::Context, Some(1), Some(1), "keep")
    );
    assert_eq!(
        (l[1].kind, l[1].old_no, l[1].new_no),
        (DiffLineKind::Del, Some(2), None)
    );
    assert_eq!(
        (l[2].kind, l[2].old_no, l[2].new_no, l[2].text.as_str()),
        (DiffLineKind::Add, None, Some(2), "new")
    );
    assert_eq!((l[3].old_no, l[3].new_no), (Some(3), Some(3)));
    let h2 = &f.hunks[1];
    assert_eq!(
        (h2.old_start, h2.old_lines, h2.new_start, h2.new_lines),
        (10, 1, 10, 2)
    );
    assert_eq!(h2.lines.len(), 5);
    assert_eq!(h2.lines[1].kind, DiffLineKind::Meta);
    assert_eq!(h2.lines[1].text, "\\ No newline at end of file");
    assert_eq!(h2.lines[3].new_no, Some(11));
    assert_eq!(h2.lines[4].kind, DiffLineKind::Meta);
}

#[test]
fn a_removed_line_starting_with_dashes_is_not_a_file_header() {
    let diff = "diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -1,2 +1 @@\n--- not a header\n keep\ndiff --git a/g b/g\n--- a/g\n+++ b/g\n@@ -1 +1 @@\n-a\n+b\n";
    let files = parse_unified_diff(diff);
    assert_eq!(files.len(), 2);
    assert_eq!(files[0].hunks[0].lines[0].text, "-- not a header");
    assert_eq!(files[0].deletions, 1);
    assert_eq!(files[1].path(), "g");
}

#[test]
fn parses_new_deleted_renamed_and_binary_files() {
    let diff = "diff --git a/new.txt b/new.txt\nnew file mode 100644\nindex 000..111\n--- /dev/null\n+++ b/new.txt\n@@ -0,0 +1,2 @@\n+a\n+b\ndiff --git a/gone.txt b/gone.txt\ndeleted file mode 100644\nindex 111..000\n--- a/gone.txt\n+++ /dev/null\n@@ -1 +0,0 @@\n-bye\ndiff --git a/old name.txt b/new name.txt\nsimilarity index 100%\nrename from old name.txt\nrename to new name.txt\ndiff --git a/img.png b/img.png\nindex 1..2 100644\nBinary files a/img.png and b/img.png differ\n";
    let files = parse_unified_diff(diff);
    assert_eq!(files.len(), 4);
    assert_eq!(
        (
            files[0].status,
            files[0].old_path.clone(),
            files[0].new_path.as_deref()
        ),
        (DiffFileStatus::Added, None, Some("new.txt"))
    );
    assert_eq!(files[0].additions, 2);
    assert_eq!(
        (files[1].status, files[1].new_path.clone(), files[1].path()),
        (DiffFileStatus::Deleted, None, "gone.txt")
    );
    assert_eq!(files[1].hunks[0].lines[0].kind, DiffLineKind::Del);
    assert_eq!(files[2].status, DiffFileStatus::Renamed);
    assert_eq!(files[2].old_path.as_deref(), Some("old name.txt"));
    assert_eq!(files[2].new_path.as_deref(), Some("new name.txt"));
    assert!(files[2].hunks.is_empty());
    assert!(files[3].binary);
    assert_eq!(files[3].path(), "img.png");
}

#[test]
fn header_paths_with_spaces_tabs_and_quotes() {
    let diff = "diff --git a/my file.txt b/my file.txt\n--- a/my file.txt\t\n+++ b/my file.txt\t\n@@ -1 +1 @@\n-a\n+b\ndiff --git \"a/tab\\tname\" \"b/tab\\tname\"\nBinary files \"a/tab\\tname\" and \"b/tab\\tname\" differ\n";
    let files = parse_unified_diff(diff);
    assert_eq!(files[0].path(), "my file.txt");
    assert_eq!(files[1].path(), "tab\tname");
    // Header only (no ---/+++): the `a/P b/P` split still finds the path.
    let only = parse_unified_diff("diff --git a/x y/z w.txt b/x y/z w.txt\nBinary files differ\n");
    assert_eq!(only[0].path(), "x y/z w.txt");
}

#[test]
fn plain_unified_diffs_and_crlf_and_garbage() {
    let plain = "--- old.txt\t2020-01-01\n+++ new.txt\t2020-01-02\n@@ -1 +1 @@\n-a\n+b\n--- second\n+++ second\n@@ -1 +1 @@\n-c\n+d\n";
    let files = parse_unified_diff(plain);
    assert_eq!(files.len(), 2);
    assert_eq!(
        (files[0].old_path.as_deref(), files[0].new_path.as_deref()),
        (Some("old.txt"), Some("new.txt"))
    );
    assert_eq!(files[1].path(), "second");

    let crlf = parse_unified_diff(
        "diff --git a/f b/f\r\n--- a/f\r\n+++ b/f\r\n@@ -1 +1 @@\r\n-a\r\n+b\r\n",
    );
    assert_eq!(crlf[0].path(), "f");
    assert_eq!(crlf[0].hunks[0].lines[1].text, "b\r");

    assert!(parse_unified_diff("").is_empty());
    assert!(parse_unified_diff("not a diff at all\n").is_empty());
    let broken = parse_unified_diff("@@ -x +y @@\n+a\n");
    assert!(broken.len() <= 1);
}

#[test]
fn splits_a_multi_file_diff_per_file() {
    let diff = "diff --git a/a.rs b/a.rs\n--- a/a.rs\n+++ b/a.rs\n@@ -1 +1 @@\n-1\n+2\ndiff --git a/dir/b c.md b/dir/b c.md\nnew file mode 100644\n--- /dev/null\n+++ b/dir/b c.md\n@@ -0,0 +1 @@\n+x\n";
    let parts = split_diff_by_file(diff);
    assert_eq!(
        parts.iter().map(|p| p.path.as_str()).collect::<Vec<_>>(),
        ["a.rs", "dir/b c.md"]
    );
    assert!(parts[0].patch.starts_with("diff --git a/a.rs"));
    assert!(parts[0].patch.ends_with("+2"));
    assert!(extract_file_diff(diff, "dir/b c.md").contains("new file mode"));
    assert_eq!(extract_file_diff(diff, "missing"), "");
    assert_eq!(parse_pr_diff(diff).len(), 2);
    let files = pr_files_from_diff(diff);
    assert_eq!(
        (
            files[0].status.as_str(),
            files[0].additions,
            files[0].deletions
        ),
        ("modified", 1, 1)
    );
    assert_eq!(files[1].status, "added");
}

#[test]
fn pr_identifiers_and_metadata() {
    assert_eq!(parse_pr_identifier("42").unwrap(), "42");
    assert_eq!(parse_pr_identifier(" #7 ").unwrap(), "7");
    assert_eq!(
        parse_pr_identifier("https://github.com/o/r/pull/9/files").unwrap(),
        "https://github.com/o/r/pull/9/files"
    );
    assert_eq!(parse_pr_identifier("o/r/pull/12").unwrap(), "12");
    assert!(parse_pr_identifier("abc").is_err());
    assert!(parse_pr_identifier("https://x/pull/").is_err());

    let m = parse_pr_metadata(r#"{"number":5,"title":"T","author":{"login":"bob"},"state":"OPEN","baseRefName":"main","headRefName":"f","additions":3,"deletions":1,"changedFiles":2,"url":"u","createdAt":"c","updatedAt":"d"}"#).unwrap();
    assert_eq!(
        (
            m.number,
            m.author.as_str(),
            m.body.as_str(),
            m.changed_files
        ),
        (5, "bob", "", 2)
    );
    assert_eq!(parse_pr_metadata("{}").unwrap().author, "unknown");
    assert!(parse_pr_metadata("nope").is_err());
}

#[test]
fn parses_porcelain_v2_records() {
    let out = b"# branch.oid 0123456789abcdef\0# branch.head main\0# branch.upstream origin/main\0# branch.ab +2 -3\0\
1 .M N... 100644 100644 100644 aaa bbb dir/sp ace.txt\0\
1 A. N... 000000 100644 100644 000 bbb new.txt\0\
2 R. N... 100644 100644 100644 aaa bbb R100 new name.txt\0old name.txt\0\
u UU N... 100644 100644 100644 100644 a b c conflict.txt\0\
? un tracked.txt\0! ignored.txt\0";
    let st = parse_status_v2(out);
    assert_eq!(
        (
            st.branch.as_str(),
            st.upstream.as_deref(),
            st.ahead,
            st.behind
        ),
        ("main", Some("origin/main"), 2, 3)
    );
    assert_eq!(st.head.as_deref(), Some("01234567"));
    let paths: Vec<_> = st.files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(
        paths,
        [
            "dir/sp ace.txt",
            "new.txt",
            "new name.txt",
            "conflict.txt",
            "un tracked.txt"
        ]
    );
    assert_eq!(st.files[0].kind, GitStatusKind::Modified);
    assert_eq!(st.files[2].orig_path.as_deref(), Some("old name.txt"));
    assert!(st.files[3].conflicted);
    let detached = parse_status_v2(b"# branch.oid abc\0# branch.head (detached)\0");
    assert!(detached.detached && detached.branch.is_empty());
}

#[test]
fn parses_worktree_list() {
    let out = "worktree /repo\nHEAD 1234567890abcdef\nbranch refs/heads/main\n\nworktree /repo/.worktrees/x\nHEAD abcdef1234567890\nbranch refs/heads/studio/x\nlocked\n\nworktree /bare\nbare\n\nworktree /d\nHEAD ff\ndetached\nprunable gitdir file points to non-existent location\n";
    let l = parse_worktree_list(out);
    assert_eq!(l.len(), 4);
    assert_eq!(l[0].branch.as_deref(), Some("main"));
    assert_eq!(l[0].head.as_deref(), Some("12345678"));
    assert!(l[1].locked && l[2].bare && l[3].detached && l[3].prunable);
    assert_eq!(l[1].branch.as_deref(), Some("studio/x"));
}

#[test]
fn language_detection_and_serde_shapes() {
    assert_eq!(detect_language("src/main.RS"), "rust");
    assert_eq!(detect_language("Makefile"), "plaintext");
    assert_eq!(detect_language("a.toml"), "ini");
    let v = serde_json::to_value(GitStatusKind::Untracked).unwrap();
    assert_eq!(v, serde_json::json!("?"));
    let st = GitStatus {
        is_repo: true,
        ..GitStatus::default()
    };
    let json = serde_json::to_value(&st).unwrap();
    assert_eq!(json["isRepo"], true);
    let back: GitStatus = serde_json::from_value(json).unwrap();
    assert_eq!(back, st);
}
