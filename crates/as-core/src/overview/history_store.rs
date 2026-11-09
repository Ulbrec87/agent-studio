// Adapted from agent-lookout (MIT, © 2026 Idris Olubisi): src/collector/history/historyKeeper.ts,
//   historyFormat.ts, historyLimits.ts, src/collector/eventStore.ts, historyStore.ts
//! Events and samples of the Overview, in memory rings and as JSON lines on disk
//! (`<root>/overview-history/v1-YYYY-MM-DD.jsonl`).
//!
//! Single writer: the daemon is single-instance (`daemon.lock`), so no writer lock is needed. Do
//! not open the same folder from a second process.
//!
//! Nothing is written while a poll runs: records are buffered and appended on `flush()` (the
//! daemon calls it every 5 s) and on `stop()`. When the folder cannot be made or written, the
//! store keeps working in memory.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use chrono::{TimeZone, Utc};
use regex::Regex;
use serde::Serialize;
use serde_json::Value;

use super::history_files::{HistoryFs, PathKind, StdHistoryFs};
use super::types::{
    EventKind, HistoryAgent, HistoryPoint, HistoryResponse, OverviewEvent, OverviewStatus,
    WaitLedgerData, WaitingReason,
};
use super::wait_ledger::{LEDGER_MS, WaitLedger};

/// Events kept in memory.
pub const MAX_EVENTS: usize = 1000;
/// Points kept in memory: 6 h of one point every 5 s.
pub const MAX_POINTS: usize = 4320;
/// Interval between two history points (kept by the tracker).
pub const SAMPLE_INTERVAL_MS: i64 = 5000;
/// The most one file may hold; a day that outgrows it goes on in `-2`, `-3`...
pub const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;
/// The most all history files may hold.
pub const MAX_TOTAL_BYTES: u64 = 20 * 1024 * 1024;
/// A day is kept this long after it ends.
pub const MAX_AGE_MS: i64 = 8 * 24 * 60 * 60 * 1000;
/// A file larger than this is not one we wrote and is not read.
const MAX_READ_BYTES: u64 = 2 * MAX_FILE_BYTES;
/// A longer line is not a record.
const MAX_LINE_LENGTH: usize = 8 * 1024;
const MAX_TEXT_LENGTH: usize = 1024;
/// Records dated this far after the clock were written while the clock was wrong.
const FUTURE_SLACK_MS: i64 = 60 * 1000;
/// Unwritten lines kept when writing keeps failing.
const MAX_PENDING_LINES: usize = 20_000;
const DAY_MS: i64 = 24 * 60 * 60 * 1000;
/// A finished or failed agent stays in the points written to disk this long.
const ENDED_KEPT_MS: i64 = 60 * 60 * 1000;

// ── File names ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileName {
    pub name: String,
    pub day: String,
    pub day_start: i64,
    pub part: u32,
}

/// The UTC day of a moment, `2026-10-06`.
pub fn day_of(at: i64) -> String {
    Utc.timestamp_millis_opt(at).single().map_or_else(
        || "1970-01-01".to_string(),
        |d| d.format("%Y-%m-%d").to_string(),
    )
}

/// `v1-2026-10-06.jsonl`, then `v1-2026-10-06-2.jsonl`.
pub fn history_file_name(day: &str, part: u32) -> String {
    if part > 1 {
        format!("v1-{day}-{part}.jsonl")
    } else {
        format!("v1-{day}.jsonl")
    }
}

fn file_name_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"^v1-(\d{4})-(\d{2})-(\d{2})(?:-([2-9]|[1-9]\d{1,3}))?\.jsonl$")
            .unwrap_or_else(|_| unreachable!("constant pattern"))
    })
}

