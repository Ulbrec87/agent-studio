use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::Duration;

use chrono::{TimeZone, Utc};
use parking_lot::Mutex;
use serde_json::{Value, json};

use super::*;

const T0: i64 = 1_792_670_400_000; // 2026-10-22T12:00:00Z
const DIR: &str = "/cache/usage-cache.json";
const CLAUDE: &str = "/c/projects";
const CODEX: &str = "/x/sessions";

fn ms(iso: &str) -> i64 {
    crate::usage::parse_iso_ms(iso).unwrap()
}

#[derive(Clone)]
struct MemFile {
    data: Vec<u8>,
    mtime_ms: i64,
    ino: u64,
}

#[derive(Default)]
struct MemoryFs {
    files: Mutex<HashMap<PathBuf, MemFile>>,
    texts: Mutex<HashMap<PathBuf, Vec<u8>>>,
    next_ino: AtomicUsize,
    reads: AtomicUsize,
    walks: AtomicUsize,
    writes: AtomicUsize,
    slow_walk_ms: AtomicUsize,
}

impl MemoryFs {
    fn write_at(&self, path: &str, text: &str, mtime_ms: i64, ino: Option<u64>) {
        let mut files = self.files.lock();
        let old = files.get(Path::new(path)).map(|f| f.ino);
        let ino = ino
            .or(old)
            .unwrap_or_else(|| self.next_ino.fetch_add(1, Ordering::SeqCst) as u64 + 1);
        files.insert(
            PathBuf::from(path),
            MemFile {
                data: text.as_bytes().to_vec(),
                mtime_ms,
                ino,
            },
        );
    }
    fn write(&self, path: &str, text: &str) {
        self.write_at(path, text, T0 - 3_600_000, None);
    }
    fn append_at(&self, path: &str, text: &str, mtime_ms: i64) {
        let mut files = self.files.lock();
        let f = files.get_mut(Path::new(path)).unwrap();
        f.data.extend_from_slice(text.as_bytes());
        f.mtime_ms = mtime_ms;
    }
    fn replace(&self, path: &str, text: &str) {
        let ino = self.next_ino.fetch_add(1, Ordering::SeqCst) as u64 + 1000;
        self.write_at(path, text, T0 - 1_800_000, Some(ino));
    }
    fn remove(&self, path: &str) {
        self.files.lock().remove(Path::new(path));
    }
    fn cache(&self) -> Option<String> {
        self.texts
            .lock()
            .get(Path::new(DIR))
            .map(|b| String::from_utf8_lossy(b).into_owned())
    }
    fn set_cache(&self, text: &str) {
        self.texts
            .lock()
            .insert(PathBuf::from(DIR), text.as_bytes().to_vec());
    }
}

impl UsageFs for MemoryFs {
    fn walk(&self, root: &Path, _max_depth: usize) -> Vec<UsageFileInfo> {
        self.walks.fetch_add(1, Ordering::SeqCst);
        let slow = self.slow_walk_ms.load(Ordering::SeqCst);
        if slow > 0 {
            thread::sleep(Duration::from_millis(slow as u64));
        }
        self.files
            .lock()
            .iter()
            .filter(|(p, _)| p.starts_with(root))
            .map(|(p, f)| UsageFileInfo {
                path: p.clone(),
                size: f.data.len() as u64,
                mtime_ms: f.mtime_ms,
                ino: f.ino,
            })
            .collect()
    }
    fn read_chunk(&self, path: &Path, offset: u64, max_bytes: usize) -> std::io::Result<Vec<u8>> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        let files = self.files.lock();
        let f = files
            .get(path)
            .ok_or_else(|| std::io::Error::other("gone"))?;
        let start = (offset as usize).min(f.data.len());
        let end = (start + max_bytes).min(f.data.len());
        Ok(f.data[start..end].to_vec())
    }
    fn read_text(&self, path: &Path, _max: u64) -> std::io::Result<Option<String>> {
        Ok(self
            .texts
            .lock()
            .get(path)
            .map(|b| String::from_utf8_lossy(b).into_owned()))
    }
    fn write_atomic(&self, path: &Path, bytes: &[u8]) -> std::io::Result<()> {
        self.writes.fetch_add(1, Ordering::SeqCst);
        self.texts.lock().insert(path.to_path_buf(), bytes.to_vec());
        Ok(())
    }
}

