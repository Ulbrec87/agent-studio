use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};

use super::*;
use crate::overview::git_info::GitInfo;
use crate::overview::status::STALE_AFTER_MS;
use crate::overview::types::{
    ClaudeRegistryStatusFields, EventKind, LastMessage, OverviewStatus as S, WaitingReason,
};
use crate::types::AgentStatus;

#[derive(Default)]
struct World {
    agents: Vec<AgentState>,
    activity: HashMap<String, AgentActivity>,
    entries: Vec<RegistryEntry>,
    table: ProcessTable,
}

#[derive(Clone)]
struct FakeAgents(Arc<Mutex<World>>);
impl OverviewAgentSource for FakeAgents {
    fn list(&self) -> Vec<AgentState> {
        self.0.lock().agents.clone()
    }
    fn activity(&self, id: &str) -> Option<AgentActivity> {
        self.0.lock().activity.get(id).copied()
    }
}

fn iso(ms: i64) -> String {
    DateTime::from_timestamp_millis(ms).unwrap().to_rfc3339()
}

fn agent(id: &str) -> AgentState {
    let mut a = AgentState::default();
    a.config.id = id.into();
    a.config.name = "agent".into();
    a.config.project_dir = "/work/proj".into();
    a.config.provider = ProviderId::Codex;
    a.config.model = "gpt".into();
    a.config.worktree_branch = Some("feat/x".into());
    a.status = AgentStatus::Running;
    a.pid = Some(100);
    a.started_at = Some(iso(1_000));
    a
}
fn claude(id: &str, session: Option<&str>) -> AgentState {
    let mut a = agent(id);
    a.config.provider = ProviderId::Claude;
    a.config.session_id = session.map(str::to_string);
    a
}

fn entry(pid: u32, session: &str, status: &str, waiting: Option<&str>) -> RegistryEntry {
    RegistryEntry {
        file: PathBuf::from("f"),
        config_dir: PathBuf::from("/cfg"),
        pid,
        session_id: Some(session.into()),
        cwd: None,
        started_at: None,
        kind: None,
        name: None,
        updated_at: None,
        status_updated_at: None,
        fields: ClaudeRegistryStatusFields {
            status: Some(status.into()),
            state: None,
            waiting_for: waiting.map(str::to_string),
        },
    }
}

struct Fixture {
    world: Arc<Mutex<World>>,
    clock: Arc<AtomicI64>,
    registry_calls: Arc<AtomicUsize>,
    table_calls: Arc<AtomicUsize>,
    registry_dirs: Arc<Mutex<Vec<Vec<PathBuf>>>>,
    tracker: OverviewTracker,
}

impl Fixture {
    fn now(&self) -> i64 {
        self.clock.load(Ordering::SeqCst)
    }
    fn advance(&self, ms: i64) {
        self.clock.fetch_add(ms, Ordering::SeqCst);
    }
    fn set_agents(&self, agents: Vec<AgentState>) {
        self.world.lock().agents = agents;
    }
    fn output(&self, id: &str, last_output_at: i64, exit: Option<i32>) {
        self.world.lock().activity.insert(
            id.to_string(),
            AgentActivity {
                last_output_at: u64::try_from(last_output_at).unwrap(),
                last_exit_code: exit,
            },
        );
    }
    fn entries(&self, entries: Vec<RegistryEntry>) {
        self.world.lock().entries = entries;
    }
}

type DepsParts = (
    TrackerDeps,
    Arc<AtomicUsize>,
    Arc<AtomicUsize>,
    Arc<Mutex<Vec<Vec<PathBuf>>>>,
);