/// A name read as a v1 history file's, `None` for anything else (other formats are left alone).
pub fn parse_history_file_name(name: &str) -> Option<FileName> {
    let m = file_name_regex().captures(name)?;
    let day = format!("{}-{}-{}", &m[1], &m[2], &m[3]);
    let date = chrono::NaiveDate::parse_from_str(&day, "%Y-%m-%d").ok()?;
    let day_start = date.and_hms_opt(0, 0, 0)?.and_utc().timestamp_millis();
    if day_of(day_start) != day {
        return None;
    }
    Some(FileName {
        name: name.to_string(),
        day,
        day_start,
        part: m.get(4).and_then(|p| p.as_str().parse().ok()).unwrap_or(1),
    })
}

fn by_file_order(a: &FileName, b: &FileName) -> std::cmp::Ordering {
    a.day_start.cmp(&b.day_start).then(a.part.cmp(&b.part))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SizedFile {
    pub file: FileName,
    pub size: u64,
}

pub struct Limits {
    pub now: i64,
    pub max_bytes: u64,
    pub max_age_ms: i64,
}

/// The files to delete, oldest first: every file whose day ended more than `max_age_ms` ago,
/// then the oldest until the rest and `incoming` fit in `max_bytes`. Files in `writing` never go.
pub fn files_to_delete(
    files: &[SizedFile],
    writing: &HashSet<String>,
    incoming: u64,
    limits: &Limits,
) -> Vec<String> {
    let mut ordered: Vec<&SizedFile> = files.iter().collect();
    ordered.sort_by(|a, b| by_file_order(&a.file, &b.file));
    let mut doomed = Vec::new();
    let mut left: Vec<&SizedFile> = Vec::new();
    let mut total: u64 = 0;
    for f in ordered {
        if !writing.contains(&f.file.name)
            && f.file.day_start + DAY_MS + limits.max_age_ms < limits.now
        {
            doomed.push(f.file.name.clone());
        } else {
            total += f.size;
            left.push(f);
        }
    }
    for f in left {
        if total + incoming <= limits.max_bytes {
            break;
        }
        if writing.contains(&f.file.name) {
            continue;
        }
        doomed.push(f.file.name.clone());
        total = total.saturating_sub(f.size);
    }
    doomed
}

// ── Records ─────────────────────────────────────────────────────────────────

type StatusDelta = BTreeMap<String, Option<OverviewStatus>>;

#[derive(Debug, Clone)]
enum Record {
    Start(i64),
    /// `delta` is set on a point read from a delta line: ids that changed, `None` for an agent
    /// that is gone.
    Point {
        point: HistoryPoint,
        delta: Option<StatusDelta>,
    },
    Event(OverviewEvent),
}

fn record_at(r: &Record) -> i64 {
    match r {
        Record::Start(at) => *at,
        Record::Point { point, .. } => point.at,
        Record::Event(e) => e.at,
    }
}

/// A point as one line. With no `base` the line holds the full status map (the first point of a
/// run and of each file, so any file restores on its own). With a `base` (the map of the previous
/// point written to the same file) it holds a delta, marked by a trailing `1`: the agents whose
/// status changed, and `null` for those that are gone.
fn encode_point(
    p: &HistoryPoint,
    statuses: &BTreeMap<String, OverviewStatus>,
    base: Option<&BTreeMap<String, OverviewStatus>>,
) -> String {
    let head = (p.at, p.needs_you, p.working, p.idle, p.total);
    let line = match base {
        None => serde_json::json!({"point": [head.0, head.1, head.2, head.3, head.4, statuses]}),
        Some(base) => {
            let mut delta: StatusDelta = BTreeMap::new();
            for (id, st) in statuses {
                if base.get(id) != Some(st) {
                    delta.insert(id.clone(), Some(*st));
                }
            }
            for id in base.keys() {
                if !statuses.contains_key(id) {
                    delta.insert(id.clone(), None);
                }
            }
            serde_json::json!({"point": [head.0, head.1, head.2, head.3, head.4, delta, 1]})
        }
    };
    format!("{line}\n")
}

/// Events are written field by field, so nothing else can reach a file.
#[derive(Serialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
struct EncEvent<'a> {
    id: &'a str,
    at: i64,
    kind: EventKind,
    agent_id: &'a str,
    name: &'a str,
    project_dir: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    from: Option<OverviewStatus>,
    #[serde(skip_serializing_if = "Option::is_none")]
    to: Option<OverviewStatus>,
    #[serde(skip_serializing_if = "Option::is_none")]
    waiting_reason: Option<WaitingReason>,
    #[serde(skip_serializing_if = "Option::is_none")]
    waited_ms: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    gap_ms: Option<i64>,
}

