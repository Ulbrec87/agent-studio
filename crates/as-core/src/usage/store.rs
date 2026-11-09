//! Incremental usage store (Hydra `electron/usage/UsageStore.ts`): per-file read offsets, hourly
//! buckets, a cross-file dedupe set and a JSON cache, so a rescan only reads appended bytes.
//!
//! * A pass walks the sources, compares each file with what it remembers (size, mtime, inode and
//!   the offset of the last complete line) and reads only what is new. A file that shrank or
//!   changed identity (rotated, replaced) is forgotten and re-read from the start.
//! * Every file keeps its own contribution (buckets + Codex limits); a file that disappears keeps
//!   what it added. Keys of the dedupe set point at the file that owns them.
//! * Message text is never stored: only counts, models, project keys and timestamps.
//! * The cache is `usage-cache.json` (mode 600, atomic write), at most every 30 s while scanning.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use chrono::{DateTime, TimeZone};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use super::claude_line::parse_claude_usage_line;
use super::codex_line::{
    CodexFileState, CodexParsedLimits, flush_codex_compactions, parse_codex_line,
};
use super::files::{StdUsageFs, UsageFileInfo, UsageFs};
use super::pricing::{PRICING_VERSION, estimate_cost};
use super::summary::summarize;
use super::types::{
    TokenCounts, UsageBucket, UsagePeriod, UsageProvider, UsageRecord, UsageScanProgress,
    UsageSummary,
};

pub const CACHE_VERSION: u32 = 1;
pub const CHUNK_BYTES: usize = 8 * 1024 * 1024;
/// A cache larger than this is not one we wrote and is not read.
pub const MAX_CACHE_BYTES: u64 = 200 * 1024 * 1024;
pub const SAVE_INTERVAL_MS: i64 = 30 * 1000;
pub const SEEN_MAX_AGE_MS: i64 = 60 * 24 * 60 * 60 * 1000;
pub const BUCKET_MAX_AGE_MS: i64 = 400 * 24 * 60 * 60 * 1000;
/// A Codex file untouched this long is not being written any more.
pub const QUIET_MS: i64 = 10 * 60 * 1000;
const HOUR_MS: i64 = 60 * 60 * 1000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageSource {
    pub provider: UsageProvider,
    pub root: PathBuf,
    pub max_depth: usize,
}

/// The default places to read: every Claude profile, and Codex's sessions and archive.
pub fn default_usage_sources(
    home: &Path,
    claude_config_dir: Option<&str>,
    codex_home: Option<&str>,
) -> Vec<UsageSource> {
    let codex_home = codex_home
        .filter(|c| !c.is_empty())
        .map_or_else(|| home.join(".codex"), PathBuf::from);
    let mut sources: Vec<UsageSource> =
        crate::sessions::profiles::discover_projects_dirs(home, claude_config_dir)
            .into_iter()
            .map(|root| UsageSource {
                provider: UsageProvider::Claude,
                root,
                max_depth: 4,
            })
            .collect();
    sources.push(UsageSource {
        provider: UsageProvider::Codex,
        root: codex_home.join("sessions"),
        max_depth: 5,
    });
    sources.push(UsageSource {
        provider: UsageProvider::Codex,
        root: codex_home.join("archived_sessions"),
        max_depth: 2,
    });
    sources
}

/// [`default_usage_sources`] for the real home dir, `$CLAUDE_CONFIG_DIR` and `$CODEX_HOME`.
pub fn default_usage_sources_from_env() -> Vec<UsageSource> {
    default_usage_sources(
        &crate::paths::home_dir(),
        std::env::var("CLAUDE_CONFIG_DIR").ok().as_deref(),
        std::env::var("CODEX_HOME").ok().as_deref(),
    )
}

pub struct UsageStoreOptions {
    /// `usage-cache.json`.
    pub cache_file: PathBuf,
    /// Re-evaluated on every scan.
    pub sources: Box<dyn Fn() -> Vec<UsageSource> + Send + Sync>,
    pub fs: Arc<dyn UsageFs>,
    pub now: Box<dyn Fn() -> i64 + Send + Sync>,
    /// Bytes read at a time (tests make it small).
    pub chunk_bytes: usize,
}

