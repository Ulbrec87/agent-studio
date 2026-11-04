//! PTY abstraction used by the agent manager.
//!
//! [`NativePtyBackend`] wraps `portable-pty` (ConPTY on Windows). The manager only
//! talks to the [`PtyBackend`] trait so tests can use [`super::fake_pty::FakePtyBackend`].

use std::collections::HashMap;
use std::io::{self, Read, Write};

use anyhow::{Context, Result};
use portable_pty::{ChildKiller, CommandBuilder, MasterPty, PtySize, native_pty_system};

/// Everything needed to start one process in a PTY.
#[derive(Debug, Clone)]
pub struct PtySpawnRequest {
    pub cmd: String,
    pub args: Vec<String>,
    /// Working directory; empty means inherit.
    pub cwd: String,
    /// The complete environment (the parent env is NOT inherited).
    pub env: HashMap<String, String>,
    pub cols: u16,
    pub rows: u16,
}

/// Resize / signal handle of a running PTY. Dropping it closes the PTY.
pub trait PtyControl: Send {
    fn resize(&mut self, cols: u16, rows: u16) -> io::Result<()>;
    /// `force == false`: graceful (SIGTERM on Unix; Windows has no graceful signal, so it kills).
    /// `force == true`: SIGKILL / TerminateProcess.
    fn terminate(&mut self, force: bool);
}

/// Blocks until the process exits and returns its exit code (1 when unknown).
pub trait PtyWaiter: Send {
    fn wait(&mut self) -> i32;
}

/// A spawned process: blocking reader/writer plus control and wait handles.
pub struct PtyProcess {
    pub pid: Option<u32>,
    pub reader: Box<dyn Read + Send>,
    pub writer: Box<dyn Write + Send>,
    pub control: Box<dyn PtyControl>,
    pub waiter: Box<dyn PtyWaiter>,
}

pub trait PtyBackend: Send + Sync + 'static {
    fn spawn(&self, request: &PtySpawnRequest) -> Result<PtyProcess>;
}

/// The real thing: `portable-pty`.
#[derive(Debug, Default, Clone, Copy)]
pub struct NativePtyBackend;

struct NativeControl {
    master: Box<dyn MasterPty + Send>,
    killer: Box<dyn ChildKiller + Send + Sync>,
    #[cfg_attr(not(unix), allow(dead_code))]
    pid: Option<u32>,
}

impl PtyControl for NativeControl {
    fn resize(&mut self, cols: u16, rows: u16) -> io::Result<()> {
        self.master
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| io::Error::other(e.to_string()))
    }

    fn terminate(&mut self, force: bool) {
        #[cfg(unix)]
        if let Some(pid) = self.pid.and_then(|p| i32::try_from(p).ok()) {
            use nix::sys::signal::{Signal, kill};
            let signal = if force {
                Signal::SIGKILL
            } else {
                Signal::SIGTERM
            };
            // ESRCH (already gone) is fine.
            let _ = kill(nix::unistd::Pid::from_raw(pid), signal);
            return;
        }
        let _ = force;
        let _ = self.killer.kill();
    }
}

struct NativeWaiter(Box<dyn portable_pty::Child + Send>);

impl PtyWaiter for NativeWaiter {
    fn wait(&mut self) -> i32 {
        match self.0.wait() {
            Ok(status) => i32::try_from(status.exit_code()).unwrap_or(1),
            Err(_) => 1,
        }
    }
}

impl PtyBackend for NativePtyBackend {
    fn spawn(&self, req: &PtySpawnRequest) -> Result<PtyProcess> {
        let pair = native_pty_system()
            .openpty(PtySize {
                rows: req.rows,
                cols: req.cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("opening a PTY")?;
        let mut cmd = CommandBuilder::new(&req.cmd);
        cmd.args(&req.args);
        if !req.cwd.is_empty() {
            cmd.cwd(&req.cwd);
        }
        cmd.env_clear();
        for (k, v) in &req.env {
            cmd.env(k, v);
        }
        let child = pair
            .slave
            .spawn_command(cmd)
            .with_context(|| format!("spawning {}", req.cmd))?;
        // Drop our slave handle, otherwise the reader never sees EOF after the child exits.
        drop(pair.slave);
        let pid = child.process_id();
        let killer = child.clone_killer();
        let reader = pair
            .master
            .try_clone_reader()
            .context("cloning the PTY reader")?;
        let writer = pair.master.take_writer().context("taking the PTY writer")?;
        Ok(PtyProcess {
            pid,
            reader,
            writer,
            control: Box::new(NativeControl {
                master: pair.master,
                killer,
                pid,
            }),
            waiter: Box::new(NativeWaiter(child)),
        })
    }
}
