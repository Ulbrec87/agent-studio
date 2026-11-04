//! Daemon-side owner of the usage store and the Claude limits client (Hydra
//! `electron/usage/UsageService.ts`). Synchronous and clock-injected: the daemon calls
//! [`UsageService::tick`] every few seconds from a blocking thread and the service decides what is
//! due; listeners get a [`UsageState`] whenever something a client shows changed.
//!
//! Cadence: scan the logs every 60 s; poll the plan limits every 5 min, every 60 s while a client
//! shows a usage surface ([`UsageService::set_visible`]), and once more after a Claude agent went
//! from working to idle (throttled to 60 s, [`UsageService::notify_agent_idle`]).

use std::sync::Arc;

use chrono::Local;
use parking_lot::Mutex;

use super::codex_line::CodexParsedLimits;
use super::limits_client::ClaudeLimitsClient;
use super::report::{
    REPORT_MAX_DAYS, REPORT_MIN_DAYS, UsageReport, UsageReportProvider, build_usage_report,
};
use super::store::{UsageStore, UsageStoreStatus};
use super::types::{
    ClaudeProfileLimits, IndexingState, UsageBucket, UsagePeriod, UsageScanProgress, UsageState,
    UsageSummary,
};

/// The slice of [`UsageStore`] the service uses.
pub trait UsageStoreLike: Send + Sync {
    fn load(&self);
    fn scan(&self, on_progress: Option<&(dyn Fn(UsageScanProgress) + Sync)>) -> bool;
    fn summary(&self, period: UsagePeriod, project_key: Option<&str>) -> UsageSummary;
    fn buckets(&self) -> Arc<Vec<UsageBucket>>;
    fn codex_limits(&self) -> Option<CodexParsedLimits>;
    fn status(&self) -> UsageStoreStatus;
    fn stop(&self);
}

impl UsageStoreLike for UsageStore {
    fn load(&self) {
        UsageStore::load(self);
    }
    fn scan(&self, on_progress: Option<&(dyn Fn(UsageScanProgress) + Sync)>) -> bool {
        UsageStore::scan(self, on_progress)
    }
    fn summary(&self, period: UsagePeriod, project_key: Option<&str>) -> UsageSummary {
        UsageStore::summary(self, period, &Local::now(), project_key)
    }
    fn buckets(&self) -> Arc<Vec<UsageBucket>> {
        UsageStore::buckets(self)
    }
    fn codex_limits(&self) -> Option<CodexParsedLimits> {
        UsageStore::codex_limits(self)
    }
    fn status(&self) -> UsageStoreStatus {
        UsageStore::status(self)
    }
    fn stop(&self) {
        UsageStore::stop(self);
    }
}

/// The slice of [`ClaudeLimitsClient`] the service uses.
pub trait ClaudeLimitsLike: Send + Sync {
    fn refresh(&self) -> Vec<ClaudeProfileLimits>;
    fn current(&self) -> Vec<ClaudeProfileLimits>;
}

impl ClaudeLimitsLike for ClaudeLimitsClient {
    fn refresh(&self) -> Vec<ClaudeProfileLimits> {
        ClaudeLimitsClient::refresh(self)
    }
    fn current(&self) -> Vec<ClaudeProfileLimits> {
        ClaudeLimitsClient::current(self)
    }
}

pub const SCAN_INTERVAL_MS: i64 = 60_000;
pub const LIMITS_INTERVAL_MS: i64 = 5 * 60_000;
pub const LIMITS_VISIBLE_INTERVAL_MS: i64 = 60_000;
pub const IDLE_THROTTLE_MS: i64 = 60_000;
pub const PROGRESS_EMIT_MS: i64 = 500;

pub type Listener = Arc<dyn Fn(&UsageState) + Send + Sync>;

pub struct UsageServiceOptions {
    pub store: Arc<dyn UsageStoreLike>,
    pub limits: Arc<dyn ClaudeLimitsLike>,
    pub now: Box<dyn Fn() -> i64 + Send + Sync>,
}

#[derive(Default)]
struct Sched {
    started: bool,
    visible: bool,
    last_scan_at: Option<i64>,
    last_limits_at: Option<i64>,
    last_idle_at: Option<i64>,
    last_progress_emit_at: Option<i64>,
    data_version: u64,
    last_key: String,
}

pub struct UsageService {
    store: Arc<dyn UsageStoreLike>,
    limits: Arc<dyn ClaudeLimitsLike>,
    now: Box<dyn Fn() -> i64 + Send + Sync>,
    sched: Mutex<Sched>,
    listeners: Mutex<Vec<Listener>>,
    refresh_lock: Mutex<()>,
}

impl UsageService {
    pub fn new(opts: UsageServiceOptions) -> Self {
        Self {
            store: opts.store,
            limits: opts.limits,
            now: opts.now,
            sched: Mutex::new(Sched::default()),
            listeners: Mutex::new(Vec::new()),
            refresh_lock: Mutex::new(()),
        }
    }

    fn now(&self) -> i64 {
        (self.now)()
    }