fn sources() -> Vec<UsageSource> {
    vec![
        UsageSource {
            provider: UsageProvider::Claude,
            root: PathBuf::from(CLAUDE),
            max_depth: 4,
        },
        UsageSource {
            provider: UsageProvider::Codex,
            root: PathBuf::from(CODEX),
            max_depth: 5,
        },
    ]
}

fn make_with(fs: &Arc<MemoryFs>, now: Arc<Mutex<i64>>, chunk: Option<usize>) -> UsageStore {
    let mut opts = UsageStoreOptions::new(PathBuf::from(DIR), sources);
    opts.fs = fs.clone();
    opts.now = Box::new(move || *now.lock());
    if let Some(c) = chunk {
        opts.chunk_bytes = c;
    }
    UsageStore::new(opts)
}
fn make(fs: &Arc<MemoryFs>) -> UsageStore {
    make_with(fs, Arc::new(Mutex::new(T0)), None)
}

fn claude_with(id: &str, iso: &str, input: u64, output: u64, extra: Value) -> String {
    let mut v = json!({
        "type": "assistant", "requestId": format!("req_{id}"), "timestamp": iso, "cwd": "/home/u/proj",
        "message": {"id": format!("msg_{id}"), "model": "claude-opus-5",
            "usage": {"input_tokens": input, "output_tokens": output, "cache_creation_input_tokens": 0, "cache_read_input_tokens": 0}}
    });
    if let Some(e) = extra.as_object() {
        for (k, val) in e {
            v[k] = val.clone();
        }
    }
    format!("{v}\n")
}
fn claude(id: &str, iso: &str) -> String {
    claude_with(id, iso, 10, 5, json!({}))
}

fn c_row(iso: &str, ty: &str, payload: Value) -> String {
    format!(
        "{}\n",
        json!({"timestamp": iso, "type": ty, "payload": payload})
    )
}
fn u(input: u64, output: u64) -> Value {
    json!({"input_tokens": input, "cached_input_tokens": 0, "output_tokens": output, "reasoning_output_tokens": 0, "total_tokens": input + output})
}
fn codex_count(iso: &str, last: Value, total: Value, extra: Value) -> String {
    let mut p = json!({"type": "token_count", "info": {"last_token_usage": last, "total_token_usage": total}});
    for (k, v) in extra.as_object().unwrap() {
        p[k] = v.clone();
    }
    c_row(iso, "event_msg", p)
}
fn codex_head(id: &str) -> String {
    c_row(
        "2026-10-22T09:00:00.000Z",
        "session_meta",
        json!({"id": id, "cwd": "/w/proj"}),
    ) + &c_row(
        "2026-10-22T09:00:01.000Z",
        "turn_context",
        json!({"turn_id": "t1", "model": "gpt-5-codex"}),
    )
}

fn total(s: &UsageStore) -> u64 {
    s.buckets().iter().map(|b| b.tokens.total()).sum()
}
fn messages(s: &UsageStore) -> u64 {
    s.buckets().iter().map(|b| b.messages).sum()
}

// ── scan ────────────────────────────────────────────────────────────────────

#[test]
fn reads_claude_and_codex_files_with_hour_buckets_and_costs() {
    let fs = Arc::new(MemoryFs::default());
    fs.write(
        &format!("{CLAUDE}/p/s1.jsonl"),
        &(claude("1", "2026-10-22T10:15:00.000Z")
            + &claude("2", "2026-10-22T10:45:00.000Z")
            + &claude("3", "2026-10-22T11:05:00.000Z")),
    );
    fs.write(
        &format!("{CODEX}/2026/10/22/rollout-a.jsonl"),
        &(codex_head("s1")
            + &codex_count(
                "2026-10-22T09:10:00.000Z",
                u(100, 20),
                u(100, 20),
                json!({}),
            )),
    );
    let store = make(&fs);
    assert!(store.scan(None));
    let b = store.buckets();
    assert_eq!(b.len(), 3);
    let ten = b
        .iter()
        .find(|x| x.provider == UsageProvider::Claude && x.hour == ms("2026-10-22T10:00:00Z"))
        .unwrap();
    assert_eq!(
        (
            ten.messages,
            ten.project_key.as_str(),
            ten.unpriced_messages
        ),
        (2, "/home/u/proj", 0)
    );
    assert_eq!(ten.tokens.input, 20);
    assert!(ten.cost_usd > 0.0);
    let codex = b
        .iter()
        .find(|x| x.provider == UsageProvider::Codex)
        .unwrap();
    assert_eq!((codex.tokens.input, codex.tokens.output), (100, 20));
    let st = store.status();
    assert_eq!(
        (st.indexing, st.files, st.last_scan_at),
        (false, 2, Some(T0))
    );
    let now = Utc.timestamp_millis_opt(T0).unwrap();
    assert_eq!(
        store.summary(UsagePeriod::Today, &now, None).total_tokens,
        total(&store)
    );
    assert!(!store.scan(None));
}

