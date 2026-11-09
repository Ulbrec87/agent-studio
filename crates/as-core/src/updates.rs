//! Update check (Hydra `electron/updates/UpdateService.ts`, reduced).
//!
//! Agent Studio never replaces its own binary. The daemon asks GitHub Releases for the latest
//! release, compares versions and tells the user how to upgrade for the way the program was
//! installed (cargo, Homebrew, ... or a direct download of the right asset for this OS and CPU).
//!
//! * Prereleases and drafts are ignored, `404` means "no release published yet".
//! * Offline, timeouts and rate limits are not errors the user must see: the state records them and
//!   the background check stays silent.
//! * The HTTP call goes through [`HttpClient`] (the same seam as the usage limits) so tests never
//!   touch the network.
//! * `AGENT_STUDIO_UPDATE_URL` replaces the API URL (manual checks against a local server).

use std::cmp::Ordering;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::usage::credentials::Secret;
use crate::usage::limits_client::{HttpClient, HttpRequest};

pub const RELEASES_URL: &str =
    "https://api.github.com/repos/kawashita86/agent-studio/releases/latest";
pub const RELEASES_PAGE: &str = "https://github.com/kawashita86/agent-studio/releases";
pub const INSTALL_GIT_URL: &str = "https://github.com/kawashita86/agent-studio";
/// How often the daemon checks.
pub const CHECK_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_NOTES_CHARS: usize = 2000;

// ── versions ────────────────────────────────────────────────────────────────

/// A semantic version (`1.2.3`, `v1.2.3-rc.1`, build metadata ignored).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
    pub pre: Vec<String>,
}

impl Version {
    /// `None` for anything that is not `MAJOR.MINOR.PATCH` with an optional `-pre` / `+build`.
    pub fn parse(text: &str) -> Option<Version> {
        let text = text.trim();
        let text = text.strip_prefix(['v', 'V']).unwrap_or(text);
        let text = text.split('+').next()?;
        let (core, pre) = match text.split_once('-') {
            Some((core, pre)) => (core, Some(pre)),
            None => (text, None),
        };
        let mut nums = core.split('.');
        let major = nums.next()?.parse().ok()?;
        let minor = nums.next()?.parse().ok()?;
        let patch = nums.next()?.parse().ok()?;
        if nums.next().is_some() {
            return None;
        }
        let pre = match pre {
            Some("") => return None,
            Some(p) => p.split('.').map(str::to_string).collect(),
            None => Vec::new(),
        };
        Some(Version {
            major,
            minor,
            patch,
            pre,
        })
    }

    pub fn is_prerelease(&self) -> bool {
        !self.pre.is_empty()
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        (self.major, self.minor, self.patch)
            .cmp(&(other.major, other.minor, other.patch))
            .then_with(|| match (self.pre.is_empty(), other.pre.is_empty()) {
                (true, true) => Ordering::Equal,
                (true, false) => Ordering::Greater, // 1.0.0 > 1.0.0-rc.1
                (false, true) => Ordering::Less,
                (false, false) => cmp_pre(&self.pre, &other.pre),
            })
    }
}

fn cmp_pre(a: &[String], b: &[String]) -> Ordering {
    for (x, y) in a.iter().zip(b) {
        let ord = match (x.parse::<u64>(), y.parse::<u64>()) {
            (Ok(x), Ok(y)) => x.cmp(&y),
            (Ok(_), Err(_)) => Ordering::Less, // numeric identifiers sort before alphanumeric
            (Err(_), Ok(_)) => Ordering::Greater,
            (Err(_), Err(_)) => x.cmp(y),
        };
        if ord != Ordering::Equal {
            return ord;
        }
    }
    a.len().cmp(&b.len())
}

/// Whether `latest` is a newer version than `current`; unparsable input is never "newer".
pub fn is_newer(latest: &str, current: &str) -> bool {
    match (Version::parse(latest), Version::parse(current)) {
        (Some(l), Some(c)) => l > c,
        _ => false,
    }
}

