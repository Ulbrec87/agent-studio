//! `daemon.lock`: JSON `{ pid, version, socket, started_at }` written by the daemon once its
//! socket is bound, removed on exit. Liveness is decided by the pid **and** by connecting to the
//! socket (a crashed daemon leaves both files behind; a recycled pid must not fool us).

use std::time::Duration;

use anyhow::Result;
use as_core::paths::Paths;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LockData {
    pub pid: u32,
    pub version: String,
    pub socket: String,
    /// RFC 3339.
    pub started_at: String,
}

impl LockData {
    pub fn for_current_process(paths: &Paths) -> Self {
        Self {
            pid: std::process::id(),
            version: as_core::VERSION.to_string(),
            socket: paths.daemon_socket_name(),
            started_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        }
    }
}

/// Parse the lock file (no liveness check). Unreadable or malformed files read as `None`.
pub fn read(paths: &Paths) -> Option<LockData> {
    let raw = std::fs::read_to_string(paths.daemon_lock_file()).ok()?;
    let data: LockData = serde_json::from_str(&raw).ok()?;
    (data.pid != 0 && !data.socket.is_empty()).then_some(data)
}

pub fn write(paths: &Paths, data: &LockData) -> Result<()> {
    as_core::util::write_atomic(&paths.daemon_lock_file(), &serde_json::to_vec_pretty(data)?)
}

pub fn remove(paths: &Paths) {
    let _ = std::fs::remove_file(paths.daemon_lock_file());
}

/// Is a process with this pid running? (Windows: assumed true, the socket probe decides.)
pub fn pid_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        use nix::errno::Errno;
        use nix::sys::signal::kill;
        use nix::unistd::Pid;
        let Ok(raw) = i32::try_from(pid) else {
            return false;
        };
        match kill(Pid::from_raw(raw), None) {
            Ok(()) => true,
            Err(errno) => errno == Errno::EPERM,
        }
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        true
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Probe {
    /// A daemon answers on the socket (lock data when the lock file is readable).
    Live(Option<LockData>),
    /// Leftover lock and/or socket of a dead daemon.
    Stale,
    /// Nothing there.
    Absent,
}

pub async fn probe(paths: &Paths) -> Probe {
    let lock = read(paths);
    if let Some(l) = &lock
        && !pid_alive(l.pid)
    {
        return Probe::Stale;
    }
    let socket = lock
        .as_ref()
        .map_or_else(|| paths.daemon_socket_name(), |l| l.socket.clone());
    let connect = crate::transport::connect_name(&socket);
    match tokio::time::timeout(Duration::from_secs(1), connect).await {
        Ok(Ok(_)) => Probe::Live(lock),
        _ => {
            let socket_left = !cfg!(windows) && std::path::Path::new(&socket).exists();
            if lock.is_some() || socket_left || paths.daemon_lock_file().exists() {
                Probe::Stale
            } else {
                Probe::Absent
            }
        }
    }
}

/// Remove the lock file and the socket file of a dead daemon.
pub fn clear_stale(paths: &Paths) {
    remove(paths);
    crate::transport::remove_socket_file(paths);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_paths() -> (tempfile::TempDir, Paths) {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::at(dir.path());
        (dir, paths)
    }

    #[tokio::test]
    async fn nothing_there_is_absent() {
        let (_dir, paths) = temp_paths();
        assert_eq!(probe(&paths).await, Probe::Absent);
        assert_eq!(read(&paths), None);
    }

    #[tokio::test]
    async fn lock_round_trips_and_ignores_garbage() {
        let (_dir, paths) = temp_paths();
        let data = LockData::for_current_process(&paths);
        write(&paths, &data).unwrap();
        assert_eq!(read(&paths), Some(data));
        std::fs::write(paths.daemon_lock_file(), "{not json").unwrap();
        assert_eq!(read(&paths), None);
    }

    #[tokio::test]
    async fn live_pid_but_refusing_socket_is_stale() {
        let (_dir, paths) = temp_paths();
        // Our own pid is alive, but nobody listens on the socket (e.g. a recycled pid).
        write(&paths, &LockData::for_current_process(&paths)).unwrap();
        assert_eq!(probe(&paths).await, Probe::Stale);
        clear_stale(&paths);
        assert!(!paths.daemon_lock_file().exists());
        assert_eq!(probe(&paths).await, Probe::Absent);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn dead_pid_is_stale_and_cleanup_removes_both_files() {
        let (_dir, paths) = temp_paths();
        let mut child = std::process::Command::new("sh")
            .args(["-c", "exit 0"])
            .spawn()
            .unwrap();
        let pid = child.id();
        child.wait().unwrap();
        assert!(!pid_alive(pid));
        let mut data = LockData::for_current_process(&paths);
        data.pid = pid;
        write(&paths, &data).unwrap();
        std::fs::write(paths.daemon_socket_name(), "").unwrap(); // leftover socket file
        assert_eq!(probe(&paths).await, Probe::Stale);
        clear_stale(&paths);
        assert!(!paths.daemon_lock_file().exists());
        assert!(!std::path::Path::new(&paths.daemon_socket_name()).exists());
    }
}
