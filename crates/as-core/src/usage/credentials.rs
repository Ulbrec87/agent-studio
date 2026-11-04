// Adapted from Tokdash (MIT, © JingbiaoMei): src/tokdash/sources/quota/claude.py
//! Read-only access to a Claude Code OAuth sign-in (Hydra `electron/usage/claudeCredentials.ts`).
//!
//! The token is never refreshed (refreshing rotates the refresh token and can sign Claude Code
//! out), never logged and never put in an error: it lives in a [`Secret`] whose `Debug` is
//! redacted, and nothing here formats a token or the credential file's text into a message. Only
//! the limits client consumes [`Secret::expose`].

use std::io::ErrorKind;
use std::path::Path;

use base64::Engine;
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::files;

/// A credential that cannot be printed by accident.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    pub fn new(s: impl Into<String>) -> Self {
        Secret(s.into())
    }
    /// The raw value; only for building the request header.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret(<redacted>)")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialFailure {
    NotSignedIn,
    Expired,
    Unreadable,
    KeychainDenied,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CredentialResult {
    Ok {
        token: Secret,
        expires_at: Option<i64>,
        plan: Option<String>,
        /// A non-secret key for the account (hash).
        identity: String,
    },
    Failed(CredentialFailure),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecurityResult {
    pub exit_code: i32,
    pub stdout: String,
}

/// Runs `security <args>`; `Err` on spawn failure or timeout.
pub type SecurityRunner = Box<dyn Fn(&[String]) -> Result<SecurityResult, ()> + Send + Sync>;

/// What the reader needs from the outside world. `Default` is the real one.
pub struct CredentialDeps {
    pub macos: bool,
    pub now: Box<dyn Fn() -> i64 + Send + Sync>,
    /// `CLAUDE_CODE_OAUTH_TOKEN`.
    pub env_token: Option<String>,
    pub run_security: SecurityRunner,
}

impl Default for CredentialDeps {
    fn default() -> Self {
        Self {
            macos: cfg!(target_os = "macos"),
            now: Box::new(|| chrono::Utc::now().timestamp_millis()),
            env_token: std::env::var("CLAUDE_CODE_OAUTH_TOKEN").ok(),
            run_security: Box::new(default_run_security),
        }
    }
}

const MAX_CREDENTIALS_BYTES: u64 = 1024 * 1024;
const KEYCHAIN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
const KEYCHAIN_SERVICE: &str = "Claude Code-credentials";
/// `security` exit code for "item could not be found".
const SEC_ITEM_NOT_FOUND: i32 = 44;
const IDENTITY_CLAIMS: [&str; 4] = ["account_id", "accountId", "sub", "email"];

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn sha256_hex(text: &str) -> String {
    hex(&Sha256::digest(text.as_bytes()))
}

/// Claude Code keys the Keychain item of a custom config dir by the first 8 hex chars of the
/// SHA-256 of its path.
pub fn claude_keychain_service(config_dir: &str, is_default: bool) -> String {
    if is_default {
        return KEYCHAIN_SERVICE.to_string();
    }
    format!("{KEYCHAIN_SERVICE}-{}", &sha256_hex(config_dir)[..8])
}

/// `security` with a hard timeout; stdout is never forwarded into an error.
fn default_run_security(args: &[String]) -> Result<SecurityResult, ()> {
    use std::process::{Command, Stdio};
    let mut child = Command::new("security")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| ())?;
    let deadline = std::time::Instant::now() + KEYCHAIN_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let mut out = String::new();
                if let Some(mut s) = child.stdout.take() {
                    use std::io::Read;
                    let mut bytes = Vec::new();
                    let _ = s
                        .by_ref()
                        .take(MAX_CREDENTIALS_BYTES)
                        .read_to_end(&mut bytes);
                    out = String::from_utf8_lossy(&bytes).into_owned();
                }
                return Ok(SecurityResult {
                    exit_code: status.code().unwrap_or(-1),
                    stdout: if status.success() { out } else { String::new() },
                });
            }
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(25));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(());
            }
        }
    }
}

/// A non-secret key for the account a token belongs to. Claude Code's access token is a JWT whose
/// payload names the account and survives refreshes; otherwise a hash of the opaque token. Always
/// a hash, so neither the token nor an e-mail address leaves this function.
pub fn claude_identity(token: &str) -> String {
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() == 3
        && let Ok(bytes) =
            base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(parts[1].trim_end_matches('='))
        && let Ok(Value::Object(payload)) = serde_json::from_slice::<Value>(&bytes)
    {
        for claim in IDENTITY_CLAIMS {
            if let Some(v) = payload
                .get(claim)
                .and_then(Value::as_str)
                .filter(|v| !v.is_empty())
            {
                return format!("id:{}", &sha256_hex(&format!("{claim}:{v}"))[..32]);
            }
        }
    }
    format!("tok:{}", &sha256_hex(token)[..32])
}

