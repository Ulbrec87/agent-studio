use std::collections::BTreeSet;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

use super::*;
use crate::overview::history_files::PathInfo;
use crate::overview::{MeasuredSpan, WaitDirection};
use OverviewStatus as S;

const DIR: &str = "/h";
const DAY: i64 = 24 * 60 * 60 * 1000;

fn t0() -> i64 {
    chrono::DateTime::parse_from_rfc3339("2026-10-22T10:00:00.000Z")
        .unwrap()
        .timestamp_millis()
}

/// A folder in memory. `others` are names that exist but are not ordinary files (links, pipes).
#[derive(Default, Clone)]
struct MemoryFs {
    files: Arc<Mutex<BTreeMap<String, String>>>,
    others: Arc<Mutex<BTreeSet<String>>>,
    folders: Arc<Mutex<Vec<PathBuf>>>,
    fail_appends: Arc<Mutex<bool>>,
}

impl MemoryFs {
    fn name(file: &Path) -> String {
        file.file_name().unwrap().to_string_lossy().into_owned()
    }
    fn set(&self, name: &str, content: &str) {
        self.files
            .lock()
            .unwrap()
            .insert(name.to_string(), content.to_string());
    }
    fn get(&self, name: &str) -> Option<String> {
        self.files.lock().unwrap().get(name).cloned()
    }
    fn lines(&self, name: &str) -> Vec<String> {
        self.get(name)
            .unwrap_or_default()
            .split('\n')
            .filter(|l| !l.is_empty())
            .map(str::to_string)
            .collect()
    }
}

impl HistoryFs for MemoryFs {
    fn make_folder(&self, dir: &Path) -> std::io::Result<()> {
        self.folders.lock().unwrap().push(dir.to_path_buf());
        Ok(())
    }
    fn list(&self, _dir: &Path) -> std::io::Result<Vec<String>> {
        let mut names: Vec<String> = self.files.lock().unwrap().keys().cloned().collect();
        names.extend(self.others.lock().unwrap().iter().cloned());
        Ok(names)
    }
    fn stat(&self, file: &Path) -> std::io::Result<Option<PathInfo>> {
        let n = Self::name(file);
        if self.others.lock().unwrap().contains(&n) {
            return Ok(Some(PathInfo {
                kind: PathKind::Other,
                size: 0,
            }));
        }
        Ok(self.files.lock().unwrap().get(&n).map(|c| PathInfo {
            kind: PathKind::File,
            size: c.len() as u64,
        }))
    }
    fn read(&self, file: &Path, max_bytes: u64) -> std::io::Result<String> {
        match self.files.lock().unwrap().get(&Self::name(file)) {
            Some(c) if c.len() as u64 <= max_bytes => Ok(c.clone()),
            _ => Err(std::io::Error::other("unreadable")),
        }
    }
    fn append(&self, file: &Path, text: &str) -> std::io::Result<()> {
        if *self.fail_appends.lock().unwrap() {
            return Err(std::io::Error::other("disk full"));
        }
        let n = Self::name(file);
        if self.others.lock().unwrap().contains(&n) {
            return Err(std::io::Error::other("not ordinary"));
        }
        self.files
            .lock()
            .unwrap()
            .entry(n)
            .or_default()
            .push_str(text);
        Ok(())
    }
    fn remove(&self, file: &Path) -> std::io::Result<()> {
        self.files.lock().unwrap().remove(&Self::name(file));
        Ok(())
    }
}

fn point(at: i64, statuses: &[(&str, S)]) -> HistoryPoint {
    HistoryPoint {
        at,
        needs_you: 0,
        working: 1,
        idle: 0,
        total: 1,
        statuses: statuses
            .iter()
            .map(|(k, v)| ((*k).to_string(), *v))
            .collect(),
    }
}
fn p(at: i64) -> HistoryPoint {
    point(at, &[("a", S::Working)])
}

fn event(at: i64) -> OverviewEvent {
    OverviewEvent {
        id: format!("a@{at}:status"),
        at,
        kind: EventKind::Status,
        agent_id: "a".into(),
        name: "alpha".into(),
        project_dir: "/w".into(),
        from: None,
        to: None,
        waiting_reason: None,
        waited_ms: None,
        gap_ms: None,
    }
}
fn ev(at: i64, from: Option<S>, to: Option<S>) -> OverviewEvent {
    OverviewEvent {
        from,
        to,
        ..event(at)
    }
}

struct Setup {
    store: HistoryStore,
    clock: Arc<AtomicI64>,
}

