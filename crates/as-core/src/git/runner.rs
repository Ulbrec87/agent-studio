//! Running `git` and `gh`: environment, timeouts, error mapping, per-repo serialization.

use std::collections::HashMap;
use std::ffi::OsString;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

use parking_lot::Mutex;

use super::error::{GitError, GitResult};

/// Default timeout of a `git` call.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
/// Timeout of calls that talk to the network (`push`, `gh`).
pub const NETWORK_TIMEOUT: Duration = Duration::from_secs(60);

/// Build a `Vec<OsString>` of arguments from anything `OsString: From`.
macro_rules! args {
    ($($a:expr),* $(,)?) => { vec![$(::std::ffi::OsString::from($a)),*] };
}
pub(crate) use args;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Program {
    Git,
    Gh,
}

impl Program {
    fn name(self) -> &'static str {
        match self {
            Program::Git => "git",
            Program::Gh => "gh",
        }
    }
    fn missing(self) -> GitError {
        match self {
            Program::Git => GitError::GitMissing,
            Program::Gh => GitError::GhMissing,
        }
    }
}

pub(crate) struct RawOutput {
    pub stdout: Vec<u8>,
    pub stderr: String,
}

impl RawOutput {
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }
}

/// Runs `git`/`gh`. Cheap to clone; the defaults are right for production. Tests can override
/// `PATH` and the child environment without touching the process environment.
#[derive(Debug, Clone, Default)]
pub struct GitRunner {
    path: Option<OsString>,
    env: Vec<(OsString, OsString)>,
    timeout: Option<Duration>,
}

static REPO_LOCKS: LazyLock<Mutex<HashMap<PathBuf, Arc<Mutex<()>>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

impl GitRunner {
    pub fn new() -> Self {
        Self::default()
    }

    /// Look programs up (and run them) with this `PATH` instead of the process one.
    pub fn with_path(mut self, path: impl Into<OsString>) -> Self {
        self.path = Some(path.into());
        self
    }

    /// Extra environment variable of every child (e.g. `GIT_CONFIG_GLOBAL` in tests).
    pub fn with_env(mut self, key: impl Into<OsString>, value: impl Into<OsString>) -> Self {
        self.env.push((key.into(), value.into()));
        self
    }