fn plan_label(
    subscription_type: Option<&Value>,
    rate_limit_tier: Option<&Value>,
) -> Option<String> {
    let base = subscription_type?.as_str()?.trim();
    let mut chars = base.chars();
    let first = chars.next()?;
    let name = first.to_uppercase().collect::<String>() + &chars.as_str().to_lowercase();
    // `/(\d+)x/i`
    let mult = rate_limit_tier.and_then(Value::as_str).and_then(|t| {
        let lower = t.to_lowercase();
        let bytes = lower.as_bytes();
        (0..bytes.len()).find_map(|i| {
            if bytes[i] != b'x' || i == 0 || !bytes[i - 1].is_ascii_digit() {
                return None;
            }
            let start = bytes[..i]
                .iter()
                .rposition(|b| !b.is_ascii_digit())
                .map_or(0, |p| p + 1);
            Some(lower[start..i].to_string())
        })
    });
    Some(match mult {
        Some(m) => format!("{name} {m}x"),
        None => name,
    })
}

enum Parsed {
    Result(CredentialResult),
    None,
}

fn from_blob(text: &str, now: i64) -> Parsed {
    let Ok(data) = serde_json::from_str::<Value>(text) else {
        return Parsed::Result(CredentialResult::Failed(CredentialFailure::Unreadable));
    };
    let Some(data) = data.as_object() else {
        return Parsed::Result(CredentialResult::Failed(CredentialFailure::Unreadable));
    };
    let Some(oauth) = data.get("claudeAiOauth").and_then(Value::as_object) else {
        return Parsed::None;
    };
    let token = oauth
        .get("accessToken")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("");
    if token.is_empty() {
        return Parsed::None;
    }
    let expires_at = oauth
        .get("expiresAt")
        .and_then(Value::as_f64)
        .filter(|n| n.is_finite())
        .map(|n| n as i64);
    if expires_at.is_some_and(|e| e <= now) {
        return Parsed::Result(CredentialResult::Failed(CredentialFailure::Expired));
    }
    Parsed::Result(CredentialResult::Ok {
        token: Secret::new(token),
        expires_at,
        plan: plan_label(oauth.get("subscriptionType"), oauth.get("rateLimitTier")),
        identity: claude_identity(token),
    })
}

