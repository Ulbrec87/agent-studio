use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread;

use super::*;

const BODY: &str = include_str!("../fixtures/claude-oauth-usage.json");
const TOKEN: &str = "sk-ant-oat01-SECRET-TOKEN-VALUE";
const T0: i64 = 1_000_000;

type Responder = Box<dyn Fn(&HttpRequest) -> Result<HttpResponse, HttpError> + Send + Sync>;

struct FakeHttp {
    calls: Mutex<Vec<HttpRequest>>,
    queue: Mutex<VecDeque<Responder>>,
    default: Mutex<Responder>,
    /// While set, `get` blocks (after recording the call), so a test controls when a request finishes.
    hold: AtomicBool,
}

impl FakeHttp {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            calls: Mutex::new(Vec::new()),
            queue: Mutex::new(VecDeque::new()),
            default: Mutex::new(Box::new(|_| Ok(resp(200, BODY)))),
            hold: AtomicBool::new(false),
        })
    }
    fn set_default(
        &self,
        r: impl Fn(&HttpRequest) -> Result<HttpResponse, HttpError> + Send + Sync + 'static,
    ) {
        *self.default.lock() = Box::new(r);
    }
    fn once(
        &self,
        r: impl Fn(&HttpRequest) -> Result<HttpResponse, HttpError> + Send + Sync + 'static,
    ) {
        self.queue.lock().push_back(Box::new(r));
    }
    fn count(&self) -> usize {
        self.calls.lock().len()
    }
}

impl HttpClient for FakeHttp {
    fn get(&self, request: &HttpRequest) -> Result<HttpResponse, HttpError> {
        self.calls.lock().push(request.clone());
        while self.hold.load(Ordering::SeqCst) {
            thread::sleep(Duration::from_millis(1));
        }
        let next = self.queue.lock().pop_front();
        match next {
            Some(r) => r(request),
            None => (self.default.lock())(request),
        }
    }
}

fn resp(status: u16, body: &str) -> HttpResponse {
    HttpResponse {
        status,
        retry_after: None,
        body: body.to_string(),
    }
}

struct FakeCreds {
    by_dir: Mutex<HashMap<String, CredentialResult>>,
    reads: AtomicUsize,
}
impl FakeCreds {
    fn ok(token: &str, identity: &str) -> CredentialResult {
        CredentialResult::Ok {
            token: Secret::new(token),
            expires_at: None,
            plan: Some("Pro".into()),
            identity: identity.into(),
        }
    }
}
impl CredentialReader for FakeCreds {
    fn read(&self, dir: &Path, _is_default: bool) -> CredentialResult {
        self.reads.fetch_add(1, Ordering::SeqCst);
        let dir = dir.to_string_lossy().into_owned();
        self.by_dir
            .lock()
            .get(&dir)
            .cloned()
            .unwrap_or_else(|| FakeCreds::ok(TOKEN, &format!("id-{dir}")))
    }
}

struct Rig {
    client: Arc<ClaudeLimitsClient>,
    http: Arc<FakeHttp>,
    creds: Arc<FakeCreds>,
    t: Arc<Mutex<i64>>,
}
impl Rig {
    fn advance(&self, ms: i64) {
        *self.t.lock() += ms;
    }
    fn now(&self) -> i64 {
        *self.t.lock()
    }
}

fn default_profiles() -> Vec<ClientProfile> {
    vec![ClientProfile {
        config_dir: "/h/.claude".into(),
        label: "default".into(),
        is_default: true,
    }]
}

fn setup(profiles: Vec<ClientProfile>, creds: HashMap<String, CredentialResult>) -> Rig {
    let http = FakeHttp::new();
    let creds = Arc::new(FakeCreds {
        by_dir: Mutex::new(creds),
        reads: AtomicUsize::new(0),
    });
    let t = Arc::new(Mutex::new(T0));
    let clock = t.clone();
    let client = Arc::new(ClaudeLimitsClient::new(ClaudeLimitsClientOptions {
        profiles: Box::new(move || profiles.clone()),
        http: http.clone(),
        credentials: creds.clone(),
        now: Box::new(move || *clock.lock()),
        version: Some("1.2.3".into()),
    }));
    Rig {
        client,
        http,
        creds,
        t,
    }
}
fn simple() -> Rig {
    setup(default_profiles(), HashMap::new())
}

fn no_token(v: &impl std::fmt::Debug) {
    assert!(!format!("{v:?}").contains(TOKEN));
}