fn deps(world: &Arc<Mutex<World>>, clock: &Arc<AtomicI64>) -> DepsParts {
    let registry_calls = Arc::new(AtomicUsize::new(0));
    let table_calls = Arc::new(AtomicUsize::new(0));
    let dirs_seen = Arc::new(Mutex::new(Vec::new()));
    let (w1, w2) = (world.clone(), world.clone());
    let (rc, tc, ds) = (
        registry_calls.clone(),
        table_calls.clone(),
        dirs_seen.clone(),
    );
    let c = clock.clone();
    let deps = TrackerDeps {
        agents: Box::new(FakeAgents(world.clone())),
        registry: Box::new(move |dirs| {
            rc.fetch_add(1, Ordering::SeqCst);
            ds.lock().push(dirs.to_vec());
            w1.lock().entries.clone()
        }),
        process_table: Box::new(move |_| {
            tc.fetch_add(1, Ordering::SeqCst);
            w2.lock().table.clone()
        }),
        config_dirs: Box::new(|| vec![PathBuf::from("/cfg")]),
        waiting_text: None,
        git: None,
        details: None,
        history: None,
        now: Box::new(move || c.load(Ordering::SeqCst)),
        sample_interval_ms: 5000,
    };
    (deps, registry_calls, table_calls, dirs_seen)
}

fn fixture_with(customize: impl FnOnce(&mut TrackerDeps)) -> Fixture {
    let world = Arc::new(Mutex::new(World::default()));
    let clock = Arc::new(AtomicI64::new(1_000_000));
    let (mut d, registry_calls, table_calls, registry_dirs) = deps(&world, &clock);
    customize(&mut d);
    Fixture {
        world,
        clock,
        registry_calls,
        table_calls,
        registry_dirs,
        tracker: OverviewTracker::new(d),
    }
}
fn fixture() -> Fixture {
    fixture_with(|_| {})
}

#[test]
fn derives_working_then_idle_from_output_silence_for_non_claude_agents() {
    let mut f = fixture();
    f.set_agents(vec![agent("a1")]);
    f.output("a1", f.now() - 1000, None);
    let snap = f.tracker.refresh().snapshot;
    assert_eq!(snap.sessions[0].status, S::Working);
    assert_eq!(snap.sessions[0].started_at, 1_000);
    assert_eq!(snap.sessions[0].worktree_branch.as_deref(), Some("feat/x"));
    assert_eq!(snap.counts.working, 1);

    f.advance(10_000);
    let snap = f.tracker.refresh().snapshot;
    assert_eq!(snap.sessions[0].status, S::Idle);
    assert_eq!(snap.sessions[0].status_since, f.now());
}

#[test]
fn a_waiting_claude_registry_entry_is_needs_you_with_its_reason() {
    let mut f = fixture();
    f.set_agents(vec![claude("a1", Some("s1"))]);
    f.output("a1", f.now(), None);
    f.entries(vec![entry(7, "s1", "waiting", Some("permission prompt"))]);
    let snap = f.tracker.refresh().snapshot;
    assert_eq!(snap.sessions[0].status, S::NeedsYou);
    assert_eq!(
        snap.sessions[0].waiting_reason,
        Some(WaitingReason::Permission)
    );
    assert_eq!(snap.counts.needs_you, 1);
    assert_eq!(*f.registry_dirs.lock(), [vec![PathBuf::from("/cfg")]]);
    assert!(f.tracker.has_registry_match("a1"));
}

fn bash_line(cmd: &str) -> String {
    serde_json::json!({"type":"assistant","message":{"id":"m1","content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":cmd}}]}}).to_string()
}

