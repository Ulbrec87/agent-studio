//! PTY abstraction used by the agent manager.
//!
//! [`NativePtyBackend`] wraps `portable-pty` (ConPTY on Windows). The manager only
//! talks to the [`PtyBackend`] trait so tests can use [`super::fake_pty::FakePtyBackend`].

use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::sync::Arc;

use anyhow::{Context, Result};
use parking_lot::Mutex;
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
        // ConPTY emits a cursor-position query (`ESC [ 6 n`) at startup and the child does not
        // run until the terminal answers it, so answer it ourselves (Windows only).
        let (reader, writer): (Box<dyn Read + Send>, Box<dyn Write + Send>) = if cfg!(windows) {
            let shared = Arc::new(Mutex::new(writer));
            (
                Box::new(DsrResponder::new(reader, shared.clone())),
                Box::new(SharedWriter(shared)),
            )
        } else {
            (reader, writer)
        };
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

/// Writer shared between the manager and the [`DsrResponder`].
struct SharedWriter(Arc<Mutex<Box<dyn Write + Send>>>);

impl Write for SharedWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.lock().write(buf)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.0.lock().flush()
    }
}

const DSR_QUERY: &[u8] = b"\x1b[6n";
const DSR_REPLY: &[u8] = b"\x1b[1;1R";

/// Reader that answers every cursor-position query (DSR) found in the output stream. The
/// output is passed through unchanged (the real terminal widget answers too; the extra reply
/// at the cursor position 1;1 is harmless for a child that does not ask).
struct DsrResponder<R> {
    inner: R,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    /// Trailing bytes of the previous chunk that may start a split query.
    carry: Vec<u8>,
}

impl<R> DsrResponder<R> {
    fn new(inner: R, writer: Arc<Mutex<Box<dyn Write + Send>>>) -> Self {
        Self {
            inner,
            writer,
            carry: Vec::new(),
        }
    }

    fn scan(&mut self, chunk: &[u8]) {
        let mut data = std::mem::take(&mut self.carry);
        data.extend_from_slice(chunk);
        let count = data
            .windows(DSR_QUERY.len())
            .filter(|w| *w == DSR_QUERY)
            .count();
        for _ in 0..count {
            let mut w = self.writer.lock();
            let _ = w.write_all(DSR_REPLY).and_then(|()| w.flush());
        }
        let keep = DSR_QUERY.len() - 1;
        let start = data.len().saturating_sub(keep);
        // Keep only a tail that is a proper prefix of the query.
        self.carry = (start..data.len())
            .map(|i| &data[i..])
            .find(|t| DSR_QUERY.starts_with(t))
            .map(<[u8]>::to_vec)
            .unwrap_or_default();
    }
}

impl<R: Read> Read for DsrResponder<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        if n > 0 {
            let chunk = buf[..n].to_vec();
            self.scan(&chunk);
        }
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Sink(Arc<Mutex<Vec<u8>>>);
    impl Write for Sink {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.lock().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn run(chunks: &[&[u8]]) -> (Vec<u8>, Vec<u8>) {
        let sink = Arc::new(Mutex::new(Vec::new()));
        let writer: Arc<Mutex<Box<dyn Write + Send>>> =
            Arc::new(Mutex::new(Box::new(Sink(sink.clone()))));
        let mut out = Vec::new();
        for c in chunks {
            let mut r = DsrResponder::new(*c, writer.clone());
            let mut buf = [0u8; 64];
            let n = r.read(&mut buf).unwrap();
            out.extend_from_slice(&buf[..n]);
            // Re-use the carry across chunks by scanning through one responder instead.
            let _ = r;
        }
        let replies = sink.lock().clone();
        (out, replies)
    }

    #[test]
    fn answers_a_query_and_passes_output_through() {
        let (out, replies) = run(&[b"a\x1b[6nb"]);
        assert_eq!(out, b"a\x1b[6nb");
        assert_eq!(replies, DSR_REPLY);
    }

    #[test]
    fn answers_a_query_split_across_chunks() {
        let sink = Arc::new(Mutex::new(Vec::new()));
        let writer: Arc<Mutex<Box<dyn Write + Send>>> =
            Arc::new(Mutex::new(Box::new(Sink(sink.clone()))));
        let mut r = DsrResponder::new(io::empty(), writer);
        r.scan(b"xx\x1b[");
        assert!(sink.lock().is_empty());
        r.scan(b"6nyy\x1b[6n");
        assert_eq!(*sink.lock(), [DSR_REPLY, DSR_REPLY].concat());
    }

    #[test]
    fn plain_output_gets_no_reply() {
        let (_, replies) = run(&[b"hello \x1b[31mred\x1b[0m"]);
        assert!(replies.is_empty());
    }
}
