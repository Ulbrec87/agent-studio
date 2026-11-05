//! Usage over the socket: logs on disk, a fake HTTP layer and fake sign-ins; never the
//! developer's real `~/.claude*`, `~/.codex` or the network.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use as_core::agents::fake_pty::FakePtyBackend;
use as_core::paths::Paths;
use as_core::sessions::{ClaudeSessionCatalog, CodexSessionCatalog};
use as_core::usage::credentials::{CredentialResult, Secret};
use as_core::usage::limits_client::{
    ClientProfile, CredentialReader, HttpClient, HttpError, HttpRequest, HttpResponse,
};
use as_core::usage::report::UsageReportProvider;
use as_core::usage::store::UsageSource;
use as_core::usage::types::{ProfileStatus, UsagePeriod, UsageProvider};
use as_core::{ManagerOptions, UsageSettings};
use as_daemon::{DaemonOptions, SessionCatalogs, UsageOptions, serve};
use as_proto::{DaemonClient, Event, NotificationKind};
use serde_json::json;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio_util::sync::CancellationToken;

const TOKEN: &str = "sk-ant-oat01-SECRET-TOKEN-VALUE";

struct FakeHttp {
    calls: AtomicUsize,
    body: String,
}
impl HttpClient for FakeHttp {
    fn get(&self, request: &HttpRequest) -> Result<HttpResponse, HttpError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        assert_eq!(request.bearer.expose(), TOKEN);
        Ok(HttpResponse {
            status: 200,
            retry_after: None,
            body: self.body.clone(),
        })
    }
}

struct FakeCreds;
impl CredentialReader for FakeCreds {
    fn read(&self, _dir: &Path, _is_default: bool) -> CredentialResult {
        CredentialResult::Ok {
            token: Secret::new(TOKEN),
            expires_at: None,
            plan: Some("Max".into()),
            identity: "id".into(),
        }
    }
}