#[test]
fn waiting_text_is_set_for_a_needs_you_claude_session_and_cleared_when_it_stops_waiting() {
    let tmp = tempfile::tempdir().unwrap();
    let projects = tmp.path().join("projects");
    std::fs::create_dir_all(projects.join("p")).unwrap();
    let transcript = projects.join("p/s1.jsonl");
    std::fs::write(&transcript, bash_line("npm test") + "\n").unwrap();
    let dirs = vec![projects.clone()];
    let mut f = fixture_with(|d| {
        d.waiting_text = Some(WaitingTextReader::new(Box::new(move || dirs.clone())))
    });
    f.set_agents(vec![claude("a1", Some("s1"))]);
    f.output("a1", f.now(), None);
    f.entries(vec![entry(7, "s1", "waiting", Some("permission prompt"))]);
    let first = f.tracker.refresh();
    assert_eq!(
        first.snapshot.sessions[0].waiting_text.as_deref(),
        Some("Run: npm test")
    );
    assert!(first.changed);

    std::fs::write(&transcript, bash_line("npm other") + "\n").unwrap();
    let second = f.tracker.refresh();
    assert_eq!(
        second.snapshot.sessions[0].waiting_text.as_deref(),
        Some("Run: npm other")
    );
    assert!(second.changed);

    f.entries(vec![entry(7, "s1", "busy", None)]);
    let third = f.tracker.refresh();
    assert_eq!(third.snapshot.sessions[0].waiting_text, None);
}

#[test]
fn keeps_status_since_while_the_status_is_unchanged() {
    let mut f = fixture();
    f.set_agents(vec![agent("a1")]);
    f.output("a1", 0, None);
    let first = f.tracker.refresh().snapshot;
    f.advance(60_000);
    let second = f.tracker.refresh().snapshot;
    assert_eq!(second.sessions[0].status, S::Idle);
    assert_eq!(
        second.sessions[0].status_since,
        first.sessions[0].status_since
    );
}

#[test]
fn flags_stale_idle_sessions_and_quiet_working_ones() {
    let mut f = fixture();
    f.set_agents(vec![agent("idle"), claude("quiet", Some("q"))]);
    f.output("idle", 0, None);
    f.output("quiet", f.now(), None);
    f.entries(vec![entry(7, "q", "busy", None)]);
    f.tracker.refresh();
    f.advance(STALE_AFTER_MS);
    f.output("quiet", f.now() - 6 * 60_000, None);
    let snap = f.tracker.refresh().snapshot;
    let idle = snap.sessions.iter().find(|s| s.agent_id == "idle").unwrap();
    let quiet = snap
        .sessions
        .iter()
        .find(|s| s.agent_id == "quiet")
        .unwrap();
    assert_eq!((idle.status, idle.stale), (S::Idle, true));
    assert_eq!(snap.counts.stale, 1);
    assert_eq!(quiet.status, S::Working);
    assert_eq!(quiet.quiet_for_ms, Some(6 * 60_000));
}

#[test]
fn maps_finished_and_failed_from_the_process_status() {
    let mut f = fixture();
    let mut done = agent("f");
    done.status = AgentStatus::Idle;
    done.pid = None;
    let mut bad = agent("e");
    bad.status = AgentStatus::Errored;
    bad.pid = None;
    f.set_agents(vec![done, bad]);
    f.output("f", 0, Some(0));
    let snap = f.tracker.refresh().snapshot;
    assert_eq!(
        snap.sessions.iter().map(|s| s.status).collect::<Vec<_>>(),
        [S::Finished, S::Failed]
    );
}

#[test]
fn restored_agents_are_not_running_since_their_ended_at_falling_back_to_last_activity() {
    let mut f = fixture();
    let restored = |id: &str, ended: Option<String>, activity: &str| {
        let mut a = agent(id);
        a.status = AgentStatus::Idle;
        a.pid = None;
        a.ended_at = ended;
        a.last_activity_at = activity.into();
        a
    };
    f.set_agents(vec![
        restored("r1", Some(iso(500_000)), ""),
        restored("r2", None, &iso(700_000)),
        restored("r3", None, "garbage"),
    ]);
    let snap = f.tracker.refresh().snapshot;
    assert!(snap.sessions.iter().all(|s| s.status == S::NotRunning));
    assert_eq!(
        snap.sessions
            .iter()
            .map(|s| s.status_since)
            .collect::<Vec<_>>(),
        [500_000, 700_000, f.now()]
    );
    assert_eq!(snap.sessions[0].ended_at, Some(500_000));
    assert_eq!(snap.sessions[1].ended_at, Some(700_000));
    assert_eq!(snap.sessions[2].ended_at, None);
    assert_eq!(snap.sessions[0].exit_code, None);
    assert_eq!((snap.counts.not_running, snap.counts.open), (3, 0));
}

