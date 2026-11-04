//! Plan limits of every Claude profile through `GET https://api.anthropic.com/api/oauth/usage`
//! (Hydra `electron/usage/ClaudeLimitsClient.ts`). Read-only: the OAuth token is never refreshed,
//! and it never leaves this module (not in state, errors or logs).
//!
//! * One request per profile per poll, 15 s timeout; 429/5xx/network errors back off
//!   60 s -> 2 min -> 5 min -> 10 min (a larger `Retry-After` wins), a success resets the ladder.
//! * 401/403 means the sign-in expired: the last good values are kept and the rejected token is
//!   not sent again until Claude Code writes a new one.
//! * Two profiles signed in to the same account (same identity hash) are shown once, the default
//!   profile winning.
//! * A denied macOS Keychain is not asked again for 24 h.
//! * The HTTP call goes through [`HttpClient`] so tests never touch the network.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::credentials::{
    CredentialDeps, CredentialFailure, CredentialResult, Secret, read_claude_credentials,
};
use super::limits::parse_claude_oauth_usage;
use super::types::{ClaudeProfileLimits, LimitWindow, ProfileStatus};

pub const CLAUDE_USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const BACKOFF_STEPS_MS: [i64; 4] = [60_000, 120_000, 300_000, 600_000];
const KEYCHAIN_DENIED_MS: i64 = 24 * 60 * 60 * 1000;

// ── HTTP seam ───────────────────────────────────────────────────────────────

/// One GET. The bearer token is a [`Secret`], so `Debug` of a request never shows it.
#[derive(Debug, Clone)]
pub struct HttpRequest {
    pub url: String,
    pub bearer: Secret,
    /// The other headers, in order.
    pub headers: Vec<(String, String)>,
    pub timeout: Duration,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpResponse {
    pub status: u16,
    /// The `Retry-After` header, as sent.
    pub retry_after: Option<String>,
    pub body: String,
}

/// Any transport failure (timeout, DNS, TLS). Carries no text: it could echo request data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HttpError;

pub trait HttpClient: Send + Sync {
    fn get(&self, request: &HttpRequest) -> Result<HttpResponse, HttpError>;
}

/// The real client: `reqwest` (blocking, rustls). Built on first use so that constructing the
/// service costs nothing; call it from a blocking thread, never from async code.
#[derive(Default)]
pub struct ReqwestHttp {
    client: std::sync::OnceLock<Option<reqwest::blocking::Client>>,
}

impl ReqwestHttp {
    pub fn new() -> Self {
        Self::default()
    }
}

impl HttpClient for ReqwestHttp {
    fn get(&self, request: &HttpRequest) -> Result<HttpResponse, HttpError> {
        let client = self
            .client
            .get_or_init(|| {
                reqwest::blocking::Client::builder()
                    .timeout(request.timeout)
                    .build()
                    .ok()
            })
            .as_ref()
            .ok_or(HttpError)?;
        let mut builder = client
            .get(&request.url)
            .bearer_auth(request.bearer.expose())
            .timeout(request.timeout);
        for (k, v) in &request.headers {
            builder = builder.header(k.as_str(), v.as_str());
        }
        let response = builder.send().map_err(|_| HttpError)?;
        let status = response.status().as_u16();
        let retry_after = response
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        // Cap the body: this is a small JSON document.
        let body = {
            use std::io::Read;
            let mut bytes = Vec::new();
            response
                .take(4 * 1024 * 1024)
                .read_to_end(&mut bytes)
                .map_err(|_| HttpError)?;
            String::from_utf8_lossy(&bytes).into_owned()
        };
        Ok(HttpResponse {
            status,
            retry_after,
            body,
        })
    }
}

// ── credentials seam ────────────────────────────────────────────────────────

pub trait CredentialReader: Send + Sync {
    fn read(&self, config_dir: &Path, is_default: bool) -> CredentialResult;
}

/// File + (macOS) Keychain, see [`read_claude_credentials`].
pub struct SystemCredentialReader;

impl CredentialReader for SystemCredentialReader {
    fn read(&self, config_dir: &Path, is_default: bool) -> CredentialResult {
        read_claude_credentials(config_dir, is_default, &CredentialDeps::default())
    }
}

// ── client ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientProfile {
    pub config_dir: String,
    pub label: String,
    pub is_default: bool,
}