#[test]
fn fetches_parses_and_sends_the_expected_request() {
    let rig = simple();
    let p = rig.client.refresh().remove(0);
    assert_eq!(
        (
            p.account.as_str(),
            p.plan.as_deref(),
            p.status,
            p.fetched_at
        ),
        ("default", Some("Pro"), ProfileStatus::Ok, Some(T0))
    );
    let w: Vec<_> = p
        .windows
        .iter()
        .map(|w| (w.id.as_str(), w.used_percent))
        .collect();
    assert_eq!(w, [("session", 4.0), ("weekly", 0.0)]);
    let calls = rig.http.calls.lock();
    let req = &calls[0];
    assert_eq!(req.url, "https://api.anthropic.com/api/oauth/usage");
    assert_eq!(req.bearer.expose(), TOKEN);
    assert_eq!(
        req.headers,
        [
            ("anthropic-beta".to_string(), "oauth-2025-04-20".to_string()),
            ("Accept".to_string(), "application/json".to_string()),
            ("User-Agent".to_string(), "agent-studio/1.2.3".to_string()),
        ]
    );
    no_token(&rig.client.current());
    no_token(&p);
    no_token(req);
}

#[test]
fn keeps_last_good_windows_on_401_and_reports_expired() {
    let rig = simple();
    rig.http.once(|_| Ok(resp(200, BODY)));
    rig.http.once(|_| Ok(resp(401, r#"{"error":"x"}"#)));
    rig.client.refresh();
    rig.advance(1000);
    let p = rig.client.refresh().remove(0);
    assert_eq!(p.status, ProfileStatus::Expired);
    assert_eq!(p.windows.len(), 2);
    assert_eq!(p.fetched_at, Some(T0));
    assert_eq!(p.checked_at, T0 + 1000);
    no_token(&p);
}

#[test]
fn does_not_resend_a_rejected_token_until_a_new_one_is_written() {
    let rig = simple();
    rig.http.once(|_| Ok(resp(401, "{}")));
    rig.client.refresh();
    rig.advance(60_000);
    let p = rig.client.refresh().remove(0);
    assert_eq!(p.status, ProfileStatus::Expired);
    assert_eq!(rig.http.count(), 1);

    rig.creds.by_dir.lock().insert(
        "/h/.claude".into(),
        FakeCreds::ok(&format!("{TOKEN}-new"), "id-/h/.claude"),
    );
    rig.advance(60_000);
    let q = rig.client.refresh().remove(0);
    assert_eq!(q.status, ProfileStatus::Ok);
    assert_eq!(rig.http.count(), 2);
}

#[test]
fn backs_off_60s_2m_5m_10m_on_429_and_resets_on_success() {
    let rig = simple();
    rig.http.set_default(|_| Ok(resp(429, "{}")));
    let p = rig.client.refresh().remove(0);
    assert_eq!(p.status, ProfileStatus::RateLimited);
    assert_eq!(rig.client.next_allowed_at(), rig.now() + 60_000);

    rig.advance(30_000);
    rig.client.refresh();
    assert_eq!(rig.http.count(), 1); // during back-off: no request

    for _ in 0..5 {
        assert!(rig.client.next_allowed_at() > rig.now());
        rig.advance(rig.client.next_allowed_at() - rig.now());
        let before = rig.http.count();
        rig.client.refresh();
        assert_eq!(rig.http.count(), before + 1);
    }
    assert_eq!(rig.client.next_allowed_at() - rig.now(), 600_000);

    rig.advance(600_000);
    rig.http.set_default(|_| Ok(resp(200, BODY)));
    let p = rig.client.refresh().remove(0);
    assert_eq!(p.status, ProfileStatus::Ok);
    assert_eq!(rig.client.next_allowed_at(), rig.now());
    rig.http.set_default(|_| Ok(resp(500, "{}")));
    rig.client.refresh();
    assert_eq!(rig.client.next_allowed_at() - rig.now(), 60_000); // ladder restarted
    assert_eq!(rig.client.current()[0].status, ProfileStatus::Error);
}

#[test]
fn a_larger_retry_after_wins() {
    let rig = simple();
    rig.http.set_default(|_| {
        Ok(HttpResponse {
            status: 429,
            retry_after: Some("900".into()),
            body: "{}".into(),
        })
    });
    rig.client.refresh();
    assert_eq!(rig.client.next_allowed_at(), rig.now() + 900_000);
}

#[test]
fn network_errors_and_bad_json_are_errors_with_back_off() {
    let rig = simple();
    rig.http.once(|_| Err(HttpError));
    let p = rig.client.refresh().remove(0);
    assert_eq!(p.status, ProfileStatus::Error);
    assert_eq!(rig.client.next_allowed_at(), rig.now() + 60_000);
    no_token(&p);
    let s = simple();
    s.http.set_default(|_| Ok(resp(200, "not json")));
    assert_eq!(s.client.refresh()[0].status, ProfileStatus::Error);
}

#[test]
fn unavailable_when_the_body_has_no_windows() {
    let rig = simple();
    rig.http.set_default(|_| Ok(resp(200, "{}")));
    let p = rig.client.refresh().remove(0);
    assert_eq!(p.status, ProfileStatus::Unavailable);
    assert!(p.windows.is_empty());
}

#[test]
fn coalesces_concurrent_refreshes() {
    let rig = simple();
    // Deterministic order: the first refresh is held inside its request (so it owns the refresh lock) until the second
    // one has had ample time to find the lock taken. A fixed sleep race made this flaky on slow macOS runners.
    rig.http.hold.store(true, Ordering::SeqCst);
    let (a, b) = {
        let (c1, c2) = (rig.client.clone(), rig.client.clone());
        let a = thread::spawn(move || c1.refresh());
        let started = std::time::Instant::now();
        while rig.http.count() == 0 {
            assert!(
                started.elapsed() < Duration::from_secs(10),
                "the first refresh never sent its request"
            );
            thread::sleep(Duration::from_millis(1));
        }
        let b = thread::spawn(move || c2.refresh());
        thread::sleep(Duration::from_millis(300));
        rig.http.hold.store(false, Ordering::SeqCst);
        (a.join().unwrap(), b.join().unwrap())
    };
    assert_eq!(rig.http.count(), 1);
    assert_eq!(a, b);
}

#[test]
fn requests_every_profile_and_drops_a_duplicate_account_default_wins() {
    let profiles = vec![
        ClientProfile {
            config_dir: "/h/.claude-work".into(),
            label: "work".into(),
            is_default: false,
        },
        ClientProfile {
            config_dir: "/h/.claude".into(),
            label: "default".into(),
            is_default: true,
        },
        ClientProfile {
            config_dir: "/h/.claude-other".into(),
            label: "other".into(),
            is_default: false,
        },
    ];
    let creds = HashMap::from([
        ("/h/.claude-work".to_string(), FakeCreds::ok(TOKEN, "same")),
        ("/h/.claude".to_string(), FakeCreds::ok(TOKEN, "same")),
        (
            "/h/.claude-other".to_string(),
            FakeCreds::ok(TOKEN, "different"),
        ),
    ]);
    let rig = setup(profiles, creds);
    let r = rig.client.refresh();
    let names: Vec<_> = r.iter().map(|p| p.account.as_str()).collect();
    assert_eq!(names, ["default", "other"]);
    assert_eq!(rig.http.count(), 2);
    let cur: Vec<_> = rig
        .client
        .current()
        .iter()
        .map(|p| p.account.clone())
        .collect();
    assert_eq!(cur, ["default", "other"]);
}

#[test]
fn reports_credential_problems_without_a_request() {
    let creds = HashMap::from([(
        "/h/.claude".to_string(),
        CredentialResult::Failed(CredentialFailure::NotSignedIn),
    )]);
    let rig = setup(default_profiles(), creds);
    let p = rig.client.refresh().remove(0);
    assert_eq!(p.status, ProfileStatus::NotSignedIn);
    assert_eq!(rig.http.count(), 0);
}

#[test]
fn does_not_retry_a_denied_keychain_for_24_hours() {
    let creds = HashMap::from([(
        "/h/.claude".to_string(),
        CredentialResult::Failed(CredentialFailure::KeychainDenied),
    )]);
    let rig = setup(default_profiles(), creds);
    rig.client.refresh();
    assert_eq!(rig.creds.reads.load(Ordering::SeqCst), 1);
    rig.advance(23 * 3_600_000);
    assert_eq!(
        rig.client.refresh()[0].status,
        ProfileStatus::KeychainDenied
    );
    assert_eq!(rig.creds.reads.load(Ordering::SeqCst), 1);
    rig.advance(2 * 3_600_000);
    rig.client.refresh();
    assert_eq!(rig.creds.reads.load(Ordering::SeqCst), 2);
}

#[test]
fn never_exposes_the_token_in_any_result() {
    let rig = simple();
    let mut seen = Vec::new();
    for status in [200u16, 401, 429, 503] {
        rig.http
            .set_default(move |_| Ok(resp(status, if status == 200 { BODY } else { "{}" })));
        seen.push(format!("{:?}", rig.client.refresh()));
        seen.push(format!("{:?}", rig.client.current()));
        rig.advance(1_000_000);
    }
    assert!(!seen.concat().contains(TOKEN));
    // the serialized form is what travels to clients
    let json = serde_json::to_string(&rig.client.current()).unwrap();
    assert!(!json.contains(TOKEN) && !json.contains("SECRET"));
}