// ── release API ─────────────────────────────────────────────────────────────

/// One downloadable file of a release.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseAsset {
    pub name: String,
    pub url: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    pub tag: String,
    pub version: Version,
    pub url: String,
    pub notes: String,
    pub assets: Vec<ReleaseAsset>,
}

/// The body was not a release object at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NotARelease;

/// Parse the body of `releases/latest`. `Ok(None)` for a draft, a prerelease or a tag that is not
/// a version; `Err` for JSON that is not a release at all.
pub fn parse_release(body: &str) -> Result<Option<Release>, NotARelease> {
    let v: Value = serde_json::from_str(body).map_err(|_| NotARelease)?;
    let obj = v.as_object().ok_or(NotARelease)?;
    let tag = obj
        .get("tag_name")
        .and_then(Value::as_str)
        .ok_or(NotARelease)?;
    let flag = |k: &str| obj.get(k).and_then(Value::as_bool).unwrap_or(false);
    if flag("draft") || flag("prerelease") {
        return Ok(None);
    }
    let Some(version) = Version::parse(tag) else {
        return Ok(None);
    };
    if version.is_prerelease() {
        return Ok(None);
    }
    let url = obj
        .get("html_url")
        .and_then(Value::as_str)
        .map_or_else(|| RELEASES_PAGE.to_string(), str::to_string);
    let notes: String = obj
        .get("body")
        .and_then(Value::as_str)
        .unwrap_or("")
        .chars()
        .take(MAX_NOTES_CHARS)
        .collect();
    let assets = obj
        .get("assets")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(|a| {
                    Some(ReleaseAsset {
                        name: a.get("name")?.as_str()?.to_string(),
                        url: a.get("browser_download_url")?.as_str()?.to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(Some(Release {
        tag: tag.to_string(),
        version,
        url,
        notes,
        assets,
    }))
}

/// The platform label in the names of release assets (`agent-studio-<tag>-<label>.<ext>`) for an OS
/// (`std::env::consts::OS`) and CPU (`std::env::consts::ARCH`); `None` where we publish nothing.
pub fn platform_label(os: &str, arch: &str) -> Option<&'static str> {
    Some(match (os, arch) {
        ("linux", "x86_64") => "linux-x86_64",
        ("macos", "x86_64") => "macos-x86_64",
        ("macos", "aarch64") => "macos-aarch64",
        ("windows", "x86_64") => "windows-x86_64",
        _ => return None,
    })
}

/// The installer of a release for this platform: `.AppImage` (Linux), `.dmg` (macOS) or `.msi` (Windows), named
/// `agent-studio-<tag>-<label>.<ext>` by the release workflow.
pub fn select_asset<'a>(
    assets: &'a [ReleaseAsset],
    os: &str,
    arch: &str,
) -> Option<&'a ReleaseAsset> {
    let label = platform_label(os, arch)?;
    let ext = match os {
        "windows" => ".msi",
        "macos" => ".dmg",
        _ => ".AppImage",
    };
    assets
        .iter()
        .find(|a| a.name.contains(label) && a.name.ends_with(ext))
}

// ── install method ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstallMethod {
    Cargo,
    Homebrew,
    Winget,
    Scoop,
    Chocolatey,
    /// A binary somewhere on disk (a release archive, copied by hand).
    Direct,
    /// Cannot tell (no executable path, or a build in a `target/` folder).
    Unknown,
}

