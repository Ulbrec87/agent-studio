//! In-memory PTY backend for tests (also usable by daemon tests).

use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};

use anyhow::{Result, anyhow};
use parking_lot::Mutex;

use super::pty::{PtyBackend, PtyControl, PtyProcess, PtySpawnRequest, PtyWaiter};

/// Handle to one fake process, kept by the test.
#[derive(Clone)]
pub struct FakePty {
    pub pid: u32,
    writes: Arc<Mutex<Vec<u8>>>,
    data_tx: Arc<Mutex<Option<Sender<Vec<u8>>>>>,
    exit_tx: Sender<i32>,
    signals: Arc<Mutex<Vec<bool>>>,
    resizes: Arc<Mutex<Vec<(u16, u16)>>>,
}

impl FakePty {
    /// Everything written to the PTY so far (lossy UTF-8).
    pub fn written(&self) -> String {
        String::from_utf8_lossy(&self.writes.lock()).into_owned()
    }
    /// Make the process "print" `data`.
    pub fn emit_data(&self, data: &[u8]) {
        if let Some(tx) = self.data_tx.lock().as_ref() {
            let _ = tx.send(data.to_vec());
        }
    }
    /// Make the process exit with `code` (also closes the output stream).
    pub fn emit_exit(&self, code: i32) {
        self.data_tx.lock().take();
        let _ = self.exit_tx.send(code);
    }
    /// `terminate(force)` calls received, in order.
    pub fn signals(&self) -> Vec<bool> {
        self.signals.lock().clone()
    }
    pub fn resizes(&self) -> Vec<(u16, u16)> {
        self.resizes.lock().clone()
    }
}

struct ChannelReader {
    rx: Receiver<Vec<u8>>,
    pending: Vec<u8>,
}

impl Read for ChannelReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.pending.is_empty() {
            match self.rx.recv() {
                Ok(chunk) => self.pending = chunk,
                Err(_) => return Ok(0),
            }
        }
        let n = buf.len().min(self.pending.len());
        buf[..n].copy_from_slice(&self.pending[..n]);
        self.pending.drain(..n);
        Ok(n)
    }
}

struct VecWriter(Arc<Mutex<Vec<u8>>>);
impl Write for VecWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.lock().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

struct FakeControl {
    signals: Arc<Mutex<Vec<bool>>>,
    resizes: Arc<Mutex<Vec<(u16, u16)>>>,
}
impl PtyControl for FakeControl {
    fn resize(&mut self, cols: u16, rows: u16) -> io::Result<()> {
        self.resizes.lock().push((cols, rows));
        Ok(())
    }
    fn terminate(&mut self, force: bool) {
        self.signals.lock().push(force);
    }
}

struct FakeWaiter(Receiver<i32>);
impl PtyWaiter for FakeWaiter {
    fn wait(&mut self) -> i32 {
        self.0.recv().unwrap_or(1)
    }
}

#[derive(Default)]
struct Registry {
    ptys: Vec<FakePty>,
    requests: Vec<PtySpawnRequest>,
    fail_with: Option<String>,
}

/// A [`PtyBackend`] whose processes are driven by the test.
#[derive(Clone, Default)]
pub struct FakePtyBackend {
    registry: Arc<Mutex<Registry>>,
}

impl FakePtyBackend {
    pub fn new() -> Self {
        Self::default()
    }
    /// Processes spawned so far, oldest first.
    pub fn ptys(&self) -> Vec<FakePty> {
        self.registry.lock().ptys.clone()
    }
    pub fn spawn_count(&self) -> usize {
        self.registry.lock().requests.len()
    }
    /// The spawn request number `n` (0-based).
    pub fn request(&self, n: usize) -> Option<PtySpawnRequest> {
        self.registry.lock().requests.get(n).cloned()
    }
    /// Make every following spawn fail with `message` (`None` to recover).
    pub fn set_fail(&self, message: Option<&str>) {
        self.registry.lock().fail_with = message.map(str::to_string);
    }
    pub fn env_of(&self, n: usize) -> HashMap<String, String> {
        self.request(n).map(|r| r.env).unwrap_or_default()
    }
}

impl PtyBackend for FakePtyBackend {
    fn spawn(&self, request: &PtySpawnRequest) -> Result<PtyProcess> {
        let mut reg = self.registry.lock();
        reg.requests.push(request.clone());
        if let Some(msg) = &reg.fail_with {
            return Err(anyhow!("{msg}"));
        }
        let (data_tx, data_rx) = channel();
        let (exit_tx, exit_rx) = channel();
        let pty = FakePty {
            pid: 1234 + u32::try_from(reg.ptys.len()).unwrap_or(0),
            writes: Arc::default(),
            data_tx: Arc::new(Mutex::new(Some(data_tx))),
            exit_tx,
            signals: Arc::default(),
            resizes: Arc::default(),
        };
        reg.ptys.push(pty.clone());
        Ok(PtyProcess {
            pid: Some(pty.pid),
            reader: Box::new(ChannelReader {
                rx: data_rx,
                pending: Vec::new(),
            }),
            writer: Box::new(VecWriter(pty.writes.clone())),
            control: Box::new(FakeControl {
                signals: pty.signals.clone(),
                resizes: pty.resizes.clone(),
            }),
            waiter: Box::new(FakeWaiter(exit_rx)),
        })
    }
}