#[test]
fn an_agent_that_exits_during_the_run_is_finished_since_its_end_with_the_exit_code() {
    let mut f = fixture();
    f.set_agents(vec![agent("a1")]);
    f.output("a1", f.now(), None);
    f.tracker.refresh();
    f.advance(60_000);
    let ended_at = f.now() - 5_000;
    let mut done = agent("a1");
    done.status = AgentStatus::Idle;
    done.pid = None;
    done.ended_at = Some(iso(ended_at));
    f.set_agents(vec![done]);
    f.output("a1", f.now() - 5_000, Some(2));
    let s = &f.tracker.refresh().snapshot.sessions[0];
    assert_eq!(
        (s.status, s.status_since, s.ended_at, s.exit_code),
        (S::Finished, ended_at, Some(ended_at), Some(2))
    );
    f.advance(30_000);
    assert_eq!(
        f.tracker.refresh().snapshot.sessions[0].status_since,
        ended_at
    );
}

#[test]
fn status_since_is_never_in_the_future() {
    let mut f = fixture();
    let mut a = agent("a1");
    a.status = AgentStatus::Idle;
    a.pid = None;
    a.ended_at = Some(iso(f.now() + 10_000));
    f.set_agents(vec![a]);
    assert_eq!(
        f.tracker.refresh().snapshot.sessions[0].status_since,
        f.now()
    );
}

#[test]
fn reports_a_change_only_when_something_meaningful_changed() {
    let mut f = fixture();
    f.set_agents(vec![agent("a1")]);
    f.output("a1", 0, None);
    assert!(f.tracker.refresh().changed);
    f.advance(30_000);
    assert!(
        !f.tracker.refresh().changed,
        "time passing alone is not a change"
    );
    f.advance(2_000);
    assert!(!f.tracker.refresh().changed);
    f.output("a1", f.now(), None);
    let poll = f.tracker.refresh();
    assert!(poll.changed);
    assert_eq!(poll.snapshot.sessions[0].status, S::Working);
}

#[test]
fn forgets_removed_agents_and_restarts_their_status_since_when_they_come_back() {
    let mut f = fixture();
    f.set_agents(vec![agent("a1")]);
    f.output("a1", 0, None);
    f.tracker.refresh();
    f.set_agents(vec![]);
    let empty = f.tracker.refresh().snapshot;
    assert!(empty.sessions.is_empty());
    assert_eq!(empty.counts.total, 0);
    f.advance(5000);
    f.set_agents(vec![agent("a1")]);
    assert_eq!(
        f.tracker.refresh().snapshot.sessions[0].status_since,
        f.now()
    );
}

#[test]
fn reads_the_process_table_only_for_claude_agents_the_cheap_tiers_cannot_match() {
    let mut f = fixture();
    f.set_agents(vec![claude("a1", Some("s1"))]);
    f.output("a1", f.now(), None);
    f.entries(vec![entry(7, "s1", "busy", None)]);
    f.tracker.refresh();
    assert_eq!(f.table_calls.load(Ordering::SeqCst), 0);

    // No session id, and the registry pid is a grandchild of the agent pid (the PTY child is the
    // login shell wrapping claude).
    let mut a = claude("a1", None);
    a.pid = Some(100);
    f.set_agents(vec![a]);
    f.entries(vec![entry(205, "x", "waiting", Some("input needed"))]);
    f.world.lock().table = HashMap::from([(205, 150), (150, 100)]);
    let snap = f.tracker.refresh().snapshot;
    assert_eq!(f.table_calls.load(Ordering::SeqCst), 1);
    assert_eq!(snap.sessions[0].status, S::NeedsYou);
    assert_eq!(
        snap.sessions[0].waiting_reason,
        Some(WaitingReason::Question)
    );
}