/// Guess how the program was installed from the path of the running executable. Pure: the caller
/// passes the path (and `$CARGO_HOME` when set) so tests need no real installation.
pub fn detect_install_method(exe: Option<&Path>, cargo_home: Option<&Path>) -> InstallMethod {
    let Some(exe) = exe else {
        return InstallMethod::Unknown;
    };
    // Compare on a normalized string: lowercase, `/` separators.
    let norm = |p: &Path| p.to_string_lossy().replace('\\', "/").to_lowercase();
    let path = norm(exe);
    if let Some(home) = cargo_home {
        let bin = format!("{}/bin/", norm(home).trim_end_matches('/'));
        if path.starts_with(&bin) {
            return InstallMethod::Cargo;
        }
    }
    let has = |needle: &str| path.contains(needle);
    if has("/.cargo/bin/") {
        InstallMethod::Cargo
    } else if has("/cellar/") || has("/homebrew/") || has("/linuxbrew/") {
        InstallMethod::Homebrew
    } else if has("/winget/") || has("/windowsapps/") {
        InstallMethod::Winget
    } else if has("/scoop/") {
        InstallMethod::Scoop
    } else if has("/chocolatey/") {
        InstallMethod::Chocolatey
    } else if has("/target/debug/") || has("/target/release/") {
        InstallMethod::Unknown
    } else {
        InstallMethod::Direct
    }
}

/// [`detect_install_method`] for the running process.
pub fn detect_current_install_method() -> InstallMethod {
    let exe = std::env::current_exe().ok();
    let cargo_home = std::env::var_os("CARGO_HOME").map(std::path::PathBuf::from);
    detect_install_method(exe.as_deref(), cargo_home.as_deref())
}

/// The sentence that tells the user how to upgrade.
pub fn upgrade_hint(method: InstallMethod, release: Option<&UpdateInfo>) -> String {
    let download = || match release {
        Some(info) => format!(
            "Download {}",
            info.asset_url.as_deref().unwrap_or(info.url.as_str())
        ),
        None => format!("Download the latest release from {RELEASES_PAGE}"),
    };
    match method {
        InstallMethod::Cargo => {
            format!("Run `cargo install --git {INSTALL_GIT_URL} --locked --force`")
        }
        InstallMethod::Homebrew => "Run `brew upgrade agent-studio`".into(),
        InstallMethod::Winget => "Run `winget upgrade agent-studio`".into(),
        InstallMethod::Scoop => "Run `scoop update agent-studio`".into(),
        InstallMethod::Chocolatey => "Run `choco upgrade agent-studio`".into(),
        InstallMethod::Direct | InstallMethod::Unknown => download(),
    }
}