pub struct ClaudeLimitsClientOptions {
    pub profiles: Box<dyn Fn() -> Vec<ClientProfile> + Send + Sync>,
    pub http: Arc<dyn HttpClient>,
    pub credentials: Arc<dyn CredentialReader>,
    pub now: Box<dyn Fn() -> i64 + Send + Sync>,
    /// App version for the User-Agent.
    pub version: Option<String>,
}

struct ProfileState {
    account: String,
    config_dir: String,
    plan: Option<String>,
    status: ProfileStatus,
    windows: Vec<LimitWindow>,
    fetched_at: Option<i64>,
    checked_at: i64,
    identity: Option<String>,
    failures: usize,
    backoff_until: i64,
    keychain_denied_until: i64,
    /// Hash of the token the API last rejected; it is not sent again until Claude Code writes a
    /// new one.
    rejected_token: Option<String>,
}

#[derive(Default)]
struct Inner {
    states: HashMap<String, ProfileState>,
    visible: Vec<String>,
}

struct Ready {
    profile: ClientProfile,
    token: Secret,
}

enum Outcome {
    Expired {
        fingerprint: String,
    },
    Failed {
        rate_limited: bool,
        retry_after_ms: i64,
    },
    Unavailable,
    Ok(Vec<LimitWindow>),
}

pub struct ClaudeLimitsClient {
    opts: ClaudeLimitsClientOptions,
    user_agent: String,
    inner: Mutex<Inner>,
    refresh_lock: Mutex<()>,
}

fn fingerprint(token: &Secret) -> String {
    let digest = Sha256::digest(token.expose().as_bytes());
    digest.iter().take(16).map(|b| format!("{b:02x}")).collect()
}

impl ClaudeLimitsClient {
    pub fn new(opts: ClaudeLimitsClientOptions) -> Self {
        let user_agent = match &opts.version {
            Some(v) if !v.is_empty() => format!("agent-studio/{v}"),
            _ => "agent-studio".to_string(),
        };
        Self {
            opts,
            user_agent,
            inner: Mutex::new(Inner::default()),
            refresh_lock: Mutex::new(()),
        }
    }

    fn now(&self) -> i64 {
        (self.opts.now)()
    }

    /// Polls every profile; calls made meanwhile wait for that poll and return its result.
    pub fn refresh(&self) -> Vec<ClaudeProfileLimits> {
        match self.refresh_lock.try_lock() {
            Some(_guard) => self.run(),
            None => {
                let _wait = self.refresh_lock.lock();
            }
        }
        self.current()
    }

    pub fn current(&self) -> Vec<ClaudeProfileLimits> {
        let inner = self.inner.lock();
        inner
            .visible
            .iter()
            .filter_map(|dir| inner.states.get(dir))
            .map(snapshot)
            .collect()
    }

    /// When the next request may go out (now when none is backing off).
    pub fn next_allowed_at(&self) -> i64 {
        let now = self.now();
        let inner = self.inner.lock();
        let mut earliest = i64::MAX;
        for dir in &inner.visible {
            let Some(s) = inner.states.get(dir) else {
                return now;
            };
            let denied = if s.status == ProfileStatus::KeychainDenied {
                s.keychain_denied_until
            } else {
                0
            };
            let until = s.backoff_until.max(denied);
            if until <= now {
                return now;
            }
            earliest = earliest.min(until);
        }
        if earliest == i64::MAX { now } else { earliest }
    }