struct Rig {
    _dir: tempfile::TempDir,
    paths: Paths,
    http: Arc<FakeHttp>,
    stop: CancellationToken,
    task: tokio::task::JoinHandle<anyhow::Result<()>>,
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn claude_line(id: &str, ts: &str, input: u64, output: u64) -> String {
    format!(
        "{}\n",
        json!({"type": "assistant", "requestId": format!("r{id}"), "timestamp": ts, "cwd": "/w/shop",
            "message": {"id": format!("m{id}"), "model": "claude-sonnet-4-6",
                "usage": {"input_tokens": input, "output_tokens": output}}})
    )
}

fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// A Codex rollout whose newest `token_count` carries a 95 % session window, written just now.
fn codex_rollout(root: &Path) {
    let ts = now_iso();
    let resets = chrono::Utc::now().timestamp() + 3600;
    let text = format!(
        "{}\n{}\n{}\n",
        json!({"timestamp": ts, "type": "session_meta", "payload": {"id": "cx1", "cwd": "/w/docs"}}),
        json!({"timestamp": ts, "type": "turn_context", "payload": {"turn_id": "t", "model": "gpt-5-codex"}}),
        json!({"timestamp": ts, "type": "event_msg", "payload": {"type": "token_count",
            "info": {"last_token_usage": {"input_tokens": 1000, "output_tokens": 100},
                     "total_token_usage": {"input_tokens": 1000, "output_tokens": 100}},
            "rate_limits": {"plan_type": "plus", "primary": {"used_percent": 95, "window_minutes": 300, "resets_at": resets}}}})
    );
    write(&root.join("sessions/2026/10/22/rollout-a.jsonl"), &text);
}

fn enable_alerts(paths: &Paths) {
    let usage = UsageSettings {
        alerts_enabled: true,
        ..UsageSettings::default()
    };
    write(
        &paths.config_file(),
        &serde_json::to_string(&json!({"usage": usage, "importSessionsOnStartup": false})).unwrap(),
    );
}

async fn start(alerts: bool) -> (Rig, PathBuf, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let claude_projects = dir.path().join("claude/projects");
    let codex_home = dir.path().join("codex");
    write(
        &claude_projects.join("-w-shop/s1.jsonl"),
        &(claude_line("1", &now_iso(), 1000, 200) + &claude_line("2", &now_iso(), 3000, 400)),
    );
    codex_rollout(&codex_home);
    let paths = Paths::at(dir.path().join("data"));
    paths.ensure().unwrap();
    if alerts {
        enable_alerts(&paths);
    }
    let http = Arc::new(FakeHttp {
        calls: AtomicUsize::new(0),
        body: include_str!("../../as-core/src/usage/fixtures/claude-oauth-usage.json").to_string(),
    });
    let backend = FakePtyBackend::new();
    let options = DaemonOptions {
        manager: ManagerOptions {
            backend: Arc::new(backend),
            ..Default::default()
        },
        catalogs: SessionCatalogs {
            claude: Arc::new(ClaudeSessionCatalog::with_dirs(Vec::new(), 0)),
            codex: Arc::new(CodexSessionCatalog::with_dir(
                PathBuf::from("/nonexistent"),
                0,
            )),
        },
        catalog_locator: true,
        import_sessions: false,
        preflight_on_create: false,
        apply_fixed_path: false,
        os_notifications: false,
        overview: false,
        overview_config_dirs: Some(Vec::new()),
        usage: true,
        mcp: false,
        headless_override: None,
        usage_options: UsageOptions {
            sources: Some(vec![
                UsageSource {
                    provider: UsageProvider::Claude,
                    root: claude_projects.clone(),
                    max_depth: 4,
                },
                UsageSource {
                    provider: UsageProvider::Codex,
                    root: codex_home.join("sessions"),
                    max_depth: 5,
                },
            ]),
            profiles: Some(vec![ClientProfile {
                config_dir: dir.path().join("claude").to_string_lossy().into_owned(),
                label: "default".into(),
                is_default: true,
            }]),
            http: Some(http.clone()),
            credentials: Some(Arc::new(FakeCreds)),
        },
        skills: Some(as_core::skills::SkillScanOptions {
            claude_config_dirs: Vec::new(),
            home: std::env::temp_dir().join("as-test-no-home"),
        }),
        hydra_dir: None,
    };
    let stop = CancellationToken::new();
    let task = tokio::spawn(serve(paths.clone(), options, stop.clone()));
    for _ in 0..400 {
        if as_proto::transport::connect(&paths).await.is_ok() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    (
        Rig {
            _dir: dir,
            paths,
            http,
            stop,
            task,
        },
        claude_projects,
        codex_home,
    )
}

async fn next_event(
    rx: &mut UnboundedReceiver<Event>,
    what: &str,
    mut pick: impl FnMut(&Event) -> bool,
) -> Event {
    let wait = async {
        loop {
            match rx.recv().await {
                Some(e) if pick(&e) => return e,
                Some(_) => {}
                None => panic!("event stream closed while waiting for {what}"),
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(10), wait)
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for {what}"))
}

#[tokio::test]
async fn usage_state_summary_report_and_limits_over_the_socket() {
    let (rig, _, _) = start(false).await;
    let client = DaemonClient::connect(&rig.paths).await.unwrap();
    let mut events = client.take_events().unwrap();

    let state = client.refresh_usage().await.unwrap();
    // limits: one profile, from the fixture through the fake HTTP layer
    assert_eq!(state.claude.len(), 1);
    let p = &state.claude[0];
    assert_eq!(
        (p.account.as_str(), p.status, p.plan.as_deref()),
        ("default", ProfileStatus::Ok, Some("Max"))
    );
    let windows: Vec<_> = p
        .windows
        .iter()
        .map(|w| (w.id.as_str(), w.used_percent))
        .collect();
    assert_eq!(windows, [("session", 4.0), ("weekly", 0.0)]);
    // Codex limits come passively from the rollout
    let codex = state.codex.clone().expect("codex limits");
    assert_eq!(codex.windows[0].used_percent, 95.0);
    assert!(state.data_version > 0);
    assert!(!state.indexing.indexing);
    // the token never travels
    assert!(!serde_json::to_string(&state).unwrap().contains(TOKEN));
    assert!(rig.http.calls.load(Ordering::SeqCst) >= 1);

    // summary: 4 200 + 1 100 tokens today (Claude 1000+200+3000+400, Codex 1000+100)
    let today = client
        .get_usage_summary(UsagePeriod::Today, None)
        .await
        .unwrap();
    assert_eq!(today.by_provider[0].total_tokens, 4600);
    assert_eq!(today.by_provider[1].total_tokens, 1100);
    assert_eq!(today.total_tokens, 5700);
    assert!(today.cost_usd > 0.0);
    // project filter
    let shop = client
        .get_usage_summary(UsagePeriod::Today, Some("/w/shop".into()))
        .await
        .unwrap();
    assert_eq!(shop.total_tokens, 4600);
    // report
    let report = client
        .get_usage_report(UsageReportProvider::Claude, 7)
        .await
        .unwrap();
    assert_eq!(report.daily.len(), 7);
    assert_eq!(report.daily.last().unwrap().total_tokens, 4600);
    assert_eq!(report.daily.last().unwrap().projects[0].label, "shop");
    // bad range
    let err = client
        .get_usage_report(UsageReportProvider::All, 3)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("days must be"), "{err:#}");
    // the state is pushed to clients when it changes
    next_event(&mut events, "usage changed", |e| {
        matches!(e, Event::UsageChanged(_))
    })
    .await;
    // visibility requests are accepted
    client.set_usage_visible(true).await.unwrap();
    client.set_usage_visible(false).await.unwrap();
    assert_eq!(client.get_usage_state().await.unwrap().claude.len(), 1);

    rig.stop.cancel();
    rig.task.await.unwrap().unwrap();
    // the cache is flushed on shutdown, with no message text and no credential
    let cache = std::fs::read_to_string(rig.paths.usage_cache_file()).unwrap();
    assert!(cache.contains("claude-sonnet-4-6"));
    assert!(!cache.contains(TOKEN));
}

#[tokio::test]
async fn new_log_lines_are_picked_up_incrementally() {
    let (rig, claude_projects, _) = start(false).await;
    let client = DaemonClient::connect(&rig.paths).await.unwrap();
    let first = client.refresh_usage().await.unwrap();
    let file = claude_projects.join("-w-shop/s1.jsonl");
    let mut f = std::fs::OpenOptions::new()
        .append(true)
        .open(&file)
        .unwrap();
    use std::io::Write;
    f.write_all(claude_line("3", &now_iso(), 500, 50).as_bytes())
        .unwrap();
    drop(f);
    let second = client.refresh_usage().await.unwrap();
    assert!(second.data_version > first.data_version);
    let today = client
        .get_usage_summary(UsagePeriod::Today, None)
        .await
        .unwrap();
    assert_eq!(today.by_provider[0].total_tokens, 4600 + 550);
    rig.stop.cancel();
    rig.task.await.unwrap().unwrap();
}

#[tokio::test]
async fn a_window_near_its_limit_raises_one_usage_limit_notification() {
    // The daemon scans on startup; if alerts were already on, the notification could fire before
    // this client subscribes and be missed. Start with alerts off, subscribe, then enable them
    // (the hot-reloaded config re-evaluates the alerts).
    let (rig, _, _) = start(false).await;
    let client = DaemonClient::connect(&rig.paths).await.unwrap();
    let mut events = client.take_events().unwrap();
    client.refresh_usage().await.unwrap();
    enable_alerts(&rig.paths);
    let n = next_event(
        &mut events,
        "usage limit notification",
        |e| matches!(e, Event::Notification(n) if n.kind == NotificationKind::UsageLimit),
    )
    .await;
    let Event::Notification(n) = n else {
        unreachable!()
    };
    assert_eq!(n.title, "Codex session limit: 95% used");
    assert!(n.body.starts_with("Resets in "), "{}", n.body);
    // a second refresh does not repeat it
    client.refresh_usage().await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    while let Ok(e) = events.try_recv() {
        assert!(
            !matches!(&e, Event::Notification(n) if n.kind == NotificationKind::UsageLimit),
            "alert repeated"
        );
    }
    // remembered across restarts
    let remembered = std::fs::read_to_string(rig.paths.usage_alerts_file()).unwrap();
    assert!(remembered.contains("codex|default|session"), "{remembered}");
    rig.stop.cancel();
    rig.task.await.unwrap().unwrap();
}

#[tokio::test]
async fn alerts_stay_off_by_default() {
    let (rig, _, _) = start(false).await;
    let client = DaemonClient::connect(&rig.paths).await.unwrap();
    let mut events = client.take_events().unwrap();
    client.refresh_usage().await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    while let Ok(e) = events.try_recv() {
        assert!(!matches!(&e, Event::Notification(_)), "{e:?}");
    }
    assert!(!rig.paths.usage_alerts_file().exists());
    rig.stop.cancel();
    rig.task.await.unwrap().unwrap();
}