    /// Called with a fresh state whenever something a client shows changed.
    pub fn on_changed(&self, listener: Listener) {
        self.listeners.lock().push(listener);
    }

    /// Restores the cache and arms the schedule: the first [`tick`](Self::tick) scans and polls at
    /// once (the caller runs it in the background, so a big first index never blocks startup).
    pub fn start(&self) {
        self.store.load(); // a missing or bad cache just starts empty
        let mut s = self.sched.lock();
        s.started = true;
        s.last_scan_at = None;
        s.last_limits_at = None;
    }

    pub fn stop(&self) {
        self.sched.lock().started = false;
        self.store.stop(); // flushes the cache
    }

    /// A client shows (or stopped showing) a usage surface: the limits are polled every 60 s
    /// instead of every 5 min while it does. Follow with a `tick` to poll at once.
    pub fn set_visible(&self, visible: bool) {
        self.sched.lock().visible = visible;
    }

    pub fn is_visible(&self) -> bool {
        self.sched.lock().visible
    }

    /// Does whatever is due: the log scan and the limits poll. Blocking.
    pub fn tick(&self) {
        let now = self.now();
        let (scan_due, limits_due) = {
            let s = self.sched.lock();
            if !s.started {
                return;
            }
            let every = if s.visible {
                LIMITS_VISIBLE_INTERVAL_MS
            } else {
                LIMITS_INTERVAL_MS
            };
            (
                s.last_scan_at.is_none_or(|t| now - t >= SCAN_INTERVAL_MS),
                s.last_limits_at.is_none_or(|t| now - t >= every),
            )
        };
        if scan_due {
            self.scan_now();
        }
        if limits_due {
            self.refresh_limits();
        }
    }

    /// A Claude agent went from working to idle: its transcript and the limits just moved.
    /// Throttled to once per 60 s. Blocking.
    pub fn notify_agent_idle(&self) {
        let now = self.now();
        {
            let mut s = self.sched.lock();
            if s.last_idle_at.is_some_and(|t| now - t < IDLE_THROTTLE_MS) {
                return;
            }
            s.last_idle_at = Some(now);
        }
        self.scan_now();
        self.refresh_limits();
    }

    /// Scan and poll now; calls made meanwhile wait for that run. Blocking.
    pub fn refresh(&self) -> UsageState {
        match self.refresh_lock.try_lock() {
            Some(_guard) => {
                self.scan_now();
                self.refresh_limits();
            }
            None => {
                let _wait = self.refresh_lock.lock();
            }
        }
        self.state()
    }

    pub fn state(&self) -> UsageState {
        let status = self.store.status();
        // Only the first full index is shown; the sub-second incremental passes would just flicker.
        let indexing = status.indexing && status.last_scan_at.is_none();
        UsageState {
            claude: self.limits.current(),
            codex: self.store.codex_limits(),
            indexing: IndexingState {
                indexing,
                progress: if indexing { status.progress } else { None },
                last_scan_at: status.last_scan_at,
            },
            data_version: self.sched.lock().data_version,
            updated_at: self.now(),
        }
    }

    pub fn summary(&self, period: UsagePeriod, project_key: Option<&str>) -> UsageSummary {
        self.store.summary(period, project_key)
    }

    /// `days` is clamped to 7-90.
    pub fn report(&self, days: u32, provider: UsageReportProvider) -> UsageReport {
        build_usage_report(
            &self.store.buckets(),
            days.clamp(REPORT_MIN_DAYS, REPORT_MAX_DAYS),
            provider,
            &Local::now(),
        )
    }

    pub fn buckets(&self) -> Arc<Vec<UsageBucket>> {
        self.store.buckets()
    }

    fn scan_now(&self) {
        let changed = self.store.scan(Some(&|_| self.emit_if_changed()));
        {
            let mut s = self.sched.lock();
            s.last_scan_at = Some(self.now());
            if changed {
                s.data_version += 1;
            }
        }
        self.emit_if_changed();
    }

    fn refresh_limits(&self) {
        self.sched.lock().last_limits_at = Some(self.now());
        self.limits.refresh();
        self.emit_if_changed();
    }

    fn emit_if_changed(&self) {
        let state = self.state();
        let now = self.now();
        let key = {
            let mut s = self.sched.lock();
            if state.indexing.indexing {
                // Progress states (from any trigger) are at most one per PROGRESS_EMIT_MS.
                if s.last_progress_emit_at
                    .is_some_and(|t| now - t < PROGRESS_EMIT_MS)
                {
                    return;
                }
                s.last_progress_emit_at = Some(now);
            }
            let mut comparable = state.clone();
            comparable.updated_at = 0;
            let key = serde_json::to_string(&comparable).unwrap_or_default();
            if key == s.last_key {
                return;
            }
            s.last_key.clone_from(&key);
            key
        };
        let _ = key;
        let listeners: Vec<Listener> = self.listeners.lock().clone();
        for l in listeners {
            l(&state);
        }
    }
}

#[cfg(test)]
mod tests;