#[derive(Serialize, ts_rs::TS)]
struct EncEventLine<'a> {
    event: EncEvent<'a>,
}

fn encode_other(r: &Record) -> String {
    match r {
        Record::Start(at) => format!("{{\"start\":{at}}}\n"),
        Record::Event(e) => {
            let line = EncEventLine {
                event: EncEvent {
                    id: &e.id,
                    at: e.at,
                    kind: e.kind,
                    agent_id: &e.agent_id,
                    name: &e.name,
                    project_dir: &e.project_dir,
                    from: e.from,
                    to: e.to,
                    waiting_reason: e.waiting_reason,
                    waited_ms: e.waited_ms,
                    gap_ms: e.gap_ms,
                },
            };
            format!("{}\n", serde_json::to_string(&line).unwrap_or_default())
        }
        Record::Point { .. } => String::new(),
    }
}

fn num(v: Option<&Value>) -> Option<i64> {
    let n = v?.as_f64()?;
    (n.is_finite() && n >= 0.0).then_some(n as i64)
}

fn text(v: Option<&Value>) -> Option<String> {
    let s = v?.as_str()?;
    (s.len() <= MAX_TEXT_LENGTH).then(|| s.to_string())
}

fn status(v: Option<&Value>) -> Option<OverviewStatus> {
    serde_json::from_value(v?.clone()).ok()
}

fn decode_event(v: &Value) -> Option<OverviewEvent> {
    let o = v.as_object()?;
    let kind: EventKind = serde_json::from_value(o.get("kind")?.clone()).ok()?;
    Some(OverviewEvent {
        id: text(o.get("id"))?,
        at: num(o.get("at"))?,
        kind,
        agent_id: text(o.get("agentId"))?,
        name: text(o.get("name"))?,
        project_dir: text(o.get("projectDir"))?,
        from: status(o.get("from")),
        to: status(o.get("to")),
        waiting_reason: o
            .get("waitingReason")
            .and_then(|r| serde_json::from_value(r.clone()).ok()),
        waited_ms: num(o.get("waitedMs")),
        gap_ms: num(o.get("gapMs")),
    })
}

fn decode_point(v: &Value) -> Option<Record> {
    let arr = v.as_array().filter(|a| a.len() >= 5)?;
    let n = |i: usize| num(arr.get(i));
    let (at, needs_you, working, idle, total) = (n(0)?, n(1)?, n(2)?, n(3)?, n(4)?);
    let is_delta = arr.get(6).and_then(Value::as_i64) == Some(1);
    let mut statuses = BTreeMap::new();
    let mut delta: StatusDelta = BTreeMap::new();
    if let Some(raw) = arr.get(5).and_then(Value::as_object) {
        for (id, s) in raw {
            if id.len() > MAX_TEXT_LENGTH {
                continue;
            }
            if let Some(st) = status(Some(s)) {
                statuses.insert(id.clone(), st);
                delta.insert(id.clone(), Some(st));
            } else if s.is_null() && is_delta {
                delta.insert(id.clone(), None);
            }
        }
    }
    let count = |v: i64| u32::try_from(v).unwrap_or(u32::MAX);
    let point = HistoryPoint {
        at,
        needs_you: count(needs_you),
        working: count(working),
        idle: count(idle),
        total: count(total),
        statuses,
    };
    Some(Record::Point {
        point,
        delta: is_delta.then_some(delta),
    })
}

