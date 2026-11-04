use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread;
use std::time::Duration;

use super::*;
use crate::usage::credentials::{CredentialResult, Secret};
use crate::usage::limits_client::{
    ClaudeLimitsClientOptions, ClientProfile, CredentialReader, HttpClient, HttpError, HttpRequest,
    HttpResponse,
};
use crate::usage::types::ProfileStatus;

const T0: i64 = 1_792_749_600_000; // 2026-10-23T10:00:00Z

#[derive(Default)]
struct FakeStore {
    loaded: AtomicUsize,
    stopped: AtomicUsize,
    scans: AtomicUsize,
    last_scan_at: Mutex<Option<i64>>,
    indexing: AtomicBool,
    progress: Mutex<Option<UsageScanProgress>>,
    changed_next: AtomicBool,
    delay_ms: AtomicUsize,
    summaries: Mutex<Vec<(UsagePeriod, Option<String>)>>,
    clock: Arc<Mutex<i64>>,
    /// Progress steps reported by the next scan, with the clock advance before each.
    steps: Mutex<Vec<(i64, u64)>>,
}

impl UsageStoreLike for FakeStore {
    fn load(&self) {
        self.loaded.fetch_add(1, Ordering::SeqCst);
    }
    fn scan(&self, on_progress: Option<&(dyn Fn(UsageScanProgress) + Sync)>) -> bool {
        self.scans.fetch_add(1, Ordering::SeqCst);
        self.indexing.store(true, Ordering::SeqCst);
        let steps = std::mem::take(&mut *self.steps.lock());
        for (advance, n) in steps {
            *self.clock.lock() += advance;
            let p = UsageScanProgress {
                files_done: n,
                files_total: 9,
                bytes_done: 0,
                bytes_total: 1,
            };
            *self.progress.lock() = Some(p);
            if let Some(cb) = on_progress {
                cb(p);
            }
        }
        let delay = self.delay_ms.load(Ordering::SeqCst);
        if delay > 0 {
            thread::sleep(Duration::from_millis(delay as u64));
        }
        self.indexing.store(false, Ordering::SeqCst);
        *self.progress.lock() = None;
        *self.last_scan_at.lock() = Some(*self.clock.lock());
        self.changed_next.swap(false, Ordering::SeqCst)
    }
    fn summary(&self, period: UsagePeriod, project_key: Option<&str>) -> UsageSummary {
        self.summaries
            .lock()
            .push((period, project_key.map(String::from)));
        crate::usage::summary::summarize(&[], period, &chrono::Utc::now(), project_key)
    }
    fn buckets(&self) -> Arc<Vec<UsageBucket>> {
        Arc::new(Vec::new())
    }
    fn codex_limits(&self) -> Option<CodexParsedLimits> {
        None
    }
    fn status(&self) -> UsageStoreStatus {
        UsageStoreStatus {
            indexing: self.indexing.load(Ordering::SeqCst),
            progress: *self.progress.lock(),
            last_scan_at: *self.last_scan_at.lock(),
            files: 0,
        }
    }
    fn stop(&self) {
        self.stopped.fetch_add(1, Ordering::SeqCst);
    }
}

#[derive(Default)]
struct FakeLimits {
    calls: AtomicUsize,
    data: Mutex<Vec<ClaudeProfileLimits>>,
}
impl ClaudeLimitsLike for FakeLimits {
    fn refresh(&self) -> Vec<ClaudeProfileLimits> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.data.lock().clone()
    }
    fn current(&self) -> Vec<ClaudeProfileLimits> {
        self.data.lock().clone()
    }
}

struct Rig {
    store: Arc<FakeStore>,
    limits: Arc<FakeLimits>,
    service: Arc<UsageService>,
    changed: Arc<Mutex<Vec<UsageState>>>,
    clock: Arc<Mutex<i64>>,
}
impl Rig {
    fn advance(&self, ms: i64) {
        *self.clock.lock() += ms;
    }
    fn scans(&self) -> usize {
        self.store.scans.load(Ordering::SeqCst)
    }
    fn polls(&self) -> usize {
        self.limits.calls.load(Ordering::SeqCst)
    }
}