fn setup_with(
    fs: &MemoryFs,
    now: i64,
    budget: Option<(Box<dyn Fn() -> i64 + Send>, i64)>,
) -> Setup {
    let clock = Arc::new(AtomicI64::new(now));
    let c = clock.clone();
    let mut options = HistoryStoreOptions::new(PathBuf::from(DIR));
    options.fs = Box::new(fs.clone());
    options.now = Box::new(move || c.load(Ordering::SeqCst));
    if let Some((clock, ms)) = budget {
        options.budget_clock = clock;
        options.restore_budget_ms = ms;
    }
    Setup {
        store: HistoryStore::new(options),
        clock,
    }
}
fn setup(fs: &MemoryFs, now: i64) -> Setup {
    setup_with(fs, now, None)
}

const FILE: &str = "v1-2026-10-22.jsonl";

// ── memory ──────────────────────────────────────────────────────────────────

#[test]
fn keeps_at_most_1000_events_and_4320_points() {
    let mut s = setup(&MemoryFs::default(), t0()).store;
    s.start();
    let events: Vec<OverviewEvent> = (0..MAX_EVENTS as i64 + 5)
        .map(|i| event(t0() + i))
        .collect();
    s.add_events(&events);
    for i in 0..MAX_POINTS as i64 + 3 {
        s.add_point(p(t0() + i * 5000), None);
    }
    let res = s.query(0);
    assert_eq!(res.events.len(), MAX_EVENTS);
    assert_eq!(res.events[0].at, t0() + 5);
    assert_eq!(res.points.len(), MAX_POINTS);
    assert_eq!(res.since, res.points[0].at); // older points were dropped, so the history begins here
}

#[test]
fn answers_from_a_time_on_with_names_of_the_agents_it_mentions() {
    let mut s = setup(&MemoryFs::default(), t0()).store;
    s.start();
    let agents: BTreeMap<String, HistoryAgent> = [
        ("a", "alpha", "/w"),
        ("gone", "old", "/o"),
        ("unrelated", "zzz", "/z"),
    ]
    .iter()
    .map(|(id, n, d)| {
        (
            (*id).to_string(),
            HistoryAgent {
                name: (*n).into(),
                project_dir: (*d).into(),
            },
        )
    })
    .collect();
    s.add_point(
        point(t0() + 1000, &[("a", S::Working), ("gone", S::Idle)]),
        Some(&agents),
    );
    s.add_point(
        point(t0() + 6000, &[("a", S::Working)]),
        Some(&BTreeMap::new()),
    );
    let res = s.query(t0() + 2000);
    assert_eq!(
        res.points.iter().map(|x| x.at).collect::<Vec<_>>(),
        [t0() + 6000]
    );
    assert_eq!(res.agents.keys().collect::<Vec<_>>(), ["a"]);
    assert_eq!(res.started_at, t0());
    assert!(res.starts.is_empty());
    assert_eq!(s.query(0).starts, [t0()]);
    assert_eq!(s.last_point_at(), Some(t0() + 6000));
}

// ── disk ────────────────────────────────────────────────────────────────────

#[test]
fn writes_a_start_record_points_and_events_flushing_on_stop() {
    let fs = MemoryFs::default();
    let mut s = setup(&fs, t0()).store;
    s.start();
    s.add_point(p(t0() + 5000), None);
    s.add_events(&[ev(t0() + 5000, Some(S::Working), Some(S::Idle))]);
    s.stop();
    assert_eq!(*fs.folders.lock().unwrap(), [PathBuf::from(DIR)]);
    let at = t0() + 5000;
    assert_eq!(
        fs.lines(FILE),
        [
            format!("{{\"start\":{}}}", t0()),
            format!("{{\"point\":[{at},0,1,0,1,{{\"a\":\"working\"}}]}}"),
            format!(
                "{{\"event\":{{\"id\":\"a@{at}:status\",\"at\":{at},\"kind\":\"status\",\"agentId\":\"a\",\"name\":\"alpha\",\"projectDir\":\"/w\",\"from\":\"working\",\"to\":\"idle\"}}}}"
            ),
        ]
    );
}

#[test]
fn flushes_on_demand_not_on_every_add() {
    let fs = MemoryFs::default();
    let mut s = setup(&fs, t0()).store;
    s.start();
    s.add_point(p(t0()), None);
    assert_eq!(fs.lines(FILE).len(), 1); // only the start record
    s.flush();
    assert_eq!(fs.lines(FILE).len(), 2);
    s.stop();
}