// ── state ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    pub version: String,
    pub tag: String,
    /// The release page.
    pub url: String,
    pub notes: String,
    /// The archive for this OS and CPU, when the release has one.
    pub asset_url: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckStatus {
    /// No check has run yet.
    #[default]
    Idle,
    UpToDate,
    Available,
    /// The repository has no (stable) release yet.
    NoReleases,
    /// Network unreachable or timed out.
    Offline,
    RateLimited,
    /// Any other unexpected answer.
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateState {
    /// `checkForUpdates` of the config (a manual check works either way).
    pub enabled: bool,
    pub current_version: String,
    pub status: CheckStatus,
    /// Epoch ms of the last finished check.
    pub checked_at_ms: Option<i64>,
    /// The newer release, when `status` is `Available`.
    pub latest: Option<UpdateInfo>,
    pub install_method: InstallMethod,
    /// How to upgrade; empty while nothing is available.
    pub hint: String,
}

impl UpdateState {
    pub fn available(&self) -> Option<&UpdateInfo> {
        self.latest
            .as_ref()
            .filter(|_| self.status == CheckStatus::Available)
    }

    /// One line for the CLI, the About section and toasts.
    pub fn summary(&self) -> String {
        match self.status {
            CheckStatus::Idle => format!("agent-studio {} (not checked yet)", self.current_version),
            CheckStatus::UpToDate => format!("agent-studio {} is up to date", self.current_version),
            CheckStatus::Available => match &self.latest {
                Some(info) => format!(
                    "Update available: {} (you have {})",
                    info.version, self.current_version
                ),
                None => "Update available".into(),
            },
            CheckStatus::NoReleases => format!(
                "agent-studio {}: no release published yet",
                self.current_version
            ),
            CheckStatus::Offline => "Could not reach GitHub (offline?)".into(),
            CheckStatus::RateLimited => "GitHub rate limit reached, try again later".into(),
            CheckStatus::Failed => "Update check failed".into(),
        }
    }
}

// ── service ─────────────────────────────────────────────────────────────────

pub struct UpdateOptions {
    pub http: Arc<dyn HttpClient>,
    pub current_version: String,
    pub url: String,
    /// `std::env::consts::OS` / `ARCH` (injected by tests).
    pub os: String,
    pub arch: String,
    pub install_method: InstallMethod,
}

impl UpdateOptions {
    /// The running program: real version, OS, CPU, install method; `AGENT_STUDIO_UPDATE_URL`
    /// overrides the API URL.
    pub fn production(http: Arc<dyn HttpClient>) -> Self {
        Self {
            http,
            current_version: crate::VERSION.to_string(),
            url: std::env::var("AGENT_STUDIO_UPDATE_URL")
                .ok()
                .filter(|u| !u.trim().is_empty())
                .unwrap_or_else(|| RELEASES_URL.to_string()),
            os: std::env::consts::OS.to_string(),
            arch: std::env::consts::ARCH.to_string(),
            install_method: detect_current_install_method(),
        }
    }
}

pub struct UpdateService {
    opts: UpdateOptions,
    state: Mutex<UpdateState>,
}

impl UpdateService {
    pub fn new(opts: UpdateOptions) -> Self {
        let state = UpdateState {
            enabled: true,
            current_version: opts.current_version.clone(),
            status: CheckStatus::Idle,
            checked_at_ms: None,
            latest: None,
            install_method: opts.install_method,
            hint: String::new(),
        };
        Self {
            opts,
            state: Mutex::new(state),
        }
    }

    pub fn state(&self) -> UpdateState {
        self.state.lock().clone()
    }

    pub fn set_enabled(&self, enabled: bool) {
        self.state.lock().enabled = enabled;
    }

    /// One request to the releases API (blocking: call from a blocking thread). Updates and
    /// returns the state; the previous `latest` is kept when the check could not complete.
    pub fn check(&self) -> UpdateState {
        let outcome = self.fetch();
        let mut state = self.state.lock();
        state.checked_at_ms = Some(chrono::Utc::now().timestamp_millis());
        match outcome {
            Outcome::Release(release) => {
                if is_newer(&release.tag, &self.opts.current_version) {
                    let info = UpdateInfo {
                        version: release.version_string(),
                        tag: release.tag.clone(),
                        url: release.url.clone(),
                        notes: release.notes.clone(),
                        asset_url: select_asset(&release.assets, &self.opts.os, &self.opts.arch)
                            .map(|a| a.url.clone()),
                    };
                    state.hint = upgrade_hint(self.opts.install_method, Some(&info));
                    state.latest = Some(info);
                    state.status = CheckStatus::Available;
                } else {
                    state.latest = None;
                    state.hint.clear();
                    state.status = CheckStatus::UpToDate;
                }
            }
            Outcome::NoRelease => {
                state.latest = None;
                state.hint.clear();
                state.status = CheckStatus::NoReleases;
            }
            Outcome::Status(status) => state.status = status,
        }
        tracing::debug!(status = ?state.status, "update check");
        state.clone()
    }

    fn fetch(&self) -> Outcome {
        let request = HttpRequest {
            url: self.opts.url.clone(),
            // Public API: no credential (an empty bearer is not sent).
            bearer: Secret::new(""),
            headers: vec![
                ("Accept".into(), "application/vnd.github+json".into()),
                (
                    "User-Agent".into(),
                    format!("agent-studio/{}", self.opts.current_version),
                ),
                ("X-GitHub-Api-Version".into(), "2022-11-28".into()),
            ],
            timeout: REQUEST_TIMEOUT,
        };
        let Ok(response) = self.opts.http.get(&request) else {
            return Outcome::Status(CheckStatus::Offline);
        };
        match response.status {
            200 => match parse_release(&response.body) {
                Ok(Some(release)) => Outcome::Release(release),
                // A draft/prerelease as "latest" (or an odd tag): nothing to offer.
                Ok(None) => Outcome::NoRelease,
                Err(NotARelease) => Outcome::Status(CheckStatus::Failed),
            },
            404 => Outcome::NoRelease,
            429 => Outcome::Status(CheckStatus::RateLimited),
            403 if response.retry_after.is_some()
                || response.body.to_ascii_lowercase().contains("rate limit") =>
            {
                Outcome::Status(CheckStatus::RateLimited)
            }
            _ => Outcome::Status(CheckStatus::Failed),
        }
    }
}

enum Outcome {
    Release(Release),
    NoRelease,
    Status(CheckStatus),
}

impl Release {
    fn version_string(&self) -> String {
        let v = &self.version;
        format!("{}.{}.{}", v.major, v.minor, v.patch)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage::limits_client::{HttpError, HttpResponse};
    use std::path::PathBuf;

    struct Mock(
        Mutex<Vec<Result<HttpResponse, HttpError>>>,
        Mutex<Vec<HttpRequest>>,
    );

    impl Mock {
        fn new(answers: Vec<Result<HttpResponse, HttpError>>) -> Arc<Self> {
            Arc::new(Mock(Mutex::new(answers), Mutex::new(Vec::new())))
        }
    }

    impl HttpClient for Mock {
        fn get(&self, request: &HttpRequest) -> Result<HttpResponse, HttpError> {
            self.1.lock().push(request.clone());
            let mut answers = self.0.lock();
            if answers.is_empty() {
                Err(HttpError)
            } else {
                answers.remove(0)
            }
        }
    }

    fn ok(status: u16, body: &str) -> Result<HttpResponse, HttpError> {
        Ok(HttpResponse {
            status,
            retry_after: None,
            body: body.to_string(),
        })
    }

    fn release_json(tag: &str, draft: bool, pre: bool) -> String {
        serde_json::json!({
            "tag_name": tag,
            "draft": draft,
            "prerelease": pre,
            "html_url": format!("https://github.com/x/y/releases/tag/{tag}"),
            "body": "notes",
            "assets": [
                {"name": format!("agent-studio-{tag}-linux-x86_64.AppImage"),
                 "browser_download_url": "https://dl/linux-x64.AppImage"},
                {"name": format!("agent-studio-{tag}-windows-x86_64.msi"),
                 "browser_download_url": "https://dl/win.msi"},
                {"name": "SHA256SUMS", "browser_download_url": "https://dl/sums"},
                {"broken": true}
            ]
        })
        .to_string()
    }

    fn service(
        current: &str,
        answers: Vec<Result<HttpResponse, HttpError>>,
        method: InstallMethod,
    ) -> (UpdateService, Arc<Mock>) {
        let mock = Mock::new(answers);
        let svc = UpdateService::new(UpdateOptions {
            http: mock.clone(),
            current_version: current.into(),
            url: "http://test/releases/latest".into(),
            os: "linux".into(),
            arch: "x86_64".into(),
            install_method: method,
        });
        (svc, mock)
    }

    #[test]
    fn parses_versions() {
        let v = Version::parse("v1.2.3").expect("v");
        assert_eq!((v.major, v.minor, v.patch), (1, 2, 3));
        assert!(
            Version::parse("1.2.3-rc.1+build5")
                .expect("pre")
                .is_prerelease()
        );
        for bad in ["", "1.2", "1.2.3.4", "a.b.c", "1.2.3-", "latest"] {
            assert!(Version::parse(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn compares_versions_like_semver() {
        assert!(is_newer("0.2.0", "0.1.9"));
        assert!(is_newer("v1.0.0", "0.99.99"));
        assert!(is_newer("0.10.0", "0.9.0"), "numeric, not lexicographic");
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("0.1.0", "0.2.0"));
        assert!(
            is_newer("1.0.0", "1.0.0-rc.1"),
            "release beats its prerelease"
        );
        assert!(!is_newer("1.0.0-rc.1", "1.0.0"));
        assert!(is_newer("1.0.0-rc.2", "1.0.0-rc.1"));
        assert!(is_newer("1.0.0-rc.10", "1.0.0-rc.9"));
        assert!(is_newer("1.0.0-rc.1", "1.0.0-beta.9"));
        assert!(!is_newer("garbage", "1.0.0"));
        assert!(!is_newer("1.0.1", "garbage"));
    }

    #[test]
    fn parses_release_json() {
        let r = parse_release(&release_json("v0.2.0", false, false))
            .expect("json")
            .expect("release");
        assert_eq!(r.tag, "v0.2.0");
        assert_eq!(r.assets.len(), 3, "the broken asset entry is skipped");
        assert!(r.url.ends_with("v0.2.0"));
        assert_eq!(
            parse_release(&release_json("v0.2.0", true, false)),
            Ok(None)
        );
        assert_eq!(
            parse_release(&release_json("v0.2.0", false, true)),
            Ok(None)
        );
        assert_eq!(
            parse_release(&release_json("v0.2.0-rc.1", false, false)),
            Ok(None)
        );
        assert_eq!(
            parse_release(&release_json("nightly", false, false)),
            Ok(None)
        );
        assert_eq!(parse_release("not json"), Err(NotARelease));
        assert_eq!(parse_release("[]"), Err(NotARelease));
        assert_eq!(parse_release("{}"), Err(NotARelease));
    }

    #[test]
    fn picks_the_asset_of_the_platform() {
        let r = parse_release(&release_json("v0.2.0", false, false))
            .expect("json")
            .expect("release");
        let pick = |os, arch| select_asset(&r.assets, os, arch).map(|a| a.url.as_str());
        assert_eq!(
            pick("linux", "x86_64"),
            Some("https://dl/linux-x64.AppImage")
        );
        assert_eq!(pick("windows", "x86_64"), Some("https://dl/win.msi"));
        assert_eq!(pick("linux", "aarch64"), None, "not in this release");
        assert_eq!(pick("freebsd", "x86_64"), None);
        assert_eq!(platform_label("macos", "aarch64"), Some("macos-aarch64"));
        assert_eq!(platform_label("macos", "x86_64"), Some("macos-x86_64"));
        assert_eq!(platform_label("linux", "riscv64"), None);
    }

    #[test]
    fn detects_the_install_method() {
        let d = |p: &str| detect_install_method(Some(&PathBuf::from(p)), None);
        assert_eq!(d("/home/me/.cargo/bin/agent-studio"), InstallMethod::Cargo);
        assert_eq!(
            d("C:\\Users\\me\\.cargo\\bin\\agent-studio.exe"),
            InstallMethod::Cargo
        );
        assert_eq!(
            d("/opt/homebrew/Cellar/agent-studio/0.1.0/bin/agent-studio"),
            InstallMethod::Homebrew
        );
        assert_eq!(
            d("/home/linuxbrew/.linuxbrew/bin/agent-studio"),
            InstallMethod::Homebrew
        );
        assert_eq!(
            d("C:\\Users\\me\\scoop\\apps\\agent-studio\\current\\agent-studio.exe"),
            InstallMethod::Scoop
        );
        assert_eq!(
            d("C:\\ProgramData\\chocolatey\\bin\\agent-studio.exe"),
            InstallMethod::Chocolatey
        );
        assert_eq!(
            d("C:\\Users\\me\\AppData\\Local\\Microsoft\\WinGet\\Packages\\x\\agent-studio.exe"),
            InstallMethod::Winget
        );
        assert_eq!(d("/usr/local/bin/agent-studio"), InstallMethod::Direct);
        assert_eq!(
            d("/src/agent-studio/target/debug/agent-studio"),
            InstallMethod::Unknown
        );
        assert_eq!(detect_install_method(None, None), InstallMethod::Unknown);
        // A custom CARGO_HOME.
        let exe = PathBuf::from("/data/cargo/bin/agent-studio");
        assert_eq!(
            detect_install_method(Some(&exe), Some(&PathBuf::from("/data/cargo"))),
            InstallMethod::Cargo
        );
        assert_eq!(
            detect_install_method(Some(&exe), None),
            InstallMethod::Direct
        );
    }

    #[test]
    fn hints_match_the_install_method() {
        assert!(upgrade_hint(InstallMethod::Homebrew, None).contains("brew upgrade agent-studio"));
        assert!(upgrade_hint(InstallMethod::Cargo, None).contains("cargo install"));
        assert!(upgrade_hint(InstallMethod::Scoop, None).contains("scoop update"));
        assert!(upgrade_hint(InstallMethod::Unknown, None).contains(RELEASES_PAGE));
    }

    #[test]
    fn available_update_with_asset_and_hint() {
        let (svc, mock) = service(
            "0.1.0",
            vec![ok(200, &release_json("v0.2.0", false, false))],
            InstallMethod::Direct,
        );
        let state = svc.check();
        assert_eq!(state.status, CheckStatus::Available);
        let info = state.available().expect("available");
        assert_eq!(info.version, "0.2.0");
        assert_eq!(
            info.asset_url.as_deref(),
            Some("https://dl/linux-x64.AppImage")
        );
        assert!(state.hint.contains("https://dl/linux-x64.AppImage"));
        assert!(state.checked_at_ms.is_some());
        // The credential-less request carries the GitHub headers and an empty bearer.
        let req = mock.1.lock()[0].clone();
        assert_eq!(req.bearer.expose(), "");
        assert!(req.headers.iter().any(|(k, _)| k == "User-Agent"));
        assert_eq!(svc.state(), state);
    }

    #[test]
    fn same_or_older_release_is_up_to_date() {
        let (svc, _) = service(
            "0.2.0",
            vec![
                ok(200, &release_json("v0.2.0", false, false)),
                ok(200, &release_json("v0.1.0", false, false)),
            ],
            InstallMethod::Cargo,
        );
        assert_eq!(svc.check().status, CheckStatus::UpToDate);
        let s = svc.check();
        assert_eq!(s.status, CheckStatus::UpToDate);
        assert!(s.latest.is_none() && s.hint.is_empty());
    }

    #[test]
    fn no_release_offline_rate_limit_and_failures() {
        let (svc, _) = service(
            "0.1.0",
            vec![
                ok(404, "{\"message\":\"Not Found\"}"),
                Err(HttpError),
                ok(429, ""),
                ok(403, "{\"message\":\"API rate limit exceeded\"}"),
                ok(500, "oops"),
                ok(200, "garbage"),
                ok(200, &release_json("v9.0.0", false, true)),
            ],
            InstallMethod::Direct,
        );
        let statuses: Vec<_> = (0..7).map(|_| svc.check().status).collect();
        assert_eq!(
            statuses,
            [
                CheckStatus::NoReleases,
                CheckStatus::Offline,
                CheckStatus::RateLimited,
                CheckStatus::RateLimited,
                CheckStatus::Failed,
                CheckStatus::Failed,
                CheckStatus::NoReleases,
            ]
        );
    }

    #[test]
    fn a_failed_check_keeps_the_known_update() {
        let (svc, _) = service(
            "0.1.0",
            vec![
                ok(200, &release_json("v0.2.0", false, false)),
                Err(HttpError),
            ],
            InstallMethod::Direct,
        );
        svc.check();
        let s = svc.check();
        assert_eq!(s.status, CheckStatus::Offline);
        assert!(s.latest.is_some(), "kept for the next banner");
        assert!(
            s.available().is_none(),
            "but it is not announced as available"
        );
    }

    #[test]
    fn summary_lines() {
        let (svc, _) = service("0.1.0", vec![ok(404, "")], InstallMethod::Direct);
        assert!(svc.state().summary().contains("not checked"));
        assert!(svc.check().summary().contains("no release published"));
    }
}