fn make() -> Rig {
    let clock = Arc::new(Mutex::new(T0));
    let store = Arc::new(FakeStore {
        clock: clock.clone(),
        ..Default::default()
    });
    let limits = Arc::new(FakeLimits::default());
    let c = clock.clone();
    let service = Arc::new(UsageService::new(UsageServiceOptions {
        store: store.clone(),
        limits: limits.clone(),
        now: Box::new(move || *c.lock()),
    }));
    let changed = Arc::new(Mutex::new(Vec::new()));
    let sink = changed.clone();
    service.on_changed(Arc::new(move |s| sink.lock().push(s.clone())));
    Rig {
        store,
        limits,
        service,
        changed,
        clock,
    }
}

#[test]
fn loads_then_the_first_tick_scans_and_polls() {
    let rig = make();
    rig.service.start();
    assert_eq!(rig.store.loaded.load(Ordering::SeqCst), 1);
    assert_eq!((rig.scans(), rig.polls()), (0, 0)); // start() itself never blocks on a scan
    rig.service.tick();
    assert_eq!((rig.scans(), rig.polls()), (1, 1));
}

#[test]
fn ticks_do_nothing_before_start_and_after_stop() {
    let rig = make();
    rig.service.tick();
    assert_eq!(rig.scans(), 0);
    rig.service.start();
    rig.service.stop();
    rig.service.tick();
    assert_eq!(rig.scans(), 0);
    assert_eq!(rig.store.stopped.load(Ordering::SeqCst), 1);
}

#[test]
fn scans_every_60_seconds() {
    let rig = make();
    rig.service.start();
    for _ in 0..4 {
        rig.service.tick();
        rig.advance(SCAN_INTERVAL_MS / 2);
        rig.service.tick(); // not due yet
        rig.advance(SCAN_INTERVAL_MS / 2);
    }
    assert_eq!(rig.scans(), 4);
}

#[test]
fn polls_limits_every_5_minutes_or_every_60_seconds_while_visible() {
    let rig = make();
    rig.service.start();
    rig.service.tick();
    assert_eq!(rig.polls(), 1);
    rig.advance(LIMITS_INTERVAL_MS - 10_000);
    rig.service.tick();
    assert_eq!(rig.polls(), 1);
    rig.advance(10_000);
    rig.service.tick();
    assert_eq!(rig.polls(), 2);

    // a surface becomes visible with a stale poll: fetched at once on the next tick
    rig.advance(60_000);
    rig.service.set_visible(true);
    rig.service.tick();
    assert_eq!(rig.polls(), 3);
    // visible: every 60 s
    rig.advance(30_000);
    rig.service.tick();
    assert_eq!(rig.polls(), 3);
    rig.advance(30_000);
    rig.service.tick();
    assert_eq!(rig.polls(), 4);
    // hidden again: back to 5 minutes
    rig.service.set_visible(false);
    rig.advance(2 * 60_000);
    rig.service.tick();
    assert_eq!(rig.polls(), 4);
    rig.advance(LIMITS_INTERVAL_MS);
    rig.service.tick();
    assert_eq!(rig.polls(), 5);
}

#[test]
fn throttles_the_agent_idle_trigger_to_once_per_60_seconds() {
    let rig = make();
    rig.service.notify_agent_idle();
    rig.service.notify_agent_idle();
    assert_eq!((rig.scans(), rig.polls()), (1, 1));
    rig.advance(IDLE_THROTTLE_MS);
    rig.service.notify_agent_idle();
    assert_eq!((rig.scans(), rig.polls()), (2, 2));
}

fn profile(plan: &str) -> ClaudeProfileLimits {
    ClaudeProfileLimits {
        account: "default".into(),
        config_dir: "/c".into(),
        plan: Some(plan.into()),
        status: ProfileStatus::Ok,
        windows: vec![],
        fetched_at: Some(1),
        checked_at: 1,
    }
}

#[test]
fn emits_changed_only_when_something_visible_changed_and_bumps_data_version_with_data() {
    let rig = make();
    rig.service.refresh();
    assert_eq!(rig.changed.lock().len(), 1); // first state
    rig.service.refresh();
    assert_eq!(rig.changed.lock().len(), 1); // only updatedAt moved
    rig.store.changed_next.store(true, Ordering::SeqCst);
    rig.service.refresh();
    assert_eq!(rig.changed.lock().len(), 2);
    assert_eq!(rig.changed.lock()[1].data_version, 1);
    *rig.limits.data.lock() = vec![profile("Max")];
    rig.service.refresh();
    let changed = rig.changed.lock();
    assert_eq!(changed.len(), 3);
    assert_eq!(changed[2].claude[0].plan.as_deref(), Some("Max"));
}