    /// Override the timeout of every call.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    fn resolve(&self, program: Program, dir: &Path) -> GitResult<PathBuf> {
        let found = match &self.path {
            Some(p) => which::which_in(program.name(), Some(p), dir),
            None => which::which(program.name()),
        };
        found.map_err(|_| program.missing())
    }

    /// Run `program` in `dir`; exit codes in `ok_codes` count as success.
    pub(crate) fn exec(
        &self,
        program: Program,
        dir: &Path,
        args: &[OsString],
        timeout: Duration,
        ok_codes: &[i32],
    ) -> GitResult<RawOutput> {
        if !dir.is_dir() {
            return Err(GitError::NotARepo(dir.display().to_string()));
        }
        let exe = self.resolve(program, dir)?;
        let mut cmd = Command::new(exe);
        match program {
            Program::Git => {
                cmd.arg("-C")
                    .arg(dir)
                    .args(["-c", "core.quotepath=off", "--no-pager"]);
                cmd.env("GIT_TERMINAL_PROMPT", "0")
                    .env("GCM_INTERACTIVE", "never")
                    .env("GIT_LITERAL_PATHSPECS", "1")
                    .env("GIT_OPTIONAL_LOCKS", "0");
            }
            Program::Gh => {
                cmd.current_dir(dir);
                cmd.env("GH_PROMPT_DISABLED", "1")
                    .env("GH_NO_UPDATE_NOTIFIER", "1")
                    .env("NO_COLOR", "1");
            }
        }
        cmd.args(args)
            .env("LC_ALL", "C")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(p) = &self.path {
            cmd.env("PATH", p);
        }
        for (k, v) in &self.env {
            cmd.env(k, v);
        }
        let timeout = self.timeout.unwrap_or(timeout);
        let mut child = cmd.spawn().map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                program.missing()
            } else {
                GitError::Io(e)
            }
        })?;
        let (tx, rx) = mpsc::channel::<(bool, Vec<u8>)>();
        for (is_err, pipe) in [
            (
                false,
                child
                    .stdout
                    .take()
                    .map(|p| Box::new(p) as Box<dyn Read + Send>),
            ),
            (
                true,
                child
                    .stderr
                    .take()
                    .map(|p| Box::new(p) as Box<dyn Read + Send>),
            ),
        ] {
            let tx = tx.clone();
            std::thread::spawn(move || {
                let mut buf = Vec::new();
                if let Some(mut p) = pipe {
                    let _ = p.read_to_end(&mut buf);
                }
                let _ = tx.send((is_err, buf));
            });
        }
        drop(tx);
        let deadline = Instant::now() + timeout;
        let status = loop {
            match child.try_wait()? {
                Some(s) => break s,
                None if Instant::now() >= deadline => {
                    let _ = child.kill();
                    let _ = child.wait();
                    tracing::warn!(program = program.name(), ?args, "git call timed out");
                    return Err(GitError::Timeout {
                        program: program.name().into(),
                        secs: timeout.as_secs(),
                    });
                }
                None => std::thread::sleep(Duration::from_millis(5)),
            }
        };
        // A grandchild (ssh, credential helper) can keep the pipes open: wait for them only briefly.
        let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
        let grace = Instant::now() + Duration::from_secs(2);
        for _ in 0..2 {
            let left = grace.saturating_duration_since(Instant::now());
            match rx.recv_timeout(left) {
                Ok((true, b)) => stderr = b,
                Ok((false, b)) => stdout = b,
                Err(_) => break,
            }
        }
        let code = status.code();
        let stderr = String::from_utf8_lossy(&stderr).trim().to_string();
        if status.success() || code.is_some_and(|c| ok_codes.contains(&c)) {
            return Ok(RawOutput { stdout, stderr });
        }
        let stderr = if stderr.is_empty() {
            String::from_utf8_lossy(&stdout).trim().to_string()
        } else {
            stderr
        };
        if stderr.contains("not a git repository") {
            return Err(GitError::NotARepo(dir.display().to_string()));
        }
        Err(GitError::Failed { code, stderr })
    }

    /// `git <args>` with the default timeout; stdout as text.
    pub(crate) fn git(&self, dir: &Path, args: &[OsString]) -> GitResult<String> {
        Ok(self
            .exec(Program::Git, dir, args, DEFAULT_TIMEOUT, &[])?
            .text())
    }

    pub(crate) fn git_raw(&self, dir: &Path, args: &[OsString]) -> GitResult<RawOutput> {
        self.exec(Program::Git, dir, args, DEFAULT_TIMEOUT, &[])
    }

    /// `true` when `git <args>` exits 0 (any failure, including a missing repo, is `false`).
    pub(crate) fn git_ok(&self, dir: &Path, args: &[OsString]) -> bool {
        self.git_raw(dir, args).is_ok()
    }

    pub(crate) fn gh(&self, dir: &Path, args: &[OsString]) -> GitResult<String> {
        Ok(self
            .exec(Program::Gh, dir, args, NETWORK_TIMEOUT, &[])?
            .text())
    }

    /// Canonical top level of the repository containing `dir` (the key of the repo lock).
    pub fn repo_root(&self, dir: &Path) -> GitResult<PathBuf> {
        let out = self.git(dir, &args!["rev-parse", "--show-toplevel"])?;
        let top = out.trim();
        if top.is_empty() {
            return Err(GitError::NotARepo(dir.display().to_string()));
        }
        Ok(PathBuf::from(top))
    }

    /// Run `f` while holding the repository's lock (serializes mutating calls per repository).
    pub(crate) fn with_repo_lock<T>(
        &self,
        root: &Path,
        f: impl FnOnce() -> GitResult<T>,
    ) -> GitResult<T> {
        let key = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
        let lock = REPO_LOCKS.lock().entry(key).or_default().clone();
        let _guard = lock.lock();
        f()
    }
}

/// Path argument for `git`: always `/`-separated.
pub(crate) fn pathspec(p: &str) -> OsString {
    OsString::from(p.replace('\\', "/"))
}