#[test]
fn restores_the_newest_events_and_points_and_records_the_start() {
    let fs = MemoryFs::default();
    let mut first = setup(&fs, t0()).store;
    first.start();
    first.add_events(&[ev(t0() + 1000, None, Some(S::Idle))]);
    let agents = BTreeMap::from([(
        "a".to_string(),
        HistoryAgent {
            name: "alpha".into(),
            project_dir: "/w".into(),
        },
    )]);
    first.add_point(p(t0() + 5000), Some(&agents));
    first.stop();

    let mut second = setup(&fs, t0() + 60_000).store;
    second.start();
    let res = second.query(0);
    assert_eq!(
        res.points.iter().map(|x| x.at).collect::<Vec<_>>(),
        [t0() + 5000]
    );
    assert_eq!(
        res.points[0].statuses,
        BTreeMap::from([("a".to_string(), S::Working)])
    );
    assert_eq!(
        res.events.iter().map(|e| e.at).collect::<Vec<_>>(),
        [t0() + 1000]
    );
    assert_eq!(res.starts, [t0(), t0() + 60_000]);
    assert_eq!(res.started_at, t0() + 60_000);
    assert_eq!(res.since, t0()); // the first record restored
    assert_eq!(
        res.agents["a"],
        HistoryAgent {
            name: "alpha".into(),
            project_dir: "/w".into()
        }
    );
    assert_eq!(second.last_point_at(), Some(t0() + 5000));
    assert_eq!(
        fs.lines(FILE).last().unwrap(),
        &format!("{{\"start\":{}}}", t0() + 60_000)
    );
}

#[test]
fn skips_malformed_lines_and_records_dated_more_than_a_minute_ahead() {
    let fs = MemoryFs::default();
    let t = t0();
    let content = [
        "{\"start\":".to_string(),
        "not json".to_string(),
        format!("{{\"point\":[{},0,1,0,1,{{\"a\":\"working\"}}]}}", t - 5000),
        format!("{{\"point\":[{},0,1,0,1,{{}}]}}", t + 30_000), // within the minute: kept
        format!("{{\"point\":[{},0,1,0,1,{{}}]}}", t + 120_000), // future: dropped
        "{\"point\":[\"x\",0,1,0,1]}".to_string(),
        "{\"point\":[1,2]}".to_string(),
        "{\"event\":{\"id\":\"x\"}}".to_string(),
        format!("{{\"event\":{{\"id\":\"e\",\"at\":{},\"kind\":\"bogus\",\"agentId\":\"a\",\"name\":\"n\",\"projectDir\":\"p\"}}}}", t - 1),
        format!("{{\"event\":{{\"id\":\"e\",\"at\":{},\"kind\":\"appeared\",\"agentId\":\"a\",\"name\":\"n\",\"projectDir\":\"p\",\"to\":\"nope\"}}}}", t - 1),
        "{\"other\":1}".to_string(),
        "null".to_string(),
        String::new(),
    ]
    .join("\n");
    fs.set(FILE, &content);
    let mut s = setup(&fs, t).store;
    s.start();
    let res = s.query(0);
    assert_eq!(
        res.points.iter().map(|x| x.at).collect::<Vec<_>>(),
        [t - 5000, t + 30_000]
    );
    // The event with the unknown status keeps its other fields: Hydra drops the bad `to` only.
    assert_eq!(
        res.events,
        [OverviewEvent {
            id: "e".into(),
            at: t - 1,
            kind: EventKind::Appeared,
            agent_id: "a".into(),
            name: "n".into(),
            project_dir: "p".into(),
            from: None,
            to: None,
            waiting_reason: None,
            waited_ms: None,
            gap_ms: None,
        }]
    );
}

#[test]
fn does_not_read_other_formats_links_other_file_types_or_oversize_files() {
    let fs = MemoryFs::default();
    let t = t0();
    let rec = |at: i64| format!("{{\"point\":[{at},0,1,0,1,{{}}]}}\n");
    fs.set("v2-2026-10-22.jsonl", &rec(t - 1000));
    fs.set("notes.txt", &rec(t - 2000));
    fs.set(
        "v1-2026-10-21.jsonl",
        &(rec(t - 3000) + &" ".repeat(2 * MAX_FILE_BYTES as usize + 1)),
    );
    fs.others
        .lock()
        .unwrap()
        .insert("v1-2026-10-20.jsonl".into());
    fs.set("v1-2026-10-19.jsonl", &rec(t - 4000));
    let mut s = setup(&fs, t).store;
    s.start();
    assert_eq!(
        s.query(0).points.iter().map(|x| x.at).collect::<Vec<_>>(),
        [t - 4000]
    );
    assert!(fs.get("v2-2026-10-22.jsonl").is_some());
    assert!(fs.get("notes.txt").is_some());
}

#[test]
fn stops_restoring_when_the_time_budget_is_spent() {
    let fs = MemoryFs::default();
    for d in 0..4 {
        fs.set(
            &format!("v1-2026-10-{:02}.jsonl", 22 - d),
            &format!("{{\"point\":[{},0,1,0,1,{{}}]}}\n", t0() - d * DAY),
        );
    }
    let tick = Arc::new(AtomicI64::new(0));
    let c = tick.clone();
    let clock: Box<dyn Fn() -> i64 + Send> =
        Box::new(move || c.fetch_add(4000, Ordering::SeqCst) + 4000);
    let mut s = setup_with(&fs, t0(), Some((clock, 10_000))).store;
    s.start();
    // deadline is 4000 + 10000: checks at 8000 and 12000 pass, 16000 does not.
    assert_eq!(s.query(0).points.len(), 2);
}