impl UsageStoreOptions {
    pub fn new(
        cache_file: PathBuf,
        sources: impl Fn() -> Vec<UsageSource> + Send + Sync + 'static,
    ) -> Self {
        Self {
            cache_file,
            sources: Box::new(sources),
            fs: Arc::new(StdUsageFs),
            now: Box::new(|| chrono::Utc::now().timestamp_millis()),
            chunk_bytes: CHUNK_BYTES,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct UsageStoreStatus {
    pub indexing: bool,
    pub progress: Option<UsageScanProgress>,
    pub last_scan_at: Option<i64>,
    pub files: usize,
}

/// What is known about a file that is still on disk.
#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
struct FileEntry {
    provider: UsageProvider,
    size: u64,
    mtime_ms: i64,
    ino: u64,
    /// Bytes consumed: the end of the last complete line read.
    offset: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    codex_state: Option<CodexFileState>,
}

/// What a file added; kept after the file is gone.
struct Contribution {
    provider: UsageProvider,
    buckets: HashMap<String, UsageBucket>,
    limits: Option<CodexParsedLimits>,
}

struct SeenEntry {
    file: String,
    ts: i64,
}

#[derive(Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
struct CacheContribution {
    path: String,
    provider: UsageProvider,
    buckets: Vec<UsageBucket>,
    limits: Option<CodexParsedLimits>,
}

#[derive(Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
struct CacheShape {
    version: u32,
    pricing_version: u32,
    last_scan_at: Option<i64>,
    files: HashMap<String, FileEntry>,
    contributions: Vec<CacheContribution>,
    /// `[key, index into contributions, ts]`
    seen: Vec<(String, usize, i64)>,
}

#[derive(Default)]
struct Inner {
    files: HashMap<String, FileEntry>,
    contributions: HashMap<String, Contribution>,
    seen: HashMap<String, SeenEntry>,
    merged: Option<Arc<Vec<UsageBucket>>>,
    last_scan_at: Option<i64>,
    dirty: bool,
    last_save_at: i64,
}

fn bucket_key(hour: i64, model: &str, project_key: &str) -> String {
    format!("{hour}|{model}|{project_key}")
}

fn path_key(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

struct Work {
    key: String,
    path: PathBuf,
    provider: UsageProvider,
    info: UsageFileInfo,
    fresh: bool,
    flush_only: bool,
}

impl Inner {
    /// Counts a record once. A key already owned (by this or another file) is skipped; a record
    /// without a key is always counted.
    fn add_record(
        &mut self,
        key: &str,
        provider: UsageProvider,
        record: &UsageRecord,
        dedupe_key: Option<&str>,
    ) -> bool {
        if let Some(dk) = dedupe_key {
            let seen_key = format!("{}:{dk}", provider_str(provider));
            if self.seen.contains_key(&seen_key) {
                return false;
            }
            self.seen.insert(
                seen_key,
                SeenEntry {
                    file: key.to_string(),
                    ts: record.ts,
                },
            );
        }
        let hour = record.ts.div_euclid(HOUR_MS) * HOUR_MS;
        let bkey = bucket_key(hour, &record.model, &record.project_key);
        let contribution = self
            .contributions
            .entry(key.to_string())
            .or_insert_with(|| Contribution {
                provider,
                buckets: HashMap::new(),
                limits: None,
            });
        let bucket = contribution
            .buckets
            .entry(bkey)
            .or_insert_with(|| UsageBucket {
                hour,
                provider,
                model: record.model.clone(),
                project_key: record.project_key.clone(),
                tokens: TokenCounts::default(),
                messages: 0,
                cost_usd: 0.0,
                unpriced_messages: 0,
            });
        bucket.tokens = bucket.tokens.add(&record.tokens);
        bucket.messages += 1;
        match estimate_cost(&record.model, &record.tokens) {
            None => bucket.unpriced_messages += 1,
            Some(cost) => bucket.cost_usd += cost,
        }
        true
    }

    /// Drops what a file added and the keys it owned. True when there was anything.
    fn forget(&mut self, key: &str) -> bool {
        let had = self.contributions.remove(key).is_some();
        self.seen.retain(|_, e| e.file != key);
        had
    }

    fn prune(&mut self, now: i64) -> bool {
        let mut changed = false;
        let cutoff_seen = now - SEEN_MAX_AGE_MS;
        let before = self.seen.len();
        self.seen.retain(|_, e| e.ts >= cutoff_seen);
        if self.seen.len() != before {
            self.dirty = true;
        }
        let cutoff = now - BUCKET_MAX_AGE_MS;
        let files = &self.files;
        self.contributions.retain(|path, c| {
            let before = c.buckets.len();
            c.buckets.retain(|_, b| b.hour >= cutoff);
            if c.buckets.len() != before {
                changed = true;
            }
            !(c.buckets.is_empty() && c.limits.is_none() && !files.contains_key(path))
        });
        if changed {
            self.dirty = true;
        }
        changed
    }
}

fn provider_str(p: UsageProvider) -> &'static str {
    match p {
        UsageProvider::Claude => "claude",
        UsageProvider::Codex => "codex",
    }
}

pub struct UsageStore {
    opts: UsageStoreOptions,
    inner: Mutex<Inner>,
    scan_lock: Mutex<()>,
    running: AtomicBool,
    last_changed: AtomicBool,
    progress: Mutex<Option<UsageScanProgress>>,
}

impl UsageStore {
    pub fn new(opts: UsageStoreOptions) -> Self {
        let mut opts = opts;
        opts.chunk_bytes = opts.chunk_bytes.max(1);
        Self {
            opts,
            inner: Mutex::new(Inner::default()),
            scan_lock: Mutex::new(()),
            running: AtomicBool::new(false),
            last_changed: AtomicBool::new(false),
            progress: Mutex::new(None),
        }
    }

    fn now(&self) -> i64 {
        (self.opts.now)()
    }

    // ── persistence ─────────────────────────────────────────────────────────

    /// Restores the cache. A missing, old or unreadable cache starts empty (one full rescan).
    pub fn load(&self) {
        let mut inner = self.inner.lock();
        *inner = Inner::default();
        let Ok(Some(text)) = self
            .opts
            .fs
            .read_text(&self.opts.cache_file, MAX_CACHE_BYTES)
        else {
            return;
        };
        let Ok(data) = serde_json::from_str::<CacheShape>(&text) else {
            return;
        };
        if data.version != CACHE_VERSION || data.pricing_version != PRICING_VERSION {
            return;
        }
        let mut contributions = HashMap::new();
        let mut order = Vec::new();
        for c in data.contributions {
            let buckets = c
                .buckets
                .into_iter()
                .map(|b| (bucket_key(b.hour, &b.model, &b.project_key), b))
                .collect();
            order.push(c.path.clone());
            contributions.insert(
                c.path,
                Contribution {
                    provider: c.provider,
                    buckets,
                    limits: c.limits,
                },
            );
        }
        let mut seen = HashMap::new();
        for (key, idx, ts) in data.seen {
            let Some(file) = order.get(idx) else { return };
            seen.insert(
                key,
                SeenEntry {
                    file: file.clone(),
                    ts,
                },
            );
        }
        inner.files = data.files;
        inner.contributions = contributions;
        inner.seen = seen;
        inner.last_scan_at = data.last_scan_at;
    }

    /// Writes the cache now if anything changed since the last write.
    pub fn flush(&self) {
        let mut inner = self.inner.lock();
        self.flush_locked(&mut inner);
    }

    fn flush_locked(&self, inner: &mut Inner) {
        if !inner.dirty {
            return;
        }
        let paths: Vec<&String> = inner.contributions.keys().collect();
        let index: HashMap<&String, usize> =
            paths.iter().enumerate().map(|(i, p)| (*p, i)).collect();
        let cache = CacheShape {
            version: CACHE_VERSION,
            pricing_version: PRICING_VERSION,
            last_scan_at: inner.last_scan_at,
            files: inner.files.clone(),
            contributions: paths
                .iter()
                .map(|path| {
                    let c = &inner.contributions[*path];
                    CacheContribution {
                        path: (*path).clone(),
                        provider: c.provider,
                        buckets: c.buckets.values().cloned().collect(),
                        limits: c.limits.clone(),
                    }
                })
                .collect(),
            seen: inner
                .seen
                .iter()
                .filter_map(|(key, e)| index.get(&e.file).map(|i| (key.clone(), *i, e.ts)))
                .collect(),
        };
        let Ok(text) = serde_json::to_string(&cache) else {
            return;
        };
        if self
            .opts
            .fs
            .write_atomic(&self.opts.cache_file, text.as_bytes())
            .is_ok()
        {
            inner.dirty = false;
            inner.last_save_at = self.now();
        } // else: stays dirty; tried again at the next save
    }

    pub fn stop(&self) {
        self.flush();
    }

    // ── reading results ─────────────────────────────────────────────────────

    pub fn buckets(&self) -> Arc<Vec<UsageBucket>> {
        let mut inner = self.inner.lock();
        if let Some(m) = &inner.merged {
            return m.clone();
        }
        let mut sum: HashMap<String, UsageBucket> = HashMap::new();
        for c in inner.contributions.values() {
            for b in c.buckets.values() {
                let key = format!(
                    "{}|{}",
                    provider_str(b.provider),
                    bucket_key(b.hour, &b.model, &b.project_key)
                );
                match sum.get_mut(&key) {
                    None => {
                        sum.insert(key, b.clone());
                    }
                    Some(into) => {
                        into.tokens = into.tokens.add(&b.tokens);
                        into.messages += b.messages;
                        into.cost_usd += b.cost_usd;
                        into.unpriced_messages += b.unpriced_messages;
                    }
                }
            }
        }
        let mut merged: Vec<UsageBucket> = sum.into_values().collect();
        merged.sort_by(|a, b| {
            a.hour
                .cmp(&b.hour)
                .then_with(|| a.provider.cmp(&b.provider))
                .then_with(|| a.model.cmp(&b.model))
                .then_with(|| a.project_key.cmp(&b.project_key))
        });
        let merged = Arc::new(merged);
        inner.merged = Some(merged.clone());
        merged
    }

    pub fn summary<Tz: TimeZone>(
        &self,
        period: UsagePeriod,
        now: &DateTime<Tz>,
        project_key: Option<&str>,
    ) -> UsageSummary {
        summarize(&self.buckets(), period, now, project_key)
    }

    /// The newest Codex rate limits seen in any file.
    pub fn codex_limits(&self) -> Option<CodexParsedLimits> {
        let inner = self.inner.lock();
        inner
            .contributions
            .values()
            .filter_map(|c| c.limits.as_ref())
            .max_by_key(|l| l.at)
            .cloned()
    }

    pub fn status(&self) -> UsageStoreStatus {
        let inner = self.inner.lock();
        UsageStoreStatus {
            indexing: self.running.load(Ordering::SeqCst),
            progress: *self.progress.lock(),
            last_scan_at: inner.last_scan_at,
            files: inner.files.len(),
        }
    }

    // ── scanning ────────────────────────────────────────────────────────────

    /// One incremental pass; returns `true` when the totals or limits changed. A call made during
    /// a pass waits for it and returns its result instead of starting another.
    pub fn scan(&self, on_progress: Option<&(dyn Fn(UsageScanProgress) + Sync)>) -> bool {
        let Some(_guard) = self.scan_lock.try_lock() else {
            let _wait = self.scan_lock.lock();
            return self.last_changed.load(Ordering::SeqCst);
        };
        self.running.store(true, Ordering::SeqCst);
        let changed = self.run_scan(on_progress);
        self.running.store(false, Ordering::SeqCst);
        *self.progress.lock() = None;
        self.last_changed.store(changed, Ordering::SeqCst);
        changed
    }

    fn report(
        &self,
        progress: &UsageScanProgress,
        cb: Option<&(dyn Fn(UsageScanProgress) + Sync)>,
    ) {
        *self.progress.lock() = Some(*progress);
        if let Some(cb) = cb {
            cb(*progress);
        }
    }

    fn run_scan(&self, on_progress: Option<&(dyn Fn(UsageScanProgress) + Sync)>) -> bool {
        let started_at = self.now();
        let mut found: Vec<(String, PathBuf, UsageProvider, UsageFileInfo)> = Vec::new();
        let mut known: HashSet<String> = HashSet::new();
        for source in (self.opts.sources)() {
            for info in self.opts.fs.walk(&source.root, source.max_depth) {
                let key = path_key(&info.path);
                if known.insert(key.clone()) {
                    found.push((key, info.path.clone(), source.provider, info));
                }
            }
        }

        let mut work: Vec<Work> = Vec::new();
        let mut bytes_total = 0u64;
        {
            let mut inner = self.inner.lock();
            for (key, path, provider, info) in found {
                let entry = inner.files.get(&key);
                let fresh = entry.is_none_or(|e| {
                    e.provider != provider
                        || e.ino != info.ino
                        || info.size < e.offset
                        || info.size < e.size
                });
                let same = !fresh
                    && entry.is_some_and(|e| e.size == info.size && e.mtime_ms == info.mtime_ms);
                if same {
                    let quiet_flush = provider == UsageProvider::Codex
                        && entry.is_some_and(|e| {
                            e.codex_state
                                .as_ref()
                                .is_some_and(|s| !s.pending_compactions.is_empty())
                        })
                        && started_at - info.mtime_ms > QUIET_MS;
                    if quiet_flush {
                        work.push(Work {
                            key,
                            path,
                            provider,
                            info,
                            fresh: false,
                            flush_only: true,
                        });
                    }
                    continue;
                }
                bytes_total += if fresh {
                    info.size
                } else {
                    info.size.saturating_sub(entry.map_or(0, |e| e.offset))
                };
                work.push(Work {
                    key,
                    path,
                    provider,
                    info,
                    fresh,
                    flush_only: false,
                });
            }
            // Files that are gone keep what they added; only the read position is forgotten.
            let before = inner.files.len();
            inner.files.retain(|k, _| known.contains(k));
            if inner.files.len() != before {
                inner.dirty = true;
            }
        }

        let mut progress = UsageScanProgress {
            files_done: 0,
            files_total: work.len() as u64,
            bytes_done: 0,
            bytes_total,
        };
        self.report(&progress, on_progress);

        let mut changed = false;
        for w in &work {
            if self.process_file(w, started_at, &mut progress, on_progress) {
                changed = true;
            }
            progress.files_done += 1;
            self.report(&progress, on_progress);
        }

        let mut inner = self.inner.lock();
        if inner.prune(started_at) {
            changed = true;
        }
        if changed {
            inner.merged = None;
        }
        inner.last_scan_at = Some(started_at);
        if changed || !work.is_empty() {
            inner.dirty = true;
            if started_at - inner.last_save_at >= SAVE_INTERVAL_MS {
                self.flush_locked(&mut inner);
            }
        }
        changed
    }

    /// Reads what a file gained. Returns true when its contribution changed.
    fn process_file(
        &self,
        w: &Work,
        now: i64,
        progress: &mut UsageScanProgress,
        on_progress: Option<&(dyn Fn(UsageScanProgress) + Sync)>,
    ) -> bool {
        let mut changed = false;
        {
            let mut inner = self.inner.lock();
            if w.fresh {
                changed = inner.forget(&w.key);
                inner.files.insert(
                    w.key.clone(),
                    FileEntry {
                        provider: w.provider,
                        size: 0,
                        mtime_ms: 0,
                        ino: w.info.ino,
                        offset: 0,
                        codex_state: None,
                    },
                );
            }
            if let Some(entry) = inner.files.get_mut(&w.key)
                && w.provider == UsageProvider::Codex
                && entry.codex_state.is_none()
            {
                entry.codex_state = Some(CodexFileState::new());
            }
            inner
                .contributions
                .entry(w.key.clone())
                .or_insert_with(|| Contribution {
                    provider: w.provider,
                    buckets: HashMap::new(),
                    limits: None,
                });
        }

        if !w.flush_only {
            let chunk_bytes = self.opts.chunk_bytes;
            let mut ok = true;
            loop {
                let offset = self.inner.lock().files.get(&w.key).map_or(0, |e| e.offset);
                let Ok(chunk) = self.opts.fs.read_chunk(&w.path, offset, chunk_bytes) else {
                    ok = false; // gone or not an ordinary file any more: tried again at the next scan
                    break;
                };
                if chunk.is_empty() {
                    break;
                }
                let consumed = match chunk.iter().rposition(|b| *b == b'\n') {
                    Some(last) => last + 1,
                    // a line longer than a chunk is no record
                    None if chunk.len() >= chunk_bytes => chunk.len(),
                    // a partial last line waits
                    None => break,
                };
                {
                    let mut guard = self.inner.lock();
                    let inner = &mut *guard;
                    // Parser state moves out of the entry while the lines are fed.
                    let mut codex = inner
                        .files
                        .get_mut(&w.key)
                        .and_then(|e| e.codex_state.take());
                    if chunk[..consumed].contains(&b'\n') {
                        for raw in chunk[..consumed].split(|b| *b == b'\n') {
                            if raw.is_empty() {
                                continue;
                            }
                            let line = String::from_utf8_lossy(raw);
                            if consume_line(inner, &w.key, w.provider, codex.as_mut(), &line) {
                                changed = true;
                            }
                        }
                    }
                    if let Some(entry) = inner.files.get_mut(&w.key) {
                        entry.codex_state = codex;
                        entry.offset += consumed as u64;
                    }
                    inner.dirty = true;
                }
                progress.bytes_done += consumed as u64;
                if chunk.len() < chunk_bytes {
                    break;
                }
                self.report(progress, on_progress);
            }
            if ok && let Some(entry) = self.inner.lock().files.get_mut(&w.key) {
                entry.size = w.info.size;
                entry.mtime_ms = w.info.mtime_ms;
                entry.ino = w.info.ino;
            }
        }

        if w.provider == UsageProvider::Codex && now - w.info.mtime_ms > QUIET_MS {
            let mut guard = self.inner.lock();
            let inner = &mut *guard;
            let flushed = inner
                .files
                .get_mut(&w.key)
                .and_then(|e| e.codex_state.as_mut())
                .map(flush_codex_compactions)
                .unwrap_or_default();
            for u in flushed {
                if inner.add_record(&w.key, w.provider, &u.record, u.dedupe_key.as_deref()) {
                    changed = true;
                }
                inner.dirty = true;
            }
        }
        changed
    }
}

fn consume_line(
    inner: &mut Inner,
    key: &str,
    provider: UsageProvider,
    codex: Option<&mut CodexFileState>,
    line: &str,
) -> bool {
    match provider {
        UsageProvider::Claude => parse_claude_usage_line(line)
            .is_some_and(|p| inner.add_record(key, provider, &p.record, p.dedupe_key.as_deref())),
        UsageProvider::Codex => {
            let Some(state) = codex else { return false };
            let Some(parsed) = parse_codex_line(line, state) else {
                return false;
            };
            let mut changed = false;
            for u in parsed.usage.unwrap_or_default() {
                if inner.add_record(key, provider, &u.record, u.dedupe_key.as_deref()) {
                    changed = true;
                }
            }
            if let Some(limits) = parsed.limits
                && let Some(c) = inner.contributions.get_mut(key)
                && c.limits.as_ref().is_none_or(|l| limits.at >= l.at)
            {
                c.limits = Some(limits);
                changed = true;
            }
            changed
        }
    }
}

#[cfg(test)]
mod tests;
