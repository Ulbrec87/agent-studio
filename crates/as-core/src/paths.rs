//! Filesystem locations used by Agent Studio.
//!
//! Everything lives under one root so a whole profile can be relocated with
//! `AGENT_STUDIO_HOME` (used by tests and for running a dev daemon next to a
//! real one). Without the override the OS config dir is used:
//! `~/.config/agent-studio` (Linux), `~/Library/Application Support/agent-studio`
//! (macOS), `%APPDATA%\agent-studio` (Windows).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

pub const APP_NAME: &str = "agent-studio";
pub const HOME_ENV: &str = "AGENT_STUDIO_HOME";

#[derive(Debug, Clone)]
pub struct Paths {
    root: PathBuf,
}

impl Paths {
    /// Resolve from `AGENT_STUDIO_HOME` or the OS config dir.
    pub fn resolve() -> Result<Self> {
        if let Some(home) = std::env::var_os(HOME_ENV).filter(|v| !v.is_empty()) {
            return Ok(Self::at(PathBuf::from(home)));
        }
        let dirs = directories::ProjectDirs::from("", "", APP_NAME)
            .context("cannot determine the user's config directory")?;
        Ok(Self::at(dirs.config_dir().to_path_buf()))
    }

    pub fn at(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn ensure(&self) -> Result<()> {
        for dir in [
            self.root.clone(),
            self.logs_dir(),
            self.headless_dir(),
            self.overview_history_dir(),
        ] {
            std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
        }
        Ok(())
    }

    pub fn config_file(&self) -> PathBuf {
        self.root.join("config.json")
    }
    /// Key bindings of the desktop app (`mod+x` accelerators).
    pub fn gui_keybindings_file(&self) -> PathBuf {
        self.root.join("gui-keybindings.json")
    }
    pub fn workspace_file(&self) -> PathBuf {
        self.root.join("workspace.json")
    }
    /// Layout of the free terminals per project (`terminals.json`).
    pub fn terminals_file(&self) -> PathBuf {
        self.root.join("terminals.json")
    }
    pub fn logs_dir(&self) -> PathBuf {
        self.root.join("logs")
    }
    pub fn headless_dir(&self) -> PathBuf {
        self.root.join("headless-runs")
    }
    pub fn overview_history_dir(&self) -> PathBuf {
        self.root.join("overview-history")
    }
    pub fn usage_cache_file(&self) -> PathBuf {
        self.root.join("usage-cache.json")
    }
    /// Usage alerts already sent (so a restart does not repeat them).
    pub fn usage_alerts_file(&self) -> PathBuf {
        self.root.join("usage-alerts.json")
    }
    pub fn manager_workspace_dir(&self) -> PathBuf {
        self.root.join("manager-workspace")
    }
    /// Lock file written by the running daemon (pid, version, socket name).
    pub fn daemon_lock_file(&self) -> PathBuf {
        self.root.join("daemon.lock")
    }
    /// Name/path of the daemon's local socket. On Unix a socket file inside
    /// the root; on Windows a named pipe derived from the root path.
    pub fn daemon_socket_name(&self) -> String {
        if cfg!(windows) {
            let mut hash: u64 = 0xcbf29ce484222325;
            for b in self.root.to_string_lossy().bytes() {
                hash ^= u64::from(b);
                hash = hash.wrapping_mul(0x100000001b3);
            }
            format!(r"\\.\pipe\agent-studio-{hash:016x}")
        } else {
            self.root.join("daemon.sock").to_string_lossy().into_owned()
        }
    }
}

/// The user's home directory (`$HOME` / `%USERPROFILE%`).
pub fn home_dir() -> PathBuf {
    directories::BaseDirs::new()
        .map(|d| d.home_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Expand a leading `~` to the home directory.
pub fn expand_tilde(path: &str) -> PathBuf {
    if path == "~" {
        return home_dir();
    }
    if let Some(rest) = path.strip_prefix("~/").or_else(|| path.strip_prefix("~\\")) {
        return home_dir().join(rest);
    }
    PathBuf::from(path)
}

/// Shorten a path for display by replacing the home directory with `~`.
pub fn tildify(path: &Path) -> String {
    let home = home_dir();
    match path.strip_prefix(&home) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".to_string(),
        Ok(rest) => format!("~{}{}", std::path::MAIN_SEPARATOR, rest.display()),
        Err(_) => path.display().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_live_under_root() {
        let p = Paths::at("/tmp/as-test");
        assert_eq!(p.config_file(), PathBuf::from("/tmp/as-test/config.json"));
        assert!(p.daemon_lock_file().starts_with("/tmp/as-test"));
    }

    #[test]
    fn tilde_round_trip() {
        let expanded = expand_tilde("~/projects/x");
        assert!(expanded.ends_with("projects/x"));
        assert!(tildify(&expanded).starts_with('~'));
    }
}
