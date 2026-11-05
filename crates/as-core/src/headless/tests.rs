use std::sync::Arc;

use super::*;

fn payload(dir: &str) -> StartHeadlessRunPayload {
    StartHeadlessRunPayload {
        prompt: "say ok".into(),
        project_dir: dir.into(),
        provider: ProviderId::Claude,
        model: "sonnet".into(),
        ..Default::default()
    }
}

/// An orchestrator whose "CLI" is a shell command line.
fn scripted(dir: &Path, unix: &'static str, windows: &'static str) -> HeadlessOrchestrator {
    HeadlessOrchestrator::with_override(
        dir,
        Some(Arc::new(move |_run| {
            Some(if cfg!(windows) {
                SpawnSpec {
                    cmd: "cmd.exe".into(),
                    args: vec!["/c".into(), windows.into()],
                }
            } else {
                SpawnSpec {
                    cmd: "sh".into(),
                    args: vec!["-c".into(), unix.into()],
                }
            })
        })),
    )
}

async fn wait_for_status(
    o: &HeadlessOrchestrator,
    id: &str,
    want: HeadlessRunStatus,
) -> HeadlessRun {
    for _ in 0..200 {
        if let Some(run) = o.get(id)
            && run.status == want
            && o.running_count() == 0
        {
            return run;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("run {id} never reached {want:?}: {:?}", o.get(id));
}

#[test]
fn hydrates_persisted_runs_and_filters() {
    let dir = tempfile::tempdir().unwrap();
    let run = HeadlessRun {
        id: "run12345".into(),
        prompt: "Build inventory dashboard".into(),
        project_dir: "/tmp/ep_inventory".into(),
        model: "sonnet".into(),
        status: HeadlessRunStatus::Completed,
        started_at: "2026-02-07T10:00:00.000Z".into(),
        ended_at: Some("2026-02-07T10:03:00.000Z".into()),
        session_id: Some("session-abc".into()),
        ..Default::default()
    };
    std::fs::write(
        dir.path().join("run12345.meta.json"),
        serde_json::json!({"schemaVersion": 1, "run": run}).to_string(),
    )
    .unwrap();
    std::fs::write(
        dir.path().join("run12345.jsonl"),
        "line1\nline2\nline3\nline4\nline5",
    )
    .unwrap();
    std::fs::write(dir.path().join("broken.meta.json"), "{nope").unwrap();

    let o = HeadlessOrchestrator::new(dir.path());
    assert_eq!(o.list(&Default::default()).len(), 1);
    let filtered = o.list(&ListHeadlessRunsOptions {
        query: Some("inventory".into()),
        status: StatusFilter::Completed,
        limit: None,
    });
    assert_eq!(filtered.len(), 1);
    assert!(
        o.list(&ListHeadlessRunsOptions {
            status: StatusFilter::Errored,
            ..Default::default()
        })
        .is_empty()
    );
    let log = o
        .get_log(
            "run12345",
            &HeadlessRunLogOptions {
                tail_lines: Some(2),
                max_chars: Some(1000),
            },
        )
        .unwrap();
    assert_eq!(log.content, "line4\nline5");
    assert_eq!((log.total_lines, log.returned_lines), (5, 2));
    assert!(log.truncated);
    assert!(o.get_log("nope", &Default::default()).is_none());
}

#[test]
fn legacy_logs_without_meta_are_backfilled() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("legacy001.jsonl"),
        "{\"session_id\":\"legacy-session-1\"}\n{\"type\":\"message\",\"text\":\"hello\"}\n",
    )
    .unwrap();
    let o = HeadlessOrchestrator::new(dir.path());
    let run = o.get("legacy001").unwrap();
    assert_eq!(run.session_id.as_deref(), Some("legacy-session-1"));
    assert_eq!(run.prompt, "(legacy run)");
    assert_eq!(run.status, HeadlessRunStatus::Completed);
    assert!(dir.path().join("legacy001.meta.json").exists());
}

#[test]
fn a_run_left_running_by_a_dead_daemon_becomes_errored() {
    let dir = tempfile::tempdir().unwrap();
    let run = HeadlessRun {
        id: "stale001".into(),
        status: HeadlessRunStatus::Running,
        started_at: "2026-02-07T10:00:00.000Z".into(),
        ..Default::default()
    };
    std::fs::write(
        dir.path().join("stale001.meta.json"),
        serde_json::json!({"schemaVersion": 1, "run": run}).to_string(),
    )
    .unwrap();
    let o = HeadlessOrchestrator::new(dir.path());
    let run = o.get("stale001").unwrap();
    assert_eq!(run.status, HeadlessRunStatus::Errored);
    assert!(run.error.unwrap().contains("Interrupted"));
}