#[test]
fn throttles_progress_emits_to_500ms_and_shows_indexing_only_on_the_first_index() {
    let rig = make();
    *rig.store.steps.lock() = vec![(0, 1), (0, 2), (PROGRESS_EMIT_MS, 3)];
    rig.service.refresh();
    let changed = rig.changed.lock().clone();
    let indexing: Vec<_> = changed.iter().filter(|s| s.indexing.indexing).collect();
    assert!(
        !indexing.is_empty() && indexing.len() <= 2,
        "{}",
        indexing.len()
    );
    assert!(!changed.last().unwrap().indexing.indexing);
    // a later incremental pass does not flash "indexing"
    rig.changed.lock().clear();
    *rig.store.steps.lock() = vec![(PROGRESS_EMIT_MS, 1)];
    rig.advance(PROGRESS_EMIT_MS);
    rig.service.refresh();
    assert!(!rig.changed.lock().iter().any(|s| s.indexing.indexing));
}

#[test]
fn coalesces_concurrent_refresh_calls() {
    let rig = make();
    rig.store.delay_ms.store(100, Ordering::SeqCst);
    let handles: Vec<_> = (0..3)
        .map(|i| {
            let s = rig.service.clone();
            thread::spawn(move || {
                thread::sleep(Duration::from_millis(i * 20));
                s.refresh()
            })
        })
        .collect();
    for h in handles {
        h.join().unwrap();
    }
    assert_eq!((rig.scans(), rig.polls()), (1, 1));
}

#[test]
fn passes_the_project_key_to_the_store_summary() {
    let rig = make();
    rig.service.summary(UsagePeriod::Week, Some("/p"));
    rig.service.summary(UsagePeriod::Today, None);
    let calls = rig.store.summaries.lock();
    assert_eq!(calls[0], (UsagePeriod::Week, Some("/p".to_string())));
    assert_eq!(calls[1], (UsagePeriod::Today, None));
}

#[test]
fn report_days_are_clamped() {
    let rig = make();
    assert_eq!(
        rig.service.report(1, UsageReportProvider::All).daily.len(),
        7
    );
    assert_eq!(
        rig.service
            .report(500, UsageReportProvider::All)
            .daily
            .len(),
        90
    );
}

// ── the state never carries the OAuth token ─────────────────────────────────

struct OkHttp;
impl HttpClient for OkHttp {
    fn get(&self, _r: &HttpRequest) -> Result<HttpResponse, HttpError> {
        Ok(HttpResponse {
            status: 200,
            retry_after: None,
            body: r#"{"five_hour":{"utilization":42,"resets_at":"2026-10-23T14:00:00Z"}}"#.into(),
        })
    }
}
struct TokenCreds;
impl CredentialReader for TokenCreds {
    fn read(&self, _d: &std::path::Path, _default: bool) -> CredentialResult {
        CredentialResult::Ok {
            token: Secret::new("sk-ant-oat01-SECRET-TOKEN-VALUE"),
            expires_at: Some(T0 + 3_600_000),
            plan: Some("Max".into()),
            identity: "id".into(),
        }
    }
}

#[test]
fn the_usage_state_never_contains_the_oauth_token() {
    let client = ClaudeLimitsClient::new(ClaudeLimitsClientOptions {
        profiles: Box::new(|| {
            vec![ClientProfile {
                config_dir: "/home/u/.claude".into(),
                label: "default".into(),
                is_default: true,
            }]
        }),
        http: Arc::new(OkHttp),
        credentials: Arc::new(TokenCreds),
        now: Box::new(|| T0),
        version: None,
    });
    let service = UsageService::new(UsageServiceOptions {
        store: Arc::new(FakeStore::default()),
        limits: Arc::new(client),
        now: Box::new(|| T0),
    });
    let state = service.refresh();
    assert!(!state.claude[0].windows.is_empty());
    let json = serde_json::to_string(&state).unwrap();
    assert!(!json.contains("SECRET-TOKEN") && !json.contains("sk-ant"));
}