/// The sign-in of one Claude profile: `<config_dir>/.credentials.json` first, then (macOS) the
/// Keychain item `Claude Code-credentials[-<hash of the dir>]` through the `security` CLI.
pub fn read_claude_credentials(
    config_dir: &Path,
    is_default: bool,
    deps: &CredentialDeps,
) -> CredentialResult {
    use CredentialFailure as F;
    let fail = |f| CredentialResult::Failed(f);
    let now = (deps.now)();

    if is_default
        && let Some(env) = deps
            .env_token
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty())
    {
        return CredentialResult::Ok {
            token: Secret::new(env),
            expires_at: None,
            plan: None,
            identity: claude_identity(env),
        };
    }

    let mut file_failure: Option<CredentialResult> = None;
    match files::read_text(&config_dir.join(".credentials.json"), MAX_CREDENTIALS_BYTES) {
        Ok(Some(text)) => {
            if let Parsed::Result(r) = from_blob(&text, now) {
                return r;
            }
        }
        Ok(None) => {}
        Err(e) if e.kind() == ErrorKind::NotFound => {}
        Err(_) => file_failure = Some(fail(F::Unreadable)),
    }
    if let Some(f) = &file_failure
        && !deps.macos
    {
        return f.clone();
    }

    if deps.macos {
        let service = claude_keychain_service(&config_dir.to_string_lossy(), is_default);
        let args = ["find-generic-password", "-s", service.as_str(), "-w"].map(String::from);
        let Ok(result) = (deps.run_security)(&args) else {
            return fail(F::KeychainDenied);
        };
        if result.exit_code == SEC_ITEM_NOT_FOUND {
            return file_failure.unwrap_or(fail(F::NotSignedIn));
        }
        if result.exit_code != 0 {
            return fail(F::KeychainDenied);
        }
        let text = result.stdout.trim();
        if text.is_empty() {
            return file_failure.unwrap_or(fail(F::NotSignedIn));
        }
        if let Parsed::Result(r) = from_blob(text, now) {
            return r;
        }
    }
    file_failure.unwrap_or(fail(F::NotSignedIn))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use parking_lot::Mutex;

    use super::*;

    const NOW: i64 = 1_792_746_000_000; // 2026-10-23T09:00:00Z
    const TOKEN: &str = "sk-ant-oat01-SECRET-TOKEN-VALUE";

    fn blob_with(over: serde_json::Value) -> String {
        let mut oauth = serde_json::json!({
            "accessToken": TOKEN,
            "expiresAt": NOW + 3_600_000,
            "subscriptionType": "max",
            "rateLimitTier": "default_claude_max_20x",
        });
        for (k, v) in over.as_object().unwrap() {
            oauth[k] = v.clone();
        }
        serde_json::json!({ "claudeAiOauth": oauth }).to_string()
    }
    fn blob() -> String {
        blob_with(serde_json::json!({}))
    }
    fn deps() -> CredentialDeps {
        CredentialDeps {
            macos: false,
            now: Box::new(|| NOW),
            env_token: None,
            run_security: Box::new(|_| panic!("security must not run")),
        }
    }
    fn failed(f: CredentialFailure) -> CredentialResult {
        CredentialResult::Failed(f)
    }
    fn ok(r: &CredentialResult) -> (&Secret, Option<i64>, Option<&str>, &str) {
        match r {
            CredentialResult::Ok {
                token,
                expires_at,
                plan,
                identity,
            } => (token, *expires_at, plan.as_deref(), identity),
            other => panic!("not ok: {other:?}"),
        }
    }
    fn write(dir: &Path, text: &str) {
        std::fs::write(dir.join(".credentials.json"), text).unwrap();
    }

    #[test]
    fn reads_a_valid_file() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), &blob());
        let r = read_claude_credentials(dir.path(), true, &deps());
        let (token, expires, plan, identity) = ok(&r);
        assert_eq!(
            (token.expose(), expires, plan),
            (TOKEN, Some(NOW + 3_600_000), Some("Max 20x"))
        );
        assert!(identity.starts_with("tok:") && identity.len() == 4 + 32);
        assert!(!identity.contains(TOKEN));
    }

    #[test]
    fn plan_is_clean_without_a_tier_multiplier() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            &blob_with(serde_json::json!({"subscriptionType": "pro", "rateLimitTier": "default"})),
        );
        let r = read_claude_credentials(dir.path(), true, &deps());
        assert_eq!(ok(&r).2, Some("Pro"));
    }

    #[test]
    fn reports_expired() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            &blob_with(serde_json::json!({"expiresAt": NOW - 1})),
        );
        assert_eq!(
            read_claude_credentials(dir.path(), true, &deps()),
            failed(CredentialFailure::Expired)
        );
    }

    #[test]
    fn not_signed_in_when_the_file_is_missing_or_has_no_oauth_block() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            read_claude_credentials(dir.path(), true, &deps()),
            failed(CredentialFailure::NotSignedIn)
        );
        write(dir.path(), "{}");
        assert_eq!(
            read_claude_credentials(dir.path(), true, &deps()),
            failed(CredentialFailure::NotSignedIn)
        );
    }

    #[test]
    fn unreadable_for_malformed_json_oversized_files_and_links() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "{nope");
        assert_eq!(
            read_claude_credentials(dir.path(), true, &deps()),
            failed(CredentialFailure::Unreadable)
        );
        write(dir.path(), &"x".repeat(1024 * 1024 + 1));
        assert_eq!(
            read_claude_credentials(dir.path(), true, &deps()),
            failed(CredentialFailure::Unreadable)
        );
        #[cfg(unix)]
        {
            let other = tempfile::tempdir().unwrap();
            std::fs::write(other.path().join("real.json"), blob()).unwrap();
            std::fs::create_dir(other.path().join("cfg")).unwrap();
            std::os::unix::fs::symlink(
                other.path().join("real.json"),
                other.path().join("cfg/.credentials.json"),
            )
            .unwrap();
            assert_eq!(
                read_claude_credentials(&other.path().join("cfg"), true, &deps()),
                failed(CredentialFailure::Unreadable)
            );
        }
    }

    #[test]
    fn uses_the_env_token_for_the_default_profile_only() {
        let dir = tempfile::tempdir().unwrap();
        let with_env = || CredentialDeps {
            env_token: Some(" env-token ".into()),
            ..deps()
        };
        let r = read_claude_credentials(dir.path(), true, &with_env());
        let (token, expires, plan, _) = ok(&r);
        assert_eq!((token.expose(), expires, plan), ("env-token", None, None));
        assert_eq!(
            read_claude_credentials(dir.path(), false, &with_env()),
            failed(CredentialFailure::NotSignedIn)
        );
    }

    // ── macOS Keychain ──────────────────────────────────────────────────────

    type Calls = Arc<Mutex<Vec<Vec<String>>>>;
    fn mac(result: Result<SecurityResult, ()>) -> (CredentialDeps, Calls) {
        let calls: Calls = Arc::new(Mutex::new(Vec::new()));
        let log = calls.clone();
        let d = CredentialDeps {
            macos: true,
            run_security: Box::new(move |args| {
                log.lock().push(args.to_vec());
                result.clone()
            }),
            ..deps()
        };
        (d, calls)
    }
    fn sec(exit_code: i32, stdout: &str) -> Result<SecurityResult, ()> {
        Ok(SecurityResult {
            exit_code,
            stdout: stdout.into(),
        })
    }

    #[test]
    fn reads_the_keychain_when_there_is_no_file() {
        let dir = tempfile::tempdir().unwrap();
        let (d, calls) = mac(sec(0, &(blob() + "\n")));
        let r = read_claude_credentials(dir.path(), true, &d);
        assert_eq!(ok(&r).0.expose(), TOKEN);
        assert_eq!(
            calls.lock()[0],
            [
                "find-generic-password",
                "-s",
                "Claude Code-credentials",
                "-w"
            ]
        );
    }

    #[test]
    fn prefers_the_file_over_the_keychain() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), &blob());
        let (d, calls) = mac(sec(0, ""));
        assert!(matches!(
            read_claude_credentials(dir.path(), true, &d),
            CredentialResult::Ok { .. }
        ));
        assert!(calls.lock().is_empty());
    }

    #[test]
    fn uses_a_hashed_service_name_for_a_non_default_config_dir() {
        let dir = tempfile::tempdir().unwrap();
        let (d, calls) = mac(sec(44, ""));
        assert_eq!(
            read_claude_credentials(dir.path(), false, &d),
            failed(CredentialFailure::NotSignedIn)
        );
        let service = calls.lock()[0][2].clone();
        let suffix = service.strip_prefix("Claude Code-credentials-").unwrap();
        assert!(suffix.len() == 8 && suffix.chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(
            service,
            claude_keychain_service(&dir.path().to_string_lossy(), false)
        );
        assert_ne!(
            claude_keychain_service("/a", false),
            claude_keychain_service("/b", false)
        );
    }

    #[test]
    fn a_denial_other_non_zero_exits_and_a_spawn_failure_are_keychain_denied() {
        let dir = tempfile::tempdir().unwrap();
        for r in [sec(36, ""), sec(128, ""), Err(())] {
            let (d, _) = mac(r);
            assert_eq!(
                read_claude_credentials(dir.path(), true, &d),
                failed(CredentialFailure::KeychainDenied)
            );
        }
    }

    #[test]
    fn expired_keychain_blob_and_garbage() {
        let dir = tempfile::tempdir().unwrap();
        let (d, _) = mac(sec(
            0,
            &blob_with(serde_json::json!({"expiresAt": NOW - 5})),
        ));
        assert_eq!(
            read_claude_credentials(dir.path(), true, &d),
            failed(CredentialFailure::Expired)
        );
        let (d, _) = mac(sec(0, "not json"));
        assert_eq!(
            read_claude_credentials(dir.path(), true, &d),
            failed(CredentialFailure::Unreadable)
        );
    }

    // ── identity and protection ─────────────────────────────────────────────

    fn jwt(payload: serde_json::Value) -> String {
        format!(
            "h.{}.sig",
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(payload.to_string())
        )
    }

    #[test]
    fn identity_is_stable_across_refreshed_tokens_and_hashes_the_claim() {
        let a = claude_identity(&jwt(serde_json::json!({"sub": "user-1", "iat": 1})));
        let b = claude_identity(&jwt(serde_json::json!({"sub": "user-1", "iat": 2})));
        assert_eq!(a, b);
        assert!(!a.contains("user-1"));
        assert_ne!(
            claude_identity(&jwt(serde_json::json!({"sub": "user-2"}))),
            a
        );
    }

    #[test]
    fn identity_falls_back_to_a_token_hash_for_opaque_tokens() {
        assert!(claude_identity("opaque").starts_with("tok:"));
        assert_eq!(claude_identity("opaque"), claude_identity("opaque"));
        assert!(!claude_identity("opaque").contains("opaque"));
    }

    #[test]
    fn a_secret_never_prints() {
        let s = Secret::new(TOKEN);
        assert!(!format!("{s:?}").contains(TOKEN));
        let r = CredentialResult::Ok {
            token: s,
            expires_at: None,
            plan: None,
            identity: "x".into(),
        };
        assert!(!format!("{r:?}").contains(TOKEN));
    }
}