#[test]
fn does_not_read_the_registry_or_process_table_when_no_claude_agent_runs() {
    let mut f = fixture();
    f.set_agents(vec![agent("a1")]);
    f.tracker.refresh();
    assert_eq!(f.registry_calls.load(Ordering::SeqCst), 0);
    assert_eq!(f.table_calls.load(Ordering::SeqCst), 0);
}

#[test]
fn get_snapshot_polls_once_when_there_is_no_snapshot_yet() {
    let mut f = fixture();
    f.set_agents(vec![agent("a1")]);
    assert!(f.tracker.peek_snapshot().is_none());
    assert_eq!(f.tracker.get_snapshot().sessions.len(), 1);
    f.set_agents(vec![]);
    assert_eq!(f.tracker.get_snapshot().sessions.len(), 1, "no second poll");
}

#[test]
fn adds_repository_branch_and_commit_and_pushes_when_they_change() {
    let info = Arc::new(Mutex::new(GitInfo {
        repo_root: PathBuf::from("/work/proj"),
        repo_name: "proj".into(),
        branch: Some("main".into()),
        commit: None,
    }));
    let i = info.clone();
    let mut f = fixture_with(|d| {
        d.git = Some(GitInfoCache::new(
            Box::new(move |_| Some(i.lock().clone())),
            0,
        ))
    });
    f.set_agents(vec![agent("a1")]);
    let snap = f.tracker.refresh().snapshot;
    assert_eq!(snap.sessions[0].repo_name.as_deref(), Some("proj"));
    assert_eq!(snap.sessions[0].branch.as_deref(), Some("main"));
    assert_eq!(snap.sessions[0].commit, None);
    {
        let mut g = info.lock();
        g.branch = None;
        g.commit = Some("abc1234".into());
    }
    f.advance(1);
    let poll = f.tracker.refresh();
    assert_eq!(poll.snapshot.sessions[0].commit.as_deref(), Some("abc1234"));
    assert_eq!(poll.snapshot.sessions[0].branch, None);
    assert!(poll.changed);
}

#[test]
fn reads_details_on_demand_for_known_agents_only() {
    let tmp = tempfile::tempdir().unwrap();
    let projects = tmp.path().join("projects");
    std::fs::create_dir_all(projects.join("p")).unwrap();
    std::fs::write(
        projects.join("p/s1.jsonl"),
        serde_json::json!({"type":"assistant","message":{"id":"m1","content":[{"type":"text","text":"hi"}]}}).to_string() + "\n",
    )
    .unwrap();
    let dirs = vec![projects.clone()];
    let mut f =
        fixture_with(|d| d.details = Some(AgentDetailsReader::new(Box::new(move || dirs.clone()))));
    f.set_agents(vec![claude("a1", Some("s1"))]);
    f.output("a1", f.now(), None);
    f.entries(vec![entry(7, "s1", "busy", None)]);
    assert!(f.tracker.get_details("a1").is_none());
    f.tracker.refresh();
    assert_eq!(
        f.tracker.get_details("a1").unwrap().last_message,
        LastMessage::Text {
            text: "hi".into(),
            truncated: false
        }
    );
    assert!(f.tracker.get_details("nope").is_none());
}

// ── history ─────────────────────────────────────────────────────────────────

#[derive(Default)]
struct SinkData {
    points: Vec<HistoryPoint>,
    events: Vec<OverviewEvent>,
    last_point_at: Option<i64>,
}
#[derive(Clone, Default)]
struct FakeSink(Arc<Mutex<SinkData>>);
impl HistorySink for FakeSink {
    fn add_events(&self, events: &[OverviewEvent]) {
        self.0.lock().events.extend_from_slice(events);
    }
    fn add_point(&self, point: HistoryPoint, _agents: &BTreeMap<String, HistoryAgent>) {
        let mut d = self.0.lock();
        d.last_point_at = Some(point.at);
        d.points.push(point);
    }
    fn last_point_at(&self) -> Option<i64> {
        self.0.lock().last_point_at
    }
}

