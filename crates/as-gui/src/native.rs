//! Native actions the shell runs itself (no Tauri types): "open in ..." and the list of usable editors.
//!
//! They run in the desktop process, not in the daemon: a daemon started by systemd or launchd outside the graphical session has no
//! display and cannot start GUI editors or a file manager. The browser dev bridge has no such shell and keeps asking the daemon.

use as_core::agents::shell::Os;
use as_core::open_in::{self, OpenOption, OpenTarget};

/// Open `path` with `target` (the lowercase names of [`OpenTarget`]: `vscode`, `files`, ...). Blocking: call it from a blocking thread.
pub fn open_in(target: &str, path: &str) -> Result<(), String> {
    let target = parse_target(target)?;
    open_in::open(target, path)
}

/// Editors, file manager and terminal that look usable here. Blocking (probes `PATH`).
pub fn list_open_targets() -> Vec<OpenOption> {
    open_in::detect_installed(Os::current())
}

fn parse_target(target: &str) -> Result<OpenTarget, String> {
    serde_json::from_value(serde_json::Value::String(target.to_string()))
        .map_err(|_| format!("unknown open target: {target}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn targets_use_the_daemons_names() {
        assert_eq!(parse_target("vscode"), Ok(OpenTarget::Vscode));
        assert_eq!(parse_target("files"), Ok(OpenTarget::Files));
        assert_eq!(parse_target("terminal"), Ok(OpenTarget::Terminal));
        assert!(parse_target("emacs").is_err());
        assert!(parse_target("").is_err());
    }

    #[test]
    fn an_unknown_target_is_an_error_before_anything_starts() {
        assert!(open_in("nope", "/tmp").unwrap_err().contains("unknown"));
    }

    #[test]
    fn the_list_only_names_known_targets() {
        // Whatever this machine has, every entry is a target the front end can ask for again.
        for option in list_open_targets() {
            assert!(OpenTarget::ALL.contains(&option.target));
        }
    }
}