#[test]
fn counts_only_appended_lines_on_the_next_scan() {
    let fs = Arc::new(MemoryFs::default());
    let path = format!("{CLAUDE}/p/s1.jsonl");
    fs.write(&path, &claude("1", "2026-10-22T10:00:00.000Z"));
    let store = make(&fs);
    store.scan(None);
    assert_eq!(messages(&store), 1);
    let before = fs.reads.load(Ordering::SeqCst);
    store.scan(None); // unchanged: not read
    assert_eq!(fs.reads.load(Ordering::SeqCst), before);
    fs.append_at(&path, &claude("2", "2026-10-22T10:30:00.000Z"), T0 - 1000);
    assert!(store.scan(None));
    assert_eq!(messages(&store), 2);
    assert_eq!(total(&store), 30);
}

#[test]
fn leaves_a_partial_last_line_for_the_next_scan() {
    let fs = Arc::new(MemoryFs::default());
    let path = format!("{CLAUDE}/p/s1.jsonl");
    let second = claude("2", "2026-10-22T10:30:00.000Z");
    fs.write(
        &path,
        &(claude("1", "2026-10-22T10:00:00.000Z") + &second[..40]),
    );
    let store = make(&fs);
    store.scan(None);
    assert_eq!(messages(&store), 1);
    fs.append_at(&path, &second[40..], T0 - 1000);
    store.scan(None);
    assert_eq!(messages(&store), 2);
}

#[test]
fn keeps_byte_offsets_right_when_a_multibyte_character_straddles_a_chunk_boundary() {
    let fs = Arc::new(MemoryFs::default());
    let path = format!("{CLAUDE}/p/s1.jsonl");
    let note = "héllo wörld ✓ 日本語 😀".repeat(3);
    let lines: Vec<String> = (1..=4)
        .map(|i| {
            claude_with(
                &i.to_string(),
                "2026-10-22T10:00:00.000Z",
                10,
                5,
                json!({"note": note}),
            )
        })
        .collect();
    fs.write(&path, &lines.concat());
    for chunk in [521, 600, 677, 801, 1003, 1500] {
        let store = make_with(&fs, Arc::new(Mutex::new(T0)), Some(chunk));
        store.scan(None);
        assert_eq!(messages(&store), 4, "chunk {chunk}");
        fs.append_at(
            &path,
            &claude_with(
                "5",
                "2026-10-22T11:00:00.000Z",
                10,
                5,
                json!({"note": "日本語"}),
            ),
            T0 - 1000,
        );
        store.scan(None);
        assert_eq!(messages(&store), 5, "chunk {chunk}");
        fs.write(&path, &lines.concat()); // back to the start for the next size
    }
}

#[test]
fn counts_a_claude_key_shared_by_two_files_once() {
    let fs = Arc::new(MemoryFs::default());
    let a = claude("1", "2026-10-22T10:00:00.000Z") + &claude("2", "2026-10-22T10:10:00.000Z");
    fs.write(&format!("{CLAUDE}/p/a.jsonl"), &a);
    fs.write(
        &format!("{CLAUDE}/p/b.jsonl"),
        &(a.clone()
            + &claude("3", "2026-10-22T10:20:00.000Z")
            + &claude("3", "2026-10-22T10:20:00.000Z")),
    );
    let store = make(&fs);
    store.scan(None);
    assert_eq!(messages(&store), 3);
}

#[test]
fn re_reads_a_replaced_file_without_double_counting_or_being_blocked_by_its_old_keys() {
    let fs = Arc::new(MemoryFs::default());
    let path = format!("{CLAUDE}/p/a.jsonl");
    fs.write(
        &path,
        &(claude("1", "2026-10-22T10:00:00.000Z")
            + &claude("2", "2026-10-22T10:10:00.000Z")
            + &claude("3", "2026-10-22T10:20:00.000Z")),
    );
    let store = make(&fs);
    store.scan(None);
    assert_eq!(messages(&store), 3);
    // shorter, same first key
    fs.replace(
        &path,
        &claude_with("1", "2026-10-22T10:00:00.000Z", 100, 5, json!({})),
    );
    assert!(store.scan(None));
    assert_eq!(messages(&store), 1);
    assert_eq!(store.buckets()[0].tokens.input, 100);
}