/// One line as a record, `None` when it is not one (blank, too long, malformed, unknown).
fn decode_line(line: &str) -> Option<Record> {
    if line.is_empty() || line.len() > MAX_LINE_LENGTH {
        return None;
    }
    let value: Value = serde_json::from_str(line).ok()?;
    let o = value.as_object()?;
    if let Some(start) = o.get("start") {
        return num(Some(start)).map(Record::Start);
    }
    if let Some(point) = o.get("point") {
        return decode_point(point);
    }
    if let Some(event) = o.get("event") {
        return decode_event(event).map(Record::Event);
    }
    None
}

fn point_time_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"^\{"point":\[(\d{1,16}),"#)
            .unwrap_or_else(|_| unreachable!("constant pattern"))
    })
}

// ── The store ───────────────────────────────────────────────────────────────

pub struct HistoryStoreOptions {
    /// Folder of the history files, `<root>/overview-history`.
    pub dir: PathBuf,
    pub fs: Box<dyn HistoryFs>,
    pub now: Box<dyn Fn() -> i64 + Send>,
    /// Clock for the restore time budget.
    pub budget_clock: Box<dyn Fn() -> i64 + Send>,
    /// Most time `start()` may spend reading files. Default 10 s.
    pub restore_budget_ms: i64,
}

impl HistoryStoreOptions {
    pub fn new(dir: PathBuf) -> Self {
        Self {
            dir,
            fs: Box::new(StdHistoryFs),
            now: Box::new(wall_clock_ms),
            budget_clock: Box::new(wall_clock_ms),
            restore_budget_ms: 10_000,
        }
    }
}

pub fn wall_clock_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

struct Pending {
    day: String,
    record: Record,
    disk: Option<BTreeMap<String, OverviewStatus>>,
}

struct Scanned {
    file: FileName,
    size: u64,
    ordinary: bool,
}

pub struct HistoryStore {
    dir: PathBuf,
    fs: Box<dyn HistoryFs>,
    now: Box<dyn Fn() -> i64 + Send>,
    budget_clock: Box<dyn Fn() -> i64 + Send>,
    restore_budget_ms: i64,

    events: Vec<OverviewEvent>,
    points: Vec<HistoryPoint>,
    starts: Vec<i64>,
    names: HashMap<String, HistoryAgent>,
    started_at: i64,
    /// Where the restored history begins; `None` when nothing was restored.
    since_at: Option<i64>,
    trimmed: bool,
    /// Moves into and out of needs-you and measured stretches of the last 9 days.
    ledger: WaitLedger,

    pending: Vec<Pending>,
    /// When each finished or failed agent was first seen so, to leave it out of points after an hour.
    ended_since: HashMap<String, i64>,
    /// The status map of the last point written, and the file it went to.
    base: Option<BTreeMap<String, OverviewStatus>>,
    base_file: Option<String>,
    /// Files this run has appended to; the first append to a non-empty one starts with a line break.
    touched: HashSet<String>,
    persist: bool,
    warned: bool,
    running: bool,
}

impl HistoryStore {
    pub fn new(options: HistoryStoreOptions) -> Self {
        Self {
            dir: options.dir,
            fs: options.fs,
            now: options.now,
            budget_clock: options.budget_clock,
            restore_budget_ms: options.restore_budget_ms,
            events: Vec::new(),
            points: Vec::new(),
            starts: Vec::new(),
            names: HashMap::new(),
            started_at: 0,
            since_at: None,
            trimmed: false,
            ledger: WaitLedger::default(),
            pending: Vec::new(),
            ended_since: HashMap::new(),
            base: None,
            base_file: None,
            touched: HashSet::new(),
            persist: true,
            warned: false,
            running: false,
        }
    }