#[test]
fn starts_the_first_write_to_a_non_empty_file_with_a_line_break() {
    let fs = MemoryFs::default();
    fs.set(FILE, "{\"point\":[1,0,0");
    let mut s = setup(&fs, t0()).store;
    s.start();
    s.add_point(p(t0() + 1), None);
    s.stop();
    let text = fs.get(FILE).unwrap();
    assert!(text.starts_with("{\"point\":[1,0,0\n{\"start\":"));
    assert!(!text.contains("\n\n"));
}

#[test]
fn rolls_over_to_part_2_past_2_mb_and_goes_on_in_the_next_day() {
    let fs = MemoryFs::default();
    fs.set(FILE, &(" ".repeat(MAX_FILE_BYTES as usize - 10) + "\n"));
    let Setup { mut store, clock } = setup(&fs, t0());
    store.start();
    store.add_point(p(t0() + 1000), None);
    store.stop();
    assert_eq!(fs.get(FILE).unwrap().len(), MAX_FILE_BYTES as usize - 9);
    assert_eq!(fs.lines("v1-2026-10-22-2.jsonl").len(), 2); // start (after restore) and the point

    clock.store(t0() + DAY, Ordering::SeqCst);
    store.add_point(p(t0() + DAY), None);
    store.flush();
    assert_eq!(fs.lines("v1-2026-10-23.jsonl").len(), 1);
}

#[test]
fn writes_in_the_next_file_when_the_day_file_is_not_an_ordinary_file() {
    let fs = MemoryFs::default();
    fs.others.lock().unwrap().insert(FILE.into());
    let mut s = setup(&fs, t0()).store;
    s.start();
    s.stop();
    assert_eq!(
        fs.lines("v1-2026-10-22-2.jsonl"),
        [format!("{{\"start\":{}}}", t0())]
    );
}

#[test]
fn keeps_the_lines_when_a_write_fails_and_writes_them_on_the_next_flush() {
    let fs = MemoryFs::default();
    let mut s = setup(&fs, t0()).store;
    *fs.fail_appends.lock().unwrap() = true;
    s.start();
    s.add_point(p(t0() + 1), None);
    s.flush();
    *fs.fail_appends.lock().unwrap() = false;
    s.flush();
    assert_eq!(fs.lines(FILE).len(), 2);
}

#[test]
fn deletes_days_that_ended_more_than_8_days_ago_before_writing() {
    let fs = MemoryFs::default();
    fs.set("v1-2026-10-13.jsonl", "x\n"); // ended Oct 14 00:00, 8d 10h before now
    fs.set("v1-2026-10-14.jsonl", "y\n"); // ended Oct 15 00:00, 7d 10h ago
    fs.set("v2-2026-10-01.jsonl", "z\n");
    let mut s = setup(&fs, t0()).store;
    s.start();
    let names: Vec<String> = fs.files.lock().unwrap().keys().cloned().collect();
    assert_eq!(
        names,
        [
            "v1-2026-10-14.jsonl",
            "v1-2026-10-22.jsonl",
            "v2-2026-10-01.jsonl"
        ]
    );
}

// ── status deltas ───────────────────────────────────────────────────────────

fn many(n: usize, over: &[(&str, S)]) -> Vec<(String, S)> {
    let mut v: Vec<(String, S)> = (0..n).map(|i| (format!("agent-{i}"), S::Working)).collect();
    for (id, st) in over {
        if let Some(e) = v.iter_mut().find(|(k, _)| k == id) {
            e.1 = *st;
        }
    }
    v
}
fn map_of(v: &[(String, S)]) -> BTreeMap<String, S> {
    v.iter().cloned().collect()
}
fn pt(at: i64, statuses: &[(String, S)]) -> HistoryPoint {
    HistoryPoint {
        statuses: map_of(statuses),
        ..p(at)
    }
}
fn point_lines(fs: &MemoryFs, name: &str) -> Vec<String> {
    fs.lines(name)
        .into_iter()
        .filter(|l| l.starts_with("{\"point\""))
        .collect()
}

