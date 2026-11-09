//! Finding, starting and stopping the daemon (Hydra `electron/daemon/lifecycle.ts`).
//!
//! `ensure_daemon`:
//! 1. probe the socket; a dead lock/socket pair is removed;
//! 2. nothing alive -> spawn `current_exe daemon --home <root>` fully detached and retry
//!    connecting with backoff for up to [`STARTUP_TIMEOUT`];
//! 3. `Hello` handshake. **Version policy** (Hydra has none; we add a safe one): when the daemon's
//!    version or protocol differs from ours, an *idle* daemon (no running agent) is asked to
//!    `Shutdown { stop_agents: false }` and replaced by a fresh one; a daemon with running agents
//!    is **never** restarted automatically, we keep talking to it (warning logged) as long as the
//!    protocol version matches, and fail with an actionable error when it does not.

use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use as_core::paths::Paths;

use crate::client::DaemonClient;
use crate::lock::{self, Probe};
use crate::message::{HelloInfo, PROTOCOL_VERSION};

pub const STARTUP_TIMEOUT: Duration = Duration::from_secs(5);

/// A client connected to a running, handshaken daemon (starting one when needed).
pub async fn ensure_daemon(paths: &Paths) -> Result<DaemonClient> {
    ensure_daemon_with(paths, spawn_daemon).await
}

/// [`ensure_daemon`] with an injectable spawner (tests).
pub async fn ensure_daemon_with(
    paths: &Paths,
    spawn: impl Fn(&Paths) -> Result<()>,
) -> Result<DaemonClient> {
    let (client, hello) = connect_or_spawn(paths, &spawn).await?;
    if compatible(&hello) {
        return Ok(client);
    }
    let status = client.status().await.ok();
    let idle = status.as_ref().is_some_and(|s| s.running_count == 0);
    if idle && client.shutdown(false).await.is_ok() {
        tracing::info!(daemon = %hello.daemon_version, ours = as_core::VERSION, "replacing idle daemon of another version");
        drop(client);
        wait_gone(paths, STARTUP_TIMEOUT).await;
        let (client, hello) = connect_or_spawn(paths, &spawn).await?;
        if hello.protocol != PROTOCOL_VERSION {
            bail!(
                "daemon protocol {} is incompatible with ours",
                hello.protocol
            );
        }
        return Ok(client);
    }
    if hello.protocol != PROTOCOL_VERSION {
        bail!(
            "the running daemon (v{}, protocol {}) is incompatible with this client (v{}, protocol {}) \
             and has running agents; run `agent-studio stop --force` to stop them and the daemon",
            hello.daemon_version,
            hello.protocol,
            as_core::VERSION,
            PROTOCOL_VERSION
        );
    }
    tracing::warn!(
        daemon = %hello.daemon_version,
        ours = as_core::VERSION,
        "daemon version differs but agents are running: keeping it"
    );
    Ok(client)
}

fn compatible(hello: &HelloInfo) -> bool {
    hello.protocol == PROTOCOL_VERSION && hello.daemon_version == as_core::VERSION
}

async fn connect_or_spawn(
    paths: &Paths,
    spawn: &impl Fn(&Paths) -> Result<()>,
) -> Result<(DaemonClient, HelloInfo)> {
    if let Probe::Stale = lock::probe(paths).await {
        tracing::info!("removing stale daemon lock/socket");
        lock::clear_stale(paths);
    }
    if let Ok(found) = try_connect(paths).await {
        return Ok(found);
    }
    spawn(paths)?;
    let deadline = Instant::now() + STARTUP_TIMEOUT;
    let mut delay = Duration::from_millis(20);
    loop {
        match try_connect(paths).await {
            Ok(found) => return Ok(found),
            Err(err) if Instant::now() >= deadline => {
                return Err(err).context(format!(
                    "the daemon did not start within {}s (see {})",
                    STARTUP_TIMEOUT.as_secs(),
                    paths.logs_dir().display()
                ));
            }
            Err(_) => {
                tokio::time::sleep(delay).await;
                delay = (delay * 2).min(Duration::from_millis(250));
            }
        }
    }
}

async fn try_connect(paths: &Paths) -> Result<(DaemonClient, HelloInfo)> {
    let client = DaemonClient::connect(paths).await?;
    let hello = tokio::time::timeout(Duration::from_secs(5), client.hello())
        .await
        .context("daemon handshake timed out")??;
    Ok((client, hello))
}

/// Wait until nothing answers on the socket any more.
async fn wait_gone(paths: &Paths, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if !matches!(lock::probe(paths).await, Probe::Live(_)) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Start `current_exe daemon --home <root>` detached from this process and terminal.
pub fn spawn_daemon(paths: &Paths) -> Result<()> {
    let exe = std::env::current_exe().context("cannot locate the agent-studio executable")?;
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("daemon")
        .arg("--home")
        .arg(paths.root())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    // Unix: the daemon calls `setsid()` itself right after starting (`as_daemon::run`), which
    // detaches it from the terminal and its job control. It must not be a process group leader
    // for that, so no `process_group(0)` here.
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NEW_PROCESS_GROUP | DETACHED_PROCESS | CREATE_NO_WINDOW);
    }
    let child = cmd.spawn().context("spawning the daemon")?;
    // Reap it from a thread when it exits early, so a failed start is not left as a zombie.
    std::thread::spawn(move || {
        let mut child = child;
        let _ = child.wait();
    });
    Ok(())
}

/// Stop the daemon if one is running (never starts one). Returns false when none was running.
/// `force` stops the agents too; without it the daemon refuses while agents are running.
/// An unresponsive daemon is terminated by pid as a last resort.
pub async fn stop_daemon(paths: &Paths, force: bool) -> Result<bool> {
    let lock_data = match lock::probe(paths).await {
        Probe::Absent => return Ok(false),
        Probe::Stale => {
            lock::clear_stale(paths);
            return Ok(false);
        }
        Probe::Live(lock) => lock,
    };
    let result = match DaemonClient::connect(paths).await {
        Ok(client) => client.shutdown(force).await,
        Err(err) => Err(err),
    };
    if let Err(err) = result {
        let refused = err.to_string().contains("agents are running");
        if refused || !force {
            return Err(err);
        }
        tracing::warn!(%err, "daemon unresponsive, terminating by pid");
        if let Some(l) = lock_data {
            terminate_pid(l.pid);
        }
    }
    wait_gone(paths, Duration::from_secs(10)).await;
    if matches!(lock::probe(paths).await, Probe::Stale) {
        lock::clear_stale(paths);
    }
    Ok(true)
}

#[cfg(unix)]
fn terminate_pid(pid: u32) {
    use nix::sys::signal::{Signal, kill};
    use nix::unistd::Pid;
    if let Ok(raw) = i32::try_from(pid) {
        let _ = kill(Pid::from_raw(raw), Signal::SIGTERM);
    }
}

#[cfg(not(unix))]
fn terminate_pid(pid: u32) {
    let _ = std::process::Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/F"])
        .output();
}