    /// Restores what the folder holds and records a `start`.
    pub fn start(&mut self) {
        if self.running {
            return;
        }
        self.running = true;
        self.started_at = (self.now)();
        if let Err(err) = self.fs.make_folder(&self.dir) {
            self.persist = false;
            self.warn(
                "cannot make the history folder, keeping history in memory",
                &err,
            );
        }
        if self.persist {
            self.restore();
        }
        self.starts.push(self.started_at);
        self.ledger.add_start(self.started_at);
        self.ledger.prune(self.started_at);
        self.enqueue(Record::Start(self.started_at));
        self.flush();
    }

    /// Writes what is left.
    pub fn stop(&mut self) {
        self.running = false;
        self.flush();
    }

    pub fn add_events(&mut self, events: &[OverviewEvent]) {
        for event in events {
            self.events.push(event.clone());
            if !event.agent_id.is_empty() {
                self.names.insert(
                    event.agent_id.clone(),
                    HistoryAgent {
                        name: event.name.clone(),
                        project_dir: event.project_dir.clone(),
                    },
                );
            }
            self.enqueue(Record::Event(event.clone()));
        }
        self.ledger.add_events(events);
        if self.events.len() > MAX_EVENTS {
            let extra = self.events.len() - MAX_EVENTS;
            self.events.drain(..extra);
        }
    }

    /// `agents` tells the names of the agents in `point.statuses`.
    pub fn add_point(
        &mut self,
        point: HistoryPoint,
        agents: Option<&BTreeMap<String, HistoryAgent>>,
    ) {
        self.ledger.add_point(point.at);
        if let Some(agents) = agents {
            for (id, a) in agents {
                self.names.insert(id.clone(), a.clone());
            }
        }
        self.points.push(point.clone());
        self.enqueue(Record::Point { point, delta: None });
        if self.points.len() > MAX_POINTS {
            let extra = self.points.len() - MAX_POINTS;
            self.points.drain(..extra);
            self.trimmed = true;
        }
    }

    /// The last 9 days of waits and measured time, for the totals.
    pub fn waits(&mut self) -> WaitLedgerData {
        self.ledger.prune((self.now)());
        self.ledger.data()
    }

    /// When the newest point was taken, restored ones included.
    pub fn last_point_at(&self) -> Option<i64> {
        self.points.last().map(|p| p.at)
    }

    /// Points, events and starts from `since_ms` on, oldest first, and the names of the agents
    /// they mention.
    pub fn query(&self, since_ms: i64) -> HistoryResponse {
        let points: Vec<HistoryPoint> = self
            .points
            .iter()
            .filter(|p| p.at >= since_ms)
            .cloned()
            .collect();
        let events: Vec<OverviewEvent> = self
            .events
            .iter()
            .filter(|e| e.at >= since_ms)
            .cloned()
            .collect();
        let mut agents = BTreeMap::new();
        let mut note = |id: &str| {
            if let Some(a) = self.names.get(id) {
                agents.insert(id.to_string(), a.clone());
            }
        };
        for p in &points {
            for id in p.statuses.keys() {
                note(id);
            }
        }
        for e in &events {
            if !e.agent_id.is_empty() {
                note(&e.agent_id);
            }
        }
        let begins = self.since_at.unwrap_or(self.started_at);
        let since = match self.points.first() {
            Some(first) if self.trimmed => begins.max(first.at),
            _ => begins,
        };
        HistoryResponse {
            since,
            started_at: self.started_at,
            starts: self
                .starts
                .iter()
                .copied()
                .filter(|s| *s >= since_ms)
                .collect(),
            points,
            events,
            agents,
        }
    }

    // ── Disk ────────────────────────────────────────────────────────────────