#[test]
fn writes_the_full_map_first_then_only_what_changed_and_restores_full_maps() {
    let fs = MemoryFs::default();
    let t = t0();
    let mut first = setup(&fs, t).store;
    first.start();
    first.add_point(pt(t + 1000, &many(3, &[])), None);
    first.add_point(pt(t + 2000, &many(3, &[])), None);
    first.add_point(pt(t + 3000, &many(3, &[("agent-1", S::Idle)])), None);
    let mut gone = many(3, &[("agent-1", S::Idle)]);
    gone.remove(0);
    first.add_point(pt(t + 4000, &gone), None);
    first.stop();

    let written = point_lines(&fs, FILE);
    assert_eq!(
        written[1],
        format!("{{\"point\":[{},0,1,0,1,{{}},1]}}", t + 2000)
    );
    assert_eq!(
        written[2],
        format!(
            "{{\"point\":[{},0,1,0,1,{{\"agent-1\":\"idle\"}},1]}}",
            t + 3000
        )
    );
    assert_eq!(
        written[3],
        format!(
            "{{\"point\":[{},0,1,0,1,{{\"agent-0\":null}},1]}}",
            t + 4000
        )
    );

    let mut second = setup(&fs, t + 60_000).store;
    second.start();
    let statuses: Vec<BTreeMap<String, S>> = second
        .query(0)
        .points
        .into_iter()
        .map(|x| x.statuses)
        .collect();
    assert_eq!(
        statuses,
        [
            map_of(&many(3, &[])),
            map_of(&many(3, &[])),
            map_of(&many(3, &[("agent-1", S::Idle)])),
            map_of(&gone)
        ]
    );
}

#[test]
fn writes_a_full_point_at_the_start_of_each_run_and_of_each_new_file() {
    let fs = MemoryFs::default();
    let t = t0();
    let mut a = setup(&fs, t).store;
    a.start();
    a.add_point(pt(t + 1000, &many(2, &[])), None);
    a.stop();
    let Setup { mut store, clock } = setup(&fs, t + 10_000);
    store.start();
    store.add_point(pt(t + 11_000, &many(2, &[])), None);
    store.flush();
    clock.store(t + DAY, Ordering::SeqCst);
    store.add_point(pt(t + DAY, &many(2, &[])), None);
    store.flush();
    let full = |name: &str| {
        point_lines(&fs, name)
            .into_iter()
            .filter(|l| !l.ends_with(",1]}"))
            .count()
    };
    assert_eq!(full(FILE), 2); // first point of each run
    assert_eq!(
        fs.lines(FILE)
            .iter()
            .filter(|l| l.ends_with(",1]}"))
            .count(),
        0
    );
    assert_eq!(full("v1-2026-10-23.jsonl"), 1);
}

#[test]
fn restores_a_file_that_starts_mid_run_and_skips_deltas_with_no_full_point_before_them() {
    let fs = MemoryFs::default();
    let t = t0();
    fs.set(
        "v1-2026-10-21.jsonl",
        &([
            format!(
                "{{\"point\":[{},0,1,0,1,{{\"a\":\"idle\"}},1]}}",
                t - DAY + 1000
            ), // delta first
            format!(
                "{{\"point\":[{},0,1,0,1,{{\"a\":\"working\",\"b\":\"idle\"}}]}}",
                t - DAY + 2000
            ),
            format!(
                "{{\"point\":[{},0,1,0,1,{{\"b\":null,\"c\":\"nope\"}},1]}}",
                t - DAY + 3000
            ),
        ]
        .join("\n")
            + "\n"),
    );
    fs.set(
        FILE,
        &format!(
            "{{\"point\":[{},0,1,0,1,{{\"x\":\"idle\"}},1]}}\n{{\"point\":[{},0,1,0,1,{{\"x\":\"working\"}}]}}\n",
            t - 1000,
            t - 500
        ),
    );
    let mut s = setup(&fs, t).store;
    s.start();
    let got: Vec<(i64, BTreeMap<String, S>)> = s
        .query(0)
        .points
        .into_iter()
        .map(|x| (x.at, x.statuses))
        .collect();
    assert_eq!(
        got,
        [
            (
                t - DAY + 2000,
                BTreeMap::from([("a".to_string(), S::Working), ("b".to_string(), S::Idle)])
            ),
            (
                t - DAY + 3000,
                BTreeMap::from([("a".to_string(), S::Working)])
            ),
            (t - 500, BTreeMap::from([("x".to_string(), S::Working)])),
        ]
    );
}

#[test]
fn costs_about_the_same_per_point_with_10_unchanged_agents() {
    let fs = MemoryFs::default();
    let mut s = setup(&fs, t0()).store;
    s.start();
    s.flush();
    let before = fs.get(FILE).unwrap().len();
    for i in 1..=100 {
        s.add_point(pt(t0() + i * 5000, &many(10, &[])), None);
    }
    s.flush();
    let per_point = (fs.get(FILE).unwrap().len() - before) as f64 / 100.0;
    assert!(per_point < 60.0, "{per_point}");
    assert_eq!(s.query(0).points[99].statuses.len(), 10);
}