#[test]
fn keeps_the_totals_of_a_file_that_disappeared() {
    let fs = Arc::new(MemoryFs::default());
    let path = format!("{CLAUDE}/p/a.jsonl");
    fs.write(
        &path,
        &(claude("1", "2026-10-22T10:00:00.000Z") + &claude("2", "2026-10-22T10:10:00.000Z")),
    );
    let store = make(&fs);
    store.scan(None);
    fs.remove(&path);
    store.scan(None);
    assert_eq!(messages(&store), 2);
    assert_eq!(store.status().files, 0);
    store.stop();
    let again = make(&fs);
    again.load();
    assert_eq!(messages(&again), 2);
}

#[test]
fn a_truncated_file_is_read_again_from_the_start() {
    let fs = Arc::new(MemoryFs::default());
    let path = format!("{CLAUDE}/p/a.jsonl");
    fs.write(
        &path,
        &(claude("1", "2026-10-22T10:00:00.000Z") + &claude("2", "2026-10-22T10:10:00.000Z")),
    );
    let store = make(&fs);
    store.scan(None);
    // same inode, but smaller than what was read: truncated in place
    fs.write_at(
        &path,
        &claude("9", "2026-10-22T11:00:00.000Z"),
        T0 - 100,
        None,
    );
    assert!(store.scan(None));
    assert_eq!(messages(&store), 1);
}

// ── codex ───────────────────────────────────────────────────────────────────

#[test]
fn returns_the_newest_limits_across_files() {
    let fs = Arc::new(MemoryFs::default());
    let rl = |used: u64| json!({"rate_limits": {"plan_type": "plus", "primary": {"used_percent": used, "window_minutes": 300, "resets_at": 2e9}}});
    fs.write(
        &format!("{CODEX}/2026/10/22/a.jsonl"),
        &(codex_head("a") + &codex_count("2026-10-22T09:30:00.000Z", u(1, 1), u(1, 1), rl(10))),
    );
    fs.write(
        &format!("{CODEX}/2026/10/22/b.jsonl"),
        &(codex_head("b") + &codex_count("2026-10-22T10:30:00.000Z", u(1, 1), u(1, 1), rl(40))),
    );
    fs.write(
        &format!("{CODEX}/2026/10/22/c.jsonl"),
        &(codex_head("c") + &codex_count("2026-10-22T08:30:00.000Z", u(1, 1), u(1, 1), rl(70))),
    );
    let store = make(&fs);
    assert!(store.codex_limits().is_none());
    store.scan(None);
    let l = store.codex_limits().unwrap();
    assert_eq!(l.windows[0].used_percent, 40.0);
    assert_eq!(l.at, ms("2026-10-22T10:30:00.000Z"));
}

#[test]
fn flushes_a_compaction_only_once_the_file_is_quiet() {
    let fs = Arc::new(MemoryFs::default());
    let path = format!("{CODEX}/2026/10/22/a.jsonl");
    let record = json!({"response_id": "resp-c", "turn_id": "turn-1", "usage": u(5000, 700), "thread_token_usage": u(6000, 750)});
    let text = c_row(
        "2026-10-22T09:00:00.000Z",
        "session_meta",
        json!({"id": "s1", "cwd": "/w"}),
    ) + &c_row(
        "2026-10-22T09:00:01.000Z",
        "turn_context",
        json!({"turn_id": "turn-1", "model": "gpt-5-codex"}),
    ) + &codex_count(
        "2026-10-22T09:00:02.000Z",
        u(1000, 50),
        u(1000, 50),
        json!({}),
    ) + &c_row(
        "2026-10-22T09:00:03.000Z",
        "token_usage_record",
        record.clone(),
    ) + &c_row(
        "2026-10-22T09:00:03.000Z",
        "compacted",
        json!({"compaction_response_id": "resp-c", "latest_token_usage_record": record}),
    ) + &c_row(
        "2026-10-22T09:00:04.000Z",
        "turn_context",
        json!({"turn_id": "turn-1", "model": "gpt-5-codex"}),
    );
    fs.write_at(&path, &text, T0 - 60_000, None); // written a minute ago
    let now = Arc::new(Mutex::new(T0));
    let store = make_with(&fs, now.clone(), None);
    store.scan(None);
    assert_eq!(messages(&store), 1);
    *now.lock() = T0 + 5 * 60_000; // still not quiet (file untouched for 6 min)
    store.scan(None);
    assert_eq!(messages(&store), 1);
    *now.lock() = T0 + QUIET_MS; // untouched for 11 minutes, file itself unchanged
    assert!(store.scan(None));
    assert_eq!(messages(&store), 2);
    *now.lock() += 60 * 60_000;
    store.scan(None);
    assert_eq!(messages(&store), 2);
}