    fn enqueue(&mut self, record: Record) {
        if !self.persist {
            return;
        }
        let disk = match &record {
            Record::Point { point, .. } => Some(self.disk_statuses(point)),
            _ => None,
        };
        self.pending.push(Pending {
            day: day_of(record_at(&record)),
            record,
            disk,
        });
        if self.pending.len() > MAX_PENDING_LINES {
            let extra = self.pending.len() - MAX_PENDING_LINES;
            self.pending.drain(..extra);
        }
    }

    /// The statuses a point keeps on disk: agents finished or failed for over an hour, and
    /// not-running ones, are left out.
    fn disk_statuses(&mut self, point: &HistoryPoint) -> BTreeMap<String, OverviewStatus> {
        let mut out = BTreeMap::new();
        for (id, st) in &point.statuses {
            if *st == OverviewStatus::NotRunning {
                continue;
            }
            if matches!(st, OverviewStatus::Finished | OverviewStatus::Failed) {
                let since = *self.ended_since.entry(id.clone()).or_insert(point.at);
                if point.at - since > ENDED_KEPT_MS {
                    continue;
                }
            } else {
                self.ended_since.remove(id);
            }
            out.insert(id.clone(), *st);
        }
        self.ended_since
            .retain(|id, _| point.statuses.contains_key(id));
        out
    }

    fn warn(&mut self, message: &str, err: &dyn std::fmt::Display) {
        if self.warned {
            return;
        }
        self.warned = true;
        tracing::warn!("overview history: {message}: {err}");
    }

    fn scan(&self) -> std::io::Result<Vec<Scanned>> {
        let mut files = Vec::new();
        for name in self.fs.list(&self.dir)? {
            let Some(file) = parse_history_file_name(&name) else {
                continue;
            };
            if let Some(info) = self.fs.stat(&self.dir.join(&name))? {
                files.push(Scanned {
                    file,
                    size: info.size,
                    ordinary: info.kind == PathKind::File,
                });
            }
        }
        Ok(files)
    }