#[test]
fn leaves_agents_finished_or_failed_for_over_an_hour_out_of_what_is_written_not_out_of_memory() {
    let fs = MemoryFs::default();
    let mut s = setup(&fs, t0()).store;
    s.start();
    let st = [
        ("run".to_string(), S::Working),
        ("done".to_string(), S::Finished),
        ("bad".to_string(), S::Failed),
    ];
    s.add_point(pt(t0(), &st), None);
    s.add_point(pt(t0() + 3_600_001, &st), None);
    s.flush();
    let written = point_lines(&fs, FILE);
    assert!(written[0].contains("\"done\":\"finished\""));
    // keys are sorted (BTreeMap), Hydra wrote them in insertion order
    assert_eq!(
        written[1],
        format!(
            "{{\"point\":[{},0,1,0,1,{{\"bad\":null,\"done\":null}},1]}}",
            t0() + 3_600_001
        )
    );
    assert_eq!(s.query(0).points[1].statuses.len(), 3);
}

#[test]
fn does_not_write_not_running_agents_to_disk_but_keeps_them_in_memory() {
    let fs = MemoryFs::default();
    let mut s = setup(&fs, t0()).store;
    s.start();
    s.add_point(
        pt(
            t0(),
            &[
                ("run".to_string(), S::Working),
                ("stopped".to_string(), S::NotRunning),
            ],
        ),
        None,
    );
    s.flush();
    let written = point_lines(&fs, FILE);
    assert!(written[0].contains("\"run\":\"working\""));
    assert!(!written[0].contains("stopped"));
    assert_eq!(s.query(0).points[0].statuses.len(), 2);
}

// ── wait ledger ─────────────────────────────────────────────────────────────

fn day_file(offset_days: i64) -> String {
    format!("v1-{}.jsonl", day_of(t0() + offset_days * DAY))
}
fn full(at: i64) -> String {
    format!("{{\"point\":[{at},0,1,0,1,{{\"a\":\"working\"}}]}}")
}
fn delta(at: i64) -> String {
    format!("{{\"point\":[{at},0,1,0,1,{{}},1]}}")
}
fn mv(at: i64, from: &str, to: &str) -> String {
    format!(
        "{{\"event\":{{\"id\":\"a@{at}:status\",\"at\":{at},\"kind\":\"status\",\"agentId\":\"a\",\"name\":\"alpha\",\"projectDir\":\"/w\",\"from\":\"{from}\",\"to\":\"{to}\"}}}}"
    )
}
fn span(start: i64, end: i64) -> MeasuredSpan {
    MeasuredSpan { start, end }
}

#[test]
fn seeds_transitions_and_spans_from_delta_points_breaking_at_gaps_and_starts() {
    let fs = MemoryFs::default();
    let t = t0() - 3 * DAY;
    fs.set(
        &day_file(-3),
        &[
            format!("{{\"start\":{t}}}"),
            full(t),
            delta(t + 5000),
            delta(t + 10_000),
            mv(t + 10_000, "working", "needs-you"),
            delta(t + 70_000), // 60 s gap: a new span
            mv(t + 72_000, "needs-you", "working"),
            delta(t + 75_000),
            format!("{{\"start\":{}}}", t + 77_000), // a start breaks the span however soon the next point comes
            full(t + 78_000),
        ]
        .join("\n"),
    );
    let mut s = setup(&fs, t0()).store;
    s.start();
    let ledger = s.waits();
    assert_eq!(
        ledger.spans,
        [
            span(t, t + 10_000),
            span(t + 70_000, t + 75_000),
            span(t + 78_000, t + 78_000)
        ]
    );
    let moves: Vec<(WaitDirection, i64, &str)> = ledger
        .transitions
        .iter()
        .map(|x| (x.direction, x.at, x.agent_id.as_str()))
        .collect();
    assert_eq!(
        moves,
        [
            (WaitDirection::In, t + 10_000, "a"),
            (WaitDirection::Out, t + 72_000, "a")
        ]
    );
    assert_eq!(ledger.transitions[0].name, "alpha");
    assert_eq!(ledger.starts, [t, t + 77_000, t0()]);
}

#[test]
fn joins_a_stretch_that_goes_on_in_the_next_days_file() {
    let fs = MemoryFs::default();
    let midnight = chrono::DateTime::parse_from_rfc3339("2026-10-20T00:00:00.000Z")
        .unwrap()
        .timestamp_millis();
    fs.set(
        "v1-2026-10-19.jsonl",
        &[
            format!("{{\"start\":{}}}", midnight - 20_000),
            full(midnight - 10_000),
            delta(midnight - 5000),
        ]
        .join("\n"),
    );
    fs.set(
        "v1-2026-10-20.jsonl",
        &[full(midnight + 2000), delta(midnight + 7000)].join("\n"),
    );
    let mut s = setup(&fs, t0()).store;
    s.start();
    assert_eq!(s.waits().spans, [span(midnight - 10_000, midnight + 7000)]);
}