    fn state_for<'a>(inner: &'a mut Inner, p: &ClientProfile) -> &'a mut ProfileState {
        let s = inner
            .states
            .entry(p.config_dir.clone())
            .or_insert_with(|| ProfileState {
                account: p.label.clone(),
                config_dir: p.config_dir.clone(),
                plan: None,
                status: ProfileStatus::NotSignedIn,
                windows: Vec::new(),
                fetched_at: None,
                checked_at: 0,
                identity: None,
                failures: 0,
                backoff_until: 0,
                keychain_denied_until: 0,
                rejected_token: None,
            });
        s.account = p.label.clone();
        s
    }

    fn run(&self) {
        let profiles = (self.opts.profiles)();
        let now = self.now();

        // Phase 1: credentials of every profile that is not backing off.
        let prepared: Vec<Option<Ready>> = profiles.iter().map(|p| self.prepare(p, now)).collect();

        // De-duplicate profiles signed into one account: the default wins, else the first.
        let kept: Vec<ClientProfile> = {
            let mut inner = self.inner.lock();
            let identities: Vec<Option<String>> = profiles
                .iter()
                .map(|p| Self::state_for(&mut inner, p).identity.clone())
                .collect();
            let mut winners: HashMap<&String, &ClientProfile> = HashMap::new();
            for (p, id) in profiles.iter().zip(&identities) {
                let Some(id) = id else { continue };
                match winners.get(id) {
                    Some(held) if !(p.is_default && !held.is_default) => {}
                    _ => {
                        winners.insert(id, p);
                    }
                }
            }
            let kept: Vec<ClientProfile> = profiles
                .iter()
                .zip(&identities)
                .filter(|(p, id)| match id {
                    None => true,
                    Some(id) => winners
                        .get(id)
                        .is_some_and(|w| w.config_dir == p.config_dir),
                })
                .map(|(p, _)| p.clone())
                .collect();
            inner.visible = kept.iter().map(|p| p.config_dir.clone()).collect();
            kept
        };

        // Phase 2: one request per remaining profile, in parallel.
        let ready: Vec<Ready> = prepared
            .into_iter()
            .flatten()
            .filter(|r| kept.iter().any(|k| k.config_dir == r.profile.config_dir))
            .collect();
        let outcomes: Vec<(String, Outcome)> = std::thread::scope(|scope| {
            let handles: Vec<_> = ready
                .iter()
                .map(|r| scope.spawn(move || (r.profile.config_dir.clone(), self.fetch_one(r))))
                .collect();
            handles.into_iter().filter_map(|h| h.join().ok()).collect()
        });
        for (dir, outcome) in outcomes {
            self.apply(&dir, outcome);
        }
    }

    fn prepare(&self, p: &ClientProfile, now: i64) -> Option<Ready> {
        {
            let mut inner = self.inner.lock();
            let s = Self::state_for(&mut inner, p);
            if now < s.backoff_until {
                return None;
            }
            if now < s.keychain_denied_until {
                s.status = ProfileStatus::KeychainDenied;
                return None;
            }
            s.checked_at = now;
        }
        let cred = self
            .opts
            .credentials
            .read(Path::new(&p.config_dir), p.is_default);
        let mut inner = self.inner.lock();
        let s = Self::state_for(&mut inner, p);
        match cred {
            CredentialResult::Failed(reason) => {
                s.status = match reason {
                    CredentialFailure::NotSignedIn => ProfileStatus::NotSignedIn,
                    CredentialFailure::Expired => ProfileStatus::Expired,
                    CredentialFailure::Unreadable => ProfileStatus::Error,
                    CredentialFailure::KeychainDenied => {
                        s.keychain_denied_until = now + KEYCHAIN_DENIED_MS;
                        ProfileStatus::KeychainDenied
                    }
                };
                None
            }
            CredentialResult::Ok {
                token,
                plan,
                identity,
                ..
            } => {
                s.identity = Some(identity);
                s.plan = plan;
                if s.rejected_token.as_deref() == Some(fingerprint(&token).as_str()) {
                    s.status = ProfileStatus::Expired;
                    return None;
                }
                Some(Ready {
                    profile: p.clone(),
                    token,
                })
            }
        }
    }

    fn fetch_one(&self, ready: &Ready) -> Outcome {
        {
            let mut inner = self.inner.lock();
            if let Some(s) = inner.states.get_mut(&ready.profile.config_dir) {
                s.checked_at = self.now();
            }
        }
        let request = HttpRequest {
            url: CLAUDE_USAGE_URL.to_string(),
            bearer: ready.token.clone(),
            headers: vec![
                ("anthropic-beta".into(), "oauth-2025-04-20".into()),
                ("Accept".into(), "application/json".into()),
                ("User-Agent".into(), self.user_agent.clone()),
            ],
            timeout: REQUEST_TIMEOUT,
        };
        let failed = |rate_limited, retry_after_ms| Outcome::Failed {
            rate_limited,
            retry_after_ms,
        };
        let Ok(res) = self.opts.http.get(&request) else {
            // Network error or timeout. Nothing from the error is kept: it could echo request data.
            return failed(false, 0);
        };
        if res.status == 401 || res.status == 403 {
            return Outcome::Expired {
                fingerprint: fingerprint(&ready.token),
            };
        }
        if res.status == 429 || res.status >= 500 || !(200..300).contains(&res.status) {
            let retry_ms = res
                .retry_after
                .as_deref()
                .and_then(|v| v.trim().parse::<f64>().ok())
                .filter(|n| n.is_finite() && *n > 0.0)
                .map_or(0, |n| (n * 1000.0) as i64);
            return failed(res.status == 429, retry_ms);
        }
        let Ok(body) = serde_json::from_str::<Value>(&res.body) else {
            return failed(false, 0);
        };
        let account = self
            .inner
            .lock()
            .states
            .get(&ready.profile.config_dir)
            .map_or_else(|| ready.profile.label.clone(), |s| s.account.clone());
        let windows = parse_claude_oauth_usage(&body, &account);
        if windows.is_empty() {
            Outcome::Unavailable
        } else {
            Outcome::Ok(windows)
        }
    }

    fn apply(&self, dir: &str, outcome: Outcome) {
        let now = self.now();
        let mut inner = self.inner.lock();
        let Some(s) = inner.states.get_mut(dir) else {
            return;
        };
        let fail = |s: &mut ProfileState, status, retry_after_ms: i64| {
            let step = BACKOFF_STEPS_MS[s.failures.min(BACKOFF_STEPS_MS.len() - 1)];
            s.failures += 1;
            s.status = status;
            s.backoff_until = now + step.max(retry_after_ms);
        };
        match outcome {
            Outcome::Expired { fingerprint } => {
                s.status = ProfileStatus::Expired;
                s.rejected_token = Some(fingerprint);
            }
            Outcome::Failed {
                rate_limited,
                retry_after_ms,
            } => {
                let status = if rate_limited {
                    ProfileStatus::RateLimited
                } else {
                    ProfileStatus::Error
                };
                fail(s, status, retry_after_ms);
            }
            Outcome::Unavailable => {
                s.failures = 0;
                s.backoff_until = 0;
                s.status = ProfileStatus::Unavailable;
            }
            Outcome::Ok(windows) => {
                s.failures = 0;
                s.backoff_until = 0;
                s.rejected_token = None;
                s.windows = windows;
                s.fetched_at = Some(now);
                s.status = ProfileStatus::Ok;
            }
        }
    }
}

fn snapshot(s: &ProfileState) -> ClaudeProfileLimits {
    ClaudeProfileLimits {
        account: s.account.clone(),
        config_dir: s.config_dir.clone(),
        plan: s.plan.clone(),
        status: s.status,
        windows: s.windows.clone(),
        fetched_at: s.fetched_at,
        checked_at: s.checked_at,
    }
}

#[cfg(test)]
mod tests;