    fn restore(&mut self) {
        let now = (self.now)();
        let deadline = (self.budget_clock)() + self.restore_budget_ms;
        let mut files = match self.scan() {
            Ok(f) => f,
            Err(err) => {
                self.warn("cannot read the history folder", &err);
                return;
            }
        };
        files.sort_by(|a, b| by_file_order(&b.file, &a.file)); // newest first

        // Chunks, newest file first (reversed at the end).
        let mut event_chunks: Vec<Vec<OverviewEvent>> = Vec::new();
        let mut point_chunks: Vec<Vec<HistoryPoint>> = Vec::new();
        let mut start_chunks: Vec<Vec<i64>> = Vec::new();
        let mut whole_chunks: Vec<WaitLedgerData> = Vec::new();
        let (mut event_count, mut point_count) = (0usize, 0usize);
        let mut oldest: Option<i64> = None;
        let whole_from = now - LEDGER_MS - DAY_MS;
        for scanned in &files {
            if (self.budget_clock)() > deadline {
                break;
            }
            if !scanned.ordinary || scanned.size > MAX_READ_BYTES {
                continue;
            }
            // Once the rings are full, older files only feed the wait ledger.
            let rings_full = event_count >= MAX_EVENTS && point_count >= MAX_POINTS;
            if rings_full && scanned.file.day_start < whole_from {
                continue;
            }
            let Ok(content) = self
                .fs
                .read(&self.dir.join(&scanned.file.name), MAX_READ_BYTES)
            else {
                continue;
            };
            let mut events = Vec::new();
            let mut points = Vec::new();
            let mut starts = Vec::new();
            let mut whole = WaitLedger::default();
            let mut first: Option<i64> = None;
            // Status map of the last point read from this file; a delta needs a full point before it.
            let mut state: Option<BTreeMap<String, OverviewStatus>> = None;
            for line in content.split('\n') {
                if rings_full
                    && line.len() <= MAX_LINE_LENGTH
                    && let Some(m) = point_time_regex().captures(line)
                    && let Ok(at) = m[1].parse::<i64>()
                    && at <= now + FUTURE_SLACK_MS
                {
                    // Only the time of a point matters here; skip parsing its statuses.
                    whole.add_point(at);
                    continue;
                }
                let Some(record) = decode_line(line) else {
                    continue;
                };
                let at = record_at(&record);
                if at > now + FUTURE_SLACK_MS {
                    continue;
                }
                if !rings_full {
                    first = Some(first.map_or(at, |f| f.min(at)));
                }
                match record {
                    Record::Event(event) => {
                        whole.add_events(std::slice::from_ref(&event));
                        if !rings_full {
                            events.push(event);
                        }
                    }
                    Record::Point { point, delta } => {
                        whole.add_point(at);
                        if rings_full {
                            continue;
                        }
                        match delta {
                            Some(delta) => {
                                let Some(prev) = &state else { continue };
                                let mut next = prev.clone();
                                for (id, st) in delta {
                                    match st {
                                        Some(st) => {
                                            next.insert(id, st);
                                        }
                                        None => {
                                            next.remove(&id);
                                        }
                                    }
                                }
                                state = Some(next);
                            }
                            None => state = Some(point.statuses.clone()),
                        }
                        points.push(HistoryPoint {
                            statuses: state.clone().unwrap_or_default(),
                            ..point
                        });
                    }
                    Record::Start(at) => {
                        whole.add_start(at);
                        starts.push(at);
                    }
                }
            }
            whole_chunks.push(whole.data());
            event_count += events.len();
            point_count += points.len();
            event_chunks.push(events);
            point_chunks.push(points);
            start_chunks.push(starts);
            if let Some(f) = first {
                oldest = Some(f);
            }
        }

        // Files hold records in time order; sort anyway so a skewed clock cannot disorder the rings.
        let mut events: Vec<OverviewEvent> = event_chunks.into_iter().rev().flatten().collect();
        events.sort_by_key(|e| e.at);
        let mut points: Vec<HistoryPoint> = point_chunks.into_iter().rev().flatten().collect();
        points.sort_by_key(|p| p.at);
        let mut starts: Vec<i64> = start_chunks.into_iter().rev().flatten().collect();
        starts.sort_unstable();
        self.trimmed = points.len() > MAX_POINTS;
        if events.len() > MAX_EVENTS {
            events.drain(..events.len() - MAX_EVENTS);
        }
        if points.len() > MAX_POINTS {
            points.drain(..points.len() - MAX_POINTS);
        }
        self.events = events;
        self.points = points;
        self.starts = starts;
        self.since_at = oldest;
        for chunk in whole_chunks.into_iter().rev() {
            self.ledger.append(chunk);
        }
        for e in &self.events {
            if !e.agent_id.is_empty() {
                self.names.insert(
                    e.agent_id.clone(),
                    HistoryAgent {
                        name: e.name.clone(),
                        project_dir: e.project_dir.clone(),
                    },
                );
            }
        }
    }

    /// Appends what is buffered, pruning old files first. Failures keep the lines for the next try.
    pub fn flush(&mut self) {
        if !self.persist || self.pending.is_empty() {
            return;
        }
        if let Err(err) = self.try_flush() {
            self.warn("cannot write the history", &err);
        }
    }