#[test]
fn holds_more_than_the_rings_old_events_and_points_still_reach_the_ledger() {
    let fs = MemoryFs::default();
    let old = t0() - 2 * DAY;
    fs.set(
        &day_file(-2),
        &[
            format!("{{\"start\":{old}}}"),
            full(old),
            delta(old + 5000),
            mv(old + 5000, "working", "needs-you"),
            mv(old + 9000, "needs-you", "idle"),
            delta(old + 10_000),
        ]
        .join("\n"),
    );
    let base = t0() - 3 * 3_600_000;
    let mut recent = vec![format!("{{\"start\":{base}}}")];
    for i in 0..(MAX_POINTS as i64 + 10) {
        recent.push(if i == 0 {
            full(base + 1)
        } else {
            delta(base + 1 + i * 2)
        });
    }
    for i in 0..(MAX_EVENTS as i64 + 10) {
        recent.push(mv(base + 2 + i, "working", "idle"));
    }
    fs.set(&day_file(0), &recent.join("\n"));
    let mut s = setup(&fs, t0()).store;
    s.start();

    let rings = s.query(0);
    assert_eq!(rings.events.len(), MAX_EVENTS);
    assert!(rings.events.iter().all(|e| e.at > old + DAY));
    assert!(rings.since > old + DAY); // the rings begin where they did before

    let ledger = s.waits();
    let moves: Vec<(WaitDirection, i64)> = ledger
        .transitions
        .iter()
        .map(|x| (x.direction, x.at))
        .collect();
    assert_eq!(
        moves,
        [
            (WaitDirection::In, old + 5000),
            (WaitDirection::Out, old + 9000)
        ]
    );
    assert_eq!(ledger.spans[0], span(old, old + 10_000));
    // The recent run is one span although its older points were not kept in the ring.
    assert_eq!(ledger.spans[1].start, base + 1);
    assert_eq!(ledger.spans[1].end, base + 1 + (MAX_POINTS as i64 + 9) * 2);
}

#[test]
fn ledger_skips_malformed_lines_and_the_future() {
    let fs = MemoryFs::default();
    fs.set(
        &day_file(0),
        &[
            "not json".to_string(),
            format!(
                "{{\"point\":[{},0,1,0,1,{{\"a\":\"working\"}}]}}",
                t0() - 10_000
            ),
            format!("{{\"point\":[{},0,1,0,1,{{}}", t0() - 5000),
            format!("{{\"point\":[{},0,1,0,1,{{}}]}}", t0() + 3_600_000),
        ]
        .join("\n"),
    );
    let mut s = setup(&fs, t0()).store;
    s.start();
    assert_eq!(s.waits().spans, [span(t0() - 10_000, t0() - 10_000)]);
}

#[test]
fn the_ledger_is_fed_live_events_become_transitions_and_points_extend_the_stretch() {
    let mut s = setup(&MemoryFs::default(), t0()).store;
    s.start();
    s.add_point(p(t0() + 1000), None);
    s.add_events(&[
        ev(t0() + 2000, Some(S::Working), Some(S::NeedsYou)),
        ev(t0() + 2500, Some(S::Working), Some(S::Idle)),
        OverviewEvent {
            kind: EventKind::Ended,
            ..ev(t0() + 3000, Some(S::NeedsYou), None)
        },
    ]);
    s.add_point(p(t0() + 6000), None);
    s.add_point(p(t0() + 11_000), None);
    s.add_point(p(t0() + 60_000), None); // gap
    let ledger = s.waits();
    assert_eq!(
        ledger.spans,
        [
            span(t0() + 1000, t0() + 11_000),
            span(t0() + 60_000, t0() + 60_000)
        ]
    );
    let dirs: Vec<WaitDirection> = ledger.transitions.iter().map(|x| x.direction).collect();
    assert_eq!(dirs, [WaitDirection::In, WaitDirection::Out]);
    assert_eq!(ledger.starts, [t0()]);
}

#[test]
fn does_not_join_the_first_point_of_a_run_to_the_last_of_the_previous_one() {
    let fs = MemoryFs::default();
    let mut first = setup(&fs, t0()).store;
    first.start();
    first.add_point(p(t0() + 1000), None);
    first.stop();
    let mut second = setup(&fs, t0() + 3000).store;
    second.start();
    second.add_point(p(t0() + 4000), None);
    assert_eq!(
        second.waits().spans,
        [
            span(t0() + 1000, t0() + 1000),
            span(t0() + 4000, t0() + 4000)
        ]
    );
}