#[test]
fn list_is_newest_first_and_limited() {
    let dir = tempfile::tempdir().unwrap();
    for (id, at) in [
        ("a", "2026-01-01T00:00:00.000Z"),
        ("b", "2026-03-01T00:00:00.000Z"),
    ] {
        let run = HeadlessRun {
            id: id.into(),
            status: HeadlessRunStatus::Completed,
            started_at: at.into(),
            ..Default::default()
        };
        std::fs::write(
            dir.path().join(format!("{id}.meta.json")),
            serde_json::json!({"schemaVersion": 1, "run": run}).to_string(),
        )
        .unwrap();
    }
    let o = HeadlessOrchestrator::new(dir.path());
    let ids: Vec<_> = o
        .list(&Default::default())
        .into_iter()
        .map(|r| r.id)
        .collect();
    assert_eq!(ids, ["b", "a"]);
    let one = o.list(&ListHeadlessRunsOptions {
        limit: Some(1),
        ..Default::default()
    });
    assert_eq!(one[0].id, "b");
}

#[tokio::test]
async fn completed_run_logs_stdout_and_captures_the_session() {
    let dir = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let o = scripted(
        dir.path(),
        r#"echo '{"type":"system","session_id":"sess-42"}'; echo '{"type":"result","result":"ok"}'; echo oops >&2"#,
        r#"echo {"type":"system","session_id":"sess-42"} & echo {"type":"result","result":"ok"} & echo oops 1>&2"#,
    );
    let mut events = o.subscribe();
    let started = o.start(payload(&work.path().to_string_lossy()));
    assert_eq!(started.status, HeadlessRunStatus::Running);
    let run = wait_for_status(&o, &started.id, HeadlessRunStatus::Completed).await;
    assert_eq!(run.session_id.as_deref(), Some("sess-42"));
    assert!(run.ended_at.is_some());
    let log = o.get_log(&run.id, &Default::default()).unwrap();
    assert!(log.content.contains("sess-42"), "{}", log.content);
    assert!(
        log.content.contains("\"type\":\"stderr\""),
        "{}",
        log.content
    );
    let mut outputs = 0;
    let mut finished = false;
    while let Ok(ev) = events.try_recv() {
        match ev {
            HeadlessEvent::Output { .. } => outputs += 1,
            HeadlessEvent::Changed(r) => finished |= r.status == HeadlessRunStatus::Completed,
        }
    }
    assert!(outputs >= 2 && finished);
    // The index survives a restart.
    let again = HeadlessOrchestrator::new(dir.path());
    assert_eq!(
        again.get(&run.id).unwrap().status,
        HeadlessRunStatus::Completed
    );
}

#[tokio::test]
async fn a_failing_cli_is_errored_with_its_stderr() {
    let dir = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let o = scripted(
        dir.path(),
        "echo boom >&2; exit 3",
        "echo boom 1>&2 & exit 3",
    );
    let started = o.start(payload(&work.path().to_string_lossy()));
    let run = wait_for_status(&o, &started.id, HeadlessRunStatus::Errored).await;
    let error = run.error.unwrap();
    assert!(
        error.contains("code 3") && error.contains("boom"),
        "{error}"
    );
}

#[tokio::test]
async fn a_missing_command_is_errored() {
    let dir = tempfile::tempdir().unwrap();
    let o = HeadlessOrchestrator::with_override(
        dir.path(),
        Some(Arc::new(|_| {
            Some(SpawnSpec {
                cmd: "definitely-not-a-real-command-xyz".into(),
                args: vec![],
            })
        })),
    );
    let started = o.start(payload(""));
    let run = wait_for_status(&o, &started.id, HeadlessRunStatus::Errored).await;
    assert!(run.error.is_some());
}

#[tokio::test]
async fn cancel_stops_the_whole_process_tree() {
    let dir = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let o = scripted(
        dir.path(),
        "echo started; sleep 30 & sleep 30",
        "echo started & ping -n 30 127.0.0.1 >nul",
    );
    let started = o.start(payload(&work.path().to_string_lossy()));
    // Wait for the first line so the process is really up.
    for _ in 0..100 {
        if o.get_log(&started.id, &Default::default())
            .is_some_and(|l| l.content.contains("started"))
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(o.cancel(&started.id));
    let run = wait_for_status(&o, &started.id, HeadlessRunStatus::Canceled).await;
    assert_eq!(run.error, None);
    // Finished runs cannot be canceled again.
    assert!(!o.cancel(&started.id));
    assert!(!o.cancel("unknown"));
}