    fn try_flush(&mut self) -> std::io::Result<()> {
        let now = (self.now)();
        let files = self.scan()?;
        struct Target {
            name: String,
            size: u64,
            chunk: String,
            base: Option<BTreeMap<String, OverviewStatus>>,
            base_file: Option<String>,
        }
        let (targets, incoming) = {
            // Group by day, in order of first appearance.
            let mut order: Vec<String> = Vec::new();
            let mut by_day: HashMap<String, Vec<&Pending>> = HashMap::new();
            for item in &self.pending {
                if !by_day.contains_key(&item.day) {
                    order.push(item.day.clone());
                }
                by_day.entry(item.day.clone()).or_default().push(item);
            }

            // Pick the file each day's lines go to: the day's last, or the next one when it is full
            // or not ordinary.
            let mut targets: Vec<(String, Target)> = Vec::new();
            let mut incoming: u64 = 0;
            let mut base = self.base.clone();
            let mut base_file = self.base_file.clone();
            for day in &order {
                let items = &by_day[day];
                let mut same_day: Vec<&Scanned> =
                    files.iter().filter(|f| f.file.day == *day).collect();
                same_day.sort_by(|a, b| by_file_order(&a.file, &b.file));
                let last = same_day.last();
                let mut part = last.map_or(1, |l| l.file.part);
                let mut size = last.filter(|l| l.ordinary).map_or(0, |l| l.size);
                // Upper bound of the chunk, with every point full, to decide whether the file has room.
                let bound: u64 = items
                    .iter()
                    .map(|p| match (&p.record, &p.disk) {
                        (Record::Point { point, .. }, Some(disk)) => {
                            encode_point(point, disk, None).len()
                        }
                        (record, _) => encode_other(record).len(),
                    } as u64)
                    .sum();
                if let Some(l) = last
                    && (!l.ordinary || (size > 0 && size + bound > MAX_FILE_BYTES))
                {
                    part += 1;
                    size = 0;
                }
                let name = history_file_name(day, part);
                // The first point written to a file is full, so each file restores on its own.
                let mut chunk = String::new();
                for p in items {
                    match (&p.record, &p.disk) {
                        (Record::Point { point, .. }, Some(disk)) => {
                            let against = if base_file.as_deref() == Some(name.as_str()) {
                                base.as_ref()
                            } else {
                                None
                            };
                            chunk.push_str(&encode_point(point, disk, against));
                            base = Some(disk.clone());
                            base_file = Some(name.clone());
                        }
                        (record, _) => chunk.push_str(&encode_other(record)),
                    }
                }
                incoming += chunk.len() as u64;
                targets.push((
                    day.clone(),
                    Target {
                        name,
                        size,
                        chunk,
                        base: base.clone(),
                        base_file: base_file.clone(),
                    },
                ));
            }

            (targets, incoming)
        };

        let writing: HashSet<String> = targets.iter().map(|(_, t)| t.name.clone()).collect();
        let sized: Vec<SizedFile> = files
            .iter()
            .map(|f| SizedFile {
                file: f.file.clone(),
                size: f.size,
            })
            .collect();
        let doomed = files_to_delete(
            &sized,
            &writing,
            incoming,
            &Limits {
                now,
                max_bytes: MAX_TOTAL_BYTES,
                max_age_ms: MAX_AGE_MS,
            },
        );
        let mut delete_error = None;
        for name in doomed {
            match self.fs.remove(&self.dir.join(&name)) {
                Ok(()) => {
                    self.touched.remove(&name);
                }
                Err(err) => delete_error = Some(err),
            }
        }
        if let Some(err) = delete_error {
            self.warn("cannot delete an old history file", &err);
        }

        let mut failed: HashSet<String> = HashSet::new();
        let mut write_error = None;
        for (day, target) in targets {
            let lead = if target.size > 0 && !self.touched.contains(&target.name) {
                "\n"
            } else {
                ""
            };
            let path = self.dir.join(&target.name);
            match self.fs.append(&path, &format!("{lead}{}", target.chunk)) {
                Ok(()) => {
                    self.touched.insert(target.name.clone());
                    self.base = target.base;
                    self.base_file = target.base_file;
                }
                Err(err) => {
                    failed.insert(day);
                    write_error = Some(err);
                }
            }
        }
        if let Some(err) = write_error {
            self.warn("cannot write the history", &err);
        }
        self.pending.retain(|p| failed.contains(&p.day));
        Ok(())
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }
}

#[cfg(test)]
mod tests;