#[test]
fn prunes_what_is_older_than_9_days_on_restore_and_later() {
    let fs = MemoryFs::default();
    let ancient = t0() - 10 * DAY;
    fs.set(
        &day_file(-10),
        &[
            format!("{{\"start\":{ancient}}}"),
            full(ancient),
            delta(ancient + 5000),
            mv(ancient + 5000, "working", "needs-you"),
            mv(ancient + 8000, "needs-you", "idle"),
        ]
        .join("\n"),
    );
    let edge = t0() - 9 * DAY + 60_000;
    fs.set(
        &day_file(-9),
        &[
            format!("{{\"start\":{edge}}}"),
            full(edge),
            delta(edge + 5000),
            mv(edge + 2000, "working", "needs-you"),
        ]
        .join("\n"),
    );
    let Setup { mut store, clock } = setup(&fs, t0());
    store.start();
    let ledger = store.waits();
    assert_eq!(
        ledger.transitions.iter().map(|x| x.at).collect::<Vec<_>>(),
        [edge + 2000]
    );
    assert_eq!(ledger.starts, [edge, t0()]);
    clock.store(t0() + DAY, Ordering::SeqCst);
    let ledger = store.waits();
    assert!(ledger.transitions.is_empty());
    assert!(ledger.spans.is_empty());
}

// ── files_to_delete ─────────────────────────────────────────────────────────

fn sized(name: &str, size: u64) -> SizedFile {
    SizedFile {
        file: parse_history_file_name(name).unwrap(),
        size,
    }
}
fn limits() -> Limits {
    Limits {
        now: t0(),
        max_bytes: 100,
        max_age_ms: 8 * DAY,
    }
}
fn set_of(names: &[&str]) -> HashSet<String> {
    names.iter().map(|s| (*s).to_string()).collect()
}

#[test]
fn deletes_the_oldest_until_the_total_and_the_incoming_bytes_fit_never_the_file_written() {
    let files = [
        sized("v1-2026-10-20.jsonl", 40),
        sized("v1-2026-10-21.jsonl", 40),
        sized("v1-2026-10-22.jsonl", 30),
    ];
    assert_eq!(
        files_to_delete(&files, &set_of(&["v1-2026-10-22.jsonl"]), 10, &limits()),
        ["v1-2026-10-20.jsonl"]
    );
    assert_eq!(
        files_to_delete(&files, &set_of(&["v1-2026-10-22.jsonl"]), 40, &limits()),
        ["v1-2026-10-20.jsonl", "v1-2026-10-21.jsonl"]
    );
    assert_eq!(
        files_to_delete(&files, &set_of(&["v1-2026-10-20.jsonl"]), 100, &limits()),
        ["v1-2026-10-21.jsonl", "v1-2026-10-22.jsonl"]
    );
}

#[test]
fn deletes_by_age_first_and_orders_parts_of_a_day() {
    let files = [
        sized("v1-2026-10-05-2.jsonl", 1),
        sized("v1-2026-10-05.jsonl", 1),
        sized("v1-2026-10-22.jsonl", 1),
    ];
    assert_eq!(
        files_to_delete(&files, &HashSet::new(), 0, &limits()),
        ["v1-2026-10-05.jsonl", "v1-2026-10-05-2.jsonl"]
    );
}

#[test]
fn rejects_names_that_are_not_v1_files_or_days() {
    assert!(parse_history_file_name("v1-2026-02-31.jsonl").is_none());
    assert!(parse_history_file_name("v2-2026-02-01.jsonl").is_none());
    assert!(parse_history_file_name("v1-2026-02-01-1.jsonl").is_none());
    assert_eq!(
        parse_history_file_name("v1-2026-02-01-3.jsonl")
            .unwrap()
            .part,
        3
    );
}

#[cfg(unix)]
#[test]
fn the_real_file_system_writes_private_files_and_never_follows_links() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("overview-history");
    let mut options = HistoryStoreOptions::new(dir.clone());
    options.now = Box::new(t0);
    let mut s = HistoryStore::new(options);
    s.start();
    s.add_point(p(t0() + 1), None);
    s.stop();
    let file = dir.join(FILE);
    assert_eq!(
        std::fs::metadata(&file).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(
        std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(std::fs::read_to_string(&file).unwrap().lines().count(), 2);

    // A link where a history file should be is not read, written to or deleted.
    let target = tmp.path().join("secret.txt");
    std::fs::write(&target, "keep").unwrap();
    std::os::unix::fs::symlink(&target, dir.join("v1-2026-10-21.jsonl")).unwrap();
    let mut s2 = HistoryStore::new({
        let mut o = HistoryStoreOptions::new(dir.clone());
        o.now = Box::new(t0);
        o
    });
    s2.start();
    s2.stop();
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "keep");
}