// ── persistence ─────────────────────────────────────────────────────────────

fn filled() -> (Arc<MemoryFs>, UsageStore) {
    let fs = Arc::new(MemoryFs::default());
    fs.write(
        &format!("{CLAUDE}/p/a.jsonl"),
        &(claude("1", "2026-10-22T10:00:00.000Z") + &claude("2", "2026-10-21T10:10:00.000Z")),
    );
    fs.write(
        &format!("{CODEX}/2026/10/22/a.jsonl"),
        &(codex_head("s")
            + &codex_count(
                "2026-10-22T09:10:00.000Z",
                u(100, 20),
                u(100, 20),
                json!({"rate_limits": {"primary": {"used_percent": 9, "window_minutes": 300, "resets_at": 2e9}}}),
            )),
    );
    let store = make(&fs);
    store.scan(None);
    (fs, store)
}

#[test]
fn restores_identical_buckets_and_limits_and_does_not_re_read_or_recount() {
    let (fs, store) = filled();
    store.stop();
    assert!(fs.cache().is_some());
    let next = make(&fs);
    next.load();
    assert_eq!(*next.buckets(), *store.buckets());
    assert_eq!(next.codex_limits(), store.codex_limits());
    let reads = fs.reads.load(Ordering::SeqCst);
    assert!(!next.scan(None));
    assert_eq!(fs.reads.load(Ordering::SeqCst), reads);
    fs.append_at(
        &format!("{CLAUDE}/p/a.jsonl"),
        &(claude("1", "2026-10-22T10:00:00.000Z") + &claude("9", "2026-10-22T12:00:00.000Z")),
        T0 - 10,
    );
    next.scan(None);
    assert_eq!(messages(&next), 4); // msg_1 was remembered as seen; only msg_9 is new
}

#[test]
fn starts_empty_on_a_pricing_version_mismatch_a_bad_file_or_a_wrong_version() {
    let (fs, store) = filled();
    store.stop();
    let good = fs.cache().unwrap();
    let pv = format!("\"pricingVersion\":{PRICING_VERSION}");
    assert!(good.contains(&pv));
    for bad in [
        good.replace(&pv, &format!("\"pricingVersion\":{}", PRICING_VERSION + 1)),
        good.replace("\"version\":1", "\"version\":2"),
        good[..50].to_string(),
        "null".to_string(),
    ] {
        fs.set_cache(&bad);
        let next = make(&fs);
        next.load();
        assert!(next.buckets().is_empty());
        assert_eq!(next.status().files, 0);
        next.scan(None);
        assert_eq!(*next.buckets(), *store.buckets()); // a full rescan gives the same totals
    }
}

#[test]
fn writes_at_most_once_per_30_seconds_while_scanning_and_on_flush() {
    let fs = Arc::new(MemoryFs::default());
    let path = format!("{CLAUDE}/p/a.jsonl");
    fs.write(&path, &claude("1", "2026-10-22T10:00:00.000Z"));
    let now = Arc::new(Mutex::new(T0));
    let store = make_with(&fs, now.clone(), None);
    store.scan(None);
    let writes = || fs.writes.load(Ordering::SeqCst);
    assert_eq!(writes(), 1);
    *now.lock() += 10_000;
    fs.append_at(&path, &claude("2", "2026-10-22T10:05:00.000Z"), *now.lock());
    store.scan(None);
    assert_eq!(writes(), 1);
    *now.lock() += 25_000;
    fs.append_at(&path, &claude("3", "2026-10-22T10:06:00.000Z"), *now.lock());
    store.scan(None);
    assert_eq!(writes(), 2);
    *now.lock() += 1000;
    fs.append_at(&path, &claude("4", "2026-10-22T10:07:00.000Z"), *now.lock());
    store.scan(None);
    store.flush();
    assert_eq!(writes(), 3);
    store.flush();
    assert_eq!(writes(), 3);
}