fn with_history() -> (Fixture, FakeSink) {
    let sink = FakeSink::default();
    let s = sink.clone();
    let f = fixture_with(|d| d.history = Some(Box::new(s)));
    f.set_agents(vec![agent("a1")]);
    f.output("a1", f.now(), None);
    (f, sink)
}

#[test]
fn history_records_no_event_for_agents_that_exist_at_the_first_poll_and_a_point() {
    let (mut f, sink) = with_history();
    f.tracker.refresh();
    let d = sink.0.lock();
    assert!(d.events.is_empty());
    assert_eq!(d.points.len(), 1);
    assert_eq!(
        (d.points[0].at, d.points[0].working, d.points[0].total),
        (f.now(), 1, 1)
    );
    assert_eq!(d.points[0].statuses["a1"], S::Working);
}

#[test]
fn history_records_status_changes_appeared_and_ended_agents() {
    let (mut f, sink) = with_history();
    f.tracker.refresh();
    f.advance(10_000);
    let mut second = agent("a2");
    second.name = "second".into();
    f.set_agents(vec![agent("a1"), second]);
    f.output("a2", f.now(), None);
    let poll = f.tracker.refresh(); // a1 went idle, a2 appeared
    let recorded: Vec<(String, EventKind)> = sink
        .0
        .lock()
        .events
        .iter()
        .map(|e| (e.agent_id.clone(), e.kind))
        .collect();
    assert_eq!(
        recorded,
        [
            ("a1".to_string(), EventKind::Status),
            ("a2".to_string(), EventKind::Appeared)
        ]
    );
    assert_eq!(poll.events, sink.0.lock().events);
    f.advance(2000);
    let mut only_second = agent("a2");
    only_second.name = "second".into();
    f.set_agents(vec![only_second]);
    let poll = f.tracker.refresh();
    assert_eq!(poll.events.len(), 1);
    assert_eq!(
        (poll.events[0].agent_id.as_str(), poll.events[0].kind),
        ("a1", EventKind::Ended)
    );
}

#[test]
fn history_samples_every_5_seconds_not_on_every_poll() {
    let (mut f, sink) = with_history();
    f.tracker.refresh();
    f.advance(2000);
    f.tracker.refresh();
    f.advance(2000);
    f.tracker.refresh();
    assert_eq!(sink.0.lock().points.len(), 1);
    f.advance(2000);
    f.tracker.refresh();
    assert_eq!(sink.0.lock().points.len(), 2);
}

#[test]
fn history_adds_a_resumed_event_after_a_gap_of_more_than_15_seconds() {
    let (mut f, sink) = with_history();
    sink.0.lock().last_point_at = Some(f.now() - 120_000); // restored from disk
    let poll = f.tracker.refresh();
    assert_eq!(poll.events.len(), 1);
    assert_eq!(poll.events[0].kind, EventKind::Resumed);
    assert_eq!(poll.events[0].gap_ms, Some(120_000));
    assert_eq!(poll.events[0].at, f.now());
    f.advance(6000);
    f.tracker.refresh();
    assert_eq!(
        sink.0
            .lock()
            .events
            .iter()
            .filter(|e| e.kind == EventKind::Resumed)
            .count(),
        1
    );
}

#[test]
fn production_wiring_builds_and_polls_an_empty_manager() {
    // A real history store on a temp dir: the whole chain runs once without agents.
    let tmp = tempfile::tempdir().unwrap();
    let store = Arc::new(Mutex::new(HistoryStore::new(
        super::super::history_store::HistoryStoreOptions::new(tmp.path().join("h")),
    )));
    store.lock().start();
    let world = Arc::new(Mutex::new(World::default()));
    let mut tracker = OverviewTracker::new(TrackerDeps::production(
        Box::new(FakeAgents(world)),
        Box::new(store.clone()),
    ));
    let poll = tracker.refresh();
    assert!(poll.snapshot.sessions.is_empty());
    assert_eq!(history_query(&store, 0).points.len(), 1);
    store.lock().stop();
}