#[test]
fn never_stores_message_text() {
    let fs = Arc::new(MemoryFs::default());
    fs.write(
        &format!("{CLAUDE}/p/a.jsonl"),
        &claude_with(
            "1",
            "2026-10-22T10:00:00.000Z",
            10,
            5,
            json!({"secret": "SECRET-TEXT"}),
        ),
    );
    let store = make(&fs);
    store.scan(None);
    store.stop();
    assert!(!fs.cache().unwrap().contains("SECRET-TEXT"));
}

// ── scan control ────────────────────────────────────────────────────────────

#[test]
fn joins_concurrent_scans() {
    let fs = Arc::new(MemoryFs::default());
    fs.write(
        &format!("{CLAUDE}/p/a.jsonl"),
        &claude("1", "2026-10-22T10:00:00.000Z"),
    );
    fs.slow_walk_ms.store(120, Ordering::SeqCst);
    let store = Arc::new(make(&fs));
    let first = {
        let s = store.clone();
        thread::spawn(move || s.scan(None))
    };
    thread::sleep(Duration::from_millis(40));
    assert!(store.status().indexing);
    let second = {
        let s = store.clone();
        thread::spawn(move || s.scan(None))
    };
    assert!(first.join().unwrap());
    assert!(second.join().unwrap());
    assert_eq!(fs.walks.load(Ordering::SeqCst), 2); // two sources, one pass
    assert!(!store.status().indexing);
}

#[test]
fn reports_progress_with_totals() {
    let fs = Arc::new(MemoryFs::default());
    let a = claude("1", "2026-10-22T10:00:00.000Z") + &claude("2", "2026-10-22T10:01:00.000Z");
    let b = claude("3", "2026-10-22T10:02:00.000Z");
    fs.write(&format!("{CLAUDE}/p/a.jsonl"), &a);
    fs.write(&format!("{CLAUDE}/p/b.jsonl"), &b);
    let seen = Mutex::new(Vec::new());
    let store = make(&fs);
    store.scan(Some(&|p| seen.lock().push(p)));
    let seen = seen.into_inner();
    let total = (a.len() + b.len()) as u64;
    assert_eq!(
        seen[0],
        UsageScanProgress {
            files_done: 0,
            files_total: 2,
            bytes_done: 0,
            bytes_total: total
        }
    );
    assert_eq!(
        *seen.last().unwrap(),
        UsageScanProgress {
            files_done: 2,
            files_total: 2,
            bytes_done: total,
            bytes_total: total
        }
    );
    assert!(store.status().progress.is_none());
}

#[test]
fn prunes_buckets_older_than_400_days() {
    let fs = Arc::new(MemoryFs::default());
    let iso = |ms: i64| {
        Utc.timestamp_millis_opt(ms)
            .unwrap()
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
    };
    let old = iso(T0 - BUCKET_MAX_AGE_MS - 3 * 3_600_000);
    let recent_ms = T0 - 2 * 3_600_000;
    fs.write(
        &format!("{CLAUDE}/p/a.jsonl"),
        &(claude("1", &old) + &claude("2", &iso(recent_ms))),
    );
    let store = make(&fs);
    store.scan(None);
    assert_eq!(store.buckets().len(), 1);
    assert_eq!(
        store.buckets()[0].hour,
        recent_ms.div_euclid(3_600_000) * 3_600_000
    );
}

#[test]
fn default_sources_cover_every_profile_and_codex() {
    let home = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(home.path().join(".claude/projects")).unwrap();
    std::fs::create_dir_all(home.path().join(".claude-work/projects")).unwrap();
    let s = default_usage_sources(home.path(), None, None);
    let roots: Vec<_> = s
        .iter()
        .map(|x| {
            (
                x.provider,
                x.root
                    .strip_prefix(home.path())
                    .unwrap()
                    .to_string_lossy()
                    .into_owned(),
                x.max_depth,
            )
        })
        .collect();
    assert_eq!(
        roots,
        [
            (UsageProvider::Claude, ".claude/projects".to_string(), 4),
            (
                UsageProvider::Claude,
                ".claude-work/projects".to_string(),
                4
            ),
            (UsageProvider::Codex, ".codex/sessions".to_string(), 5),
            (
                UsageProvider::Codex,
                ".codex/archived_sessions".to_string(),
                2
            ),
        ]
    );
}
