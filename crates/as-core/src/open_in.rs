//! "Open in …": editors, file manager and terminal (port of Hydra's `EDITOR_REGISTRY` + open-in IPC).
//!
//! Command construction is pure ([`command_for`], per [`Os`]) so every platform is unit-tested;
//! [`launch`] spawns detached and never waits.

use std::path::Path;
use std::process::{Command, Stdio};

use serde::{Deserialize, Serialize};

use crate::agents::shell::Os;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "lowercase")]
pub enum OpenTarget {
    Vscode,
    Cursor,
    Windsurf,
    Antigravity,
    Zed,
    /// `$VISUAL` / `$EDITOR`. The TUI runs it in its own terminal (suspend/restore); elsewhere it falls back to a terminal window.
    Editor,
    /// Finder / Explorer / xdg-open.
    Files,
    Terminal,
}

impl OpenTarget {
    pub const ALL: [OpenTarget; 8] = [
        Self::Vscode,
        Self::Cursor,
        Self::Windsurf,
        Self::Antigravity,
        Self::Zed,
        Self::Editor,
        Self::Files,
        Self::Terminal,
    ];

    pub fn label(self, os: Os) -> &'static str {
        match self {
            Self::Vscode => "VS Code",
            Self::Cursor => "Cursor",
            Self::Windsurf => "Windsurf",
            Self::Antigravity => "Antigravity",
            Self::Zed => "Zed",
            Self::Editor => "$EDITOR (in a terminal)",
            Self::Files => match os {
                Os::MacOs => "Finder",
                Os::Windows => "Explorer",
                Os::Linux => "File manager",
            },
            Self::Terminal => "Terminal",
        }
    }

    /// CLI of the GUI editors.
    fn cli(self) -> Option<&'static str> {
        match self {
            Self::Vscode => Some("code"),
            Self::Cursor => Some("cursor"),
            Self::Windsurf => Some("windsurf"),
            Self::Antigravity => Some("antigravity"),
            Self::Zed => Some("zed"),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct OpenOption {
    pub target: OpenTarget,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenCommand {
    pub program: String,
    pub args: Vec<String>,
}

fn cmd(program: &str, args: &[&str]) -> OpenCommand {
    OpenCommand {
        program: program.into(),
        args: args.iter().map(|s| (*s).into()).collect(),
    }
}

/// Terminal emulators tried on Linux, in order, with the flag that sets the working directory.
const LINUX_TERMINALS: [(&str, &str); 6] = [
    ("x-terminal-emulator", ""),
    ("gnome-terminal", "--working-directory"),
    ("konsole", "--workdir"),
    ("alacritty", "--working-directory"),
    ("kitty", "--directory"),
    ("xterm", ""),
];

/// The terminal emulator command for `dir` on Linux. `terminal_env` is `$TERMINAL`; `available`
/// says which programs exist. Emulators without a cwd flag are launched with the process cwd set
/// by the caller (see [`launch`] `cwd`).
pub fn linux_terminal(
    dir: &str,
    terminal_env: Option<&str>,
    available: &dyn Fn(&str) -> bool,
) -> Option<OpenCommand> {
    if let Some(t) = terminal_env.filter(|t| !t.is_empty() && available(t)) {
        return Some(cmd(t, &[]));
    }
    LINUX_TERMINALS
        .iter()
        .find(|(p, _)| available(p))
        .map(|(p, flag)| {
            if flag.is_empty() {
                cmd(p, &[])
            } else if matches!(*p, "kitty" | "konsole") {
                cmd(p, &[flag, dir])
            } else {
                cmd(p, &[&format!("{flag}={dir}")])
            }
        })
}

/// Command that opens `path` with `target` on `os`. `None` when the target is not available there
/// (a Linux terminal emulator could not be found).
pub fn command_for(
    target: OpenTarget,
    path: &str,
    os: Os,
    available: &dyn Fn(&str) -> bool,
    terminal_env: Option<&str>,
) -> Option<OpenCommand> {
    if let Some(cli) = target.cli() {
        return Some(cmd(cli, &[path]));
    }
    match (target, os) {
        (OpenTarget::Files, Os::MacOs) => Some(cmd("open", &[path])),
        (OpenTarget::Files, Os::Windows) => Some(cmd("explorer", &[path])),
        (OpenTarget::Files, Os::Linux) => Some(cmd("xdg-open", &[path])),
        (OpenTarget::Terminal | OpenTarget::Editor, Os::MacOs) => {
            Some(cmd("open", &["-a", "Terminal", path]))
        }
        (OpenTarget::Terminal | OpenTarget::Editor, Os::Windows) => {
            // `start "" /D <dir> cmd` opens a new console window in the directory.
            Some(cmd("cmd", &["/c", "start", "", "/D", path, "cmd"]))
        }
        (OpenTarget::Terminal | OpenTarget::Editor, Os::Linux) => {
            linux_terminal(path, terminal_env, available)
        }
        _ => None,
    }
}

/// Targets that look usable on this machine: GUI editors found on PATH (or as macOS app bundles)
/// plus file manager and terminal.
pub fn detect_installed(os: Os) -> Vec<OpenOption> {
    let on_path = |p: &str| which::which(p).is_ok();
    let mac_app = |app: &str| os == Os::MacOs && Path::new("/Applications").join(app).exists();
    OpenTarget::ALL
        .into_iter()
        .filter(|t| match t {
            OpenTarget::Vscode => on_path("code") || mac_app("Visual Studio Code.app"),
            OpenTarget::Cursor => on_path("cursor") || mac_app("Cursor.app"),
            OpenTarget::Windsurf => on_path("windsurf") || mac_app("Windsurf.app"),
            OpenTarget::Antigravity => on_path("antigravity") || mac_app("Antigravity.app"),
            OpenTarget::Zed => on_path("zed") || mac_app("Zed.app"),
            OpenTarget::Editor => std::env::var_os("VISUAL")
                .or_else(|| std::env::var_os("EDITOR"))
                .is_some_and(|v| !v.is_empty()),
            OpenTarget::Files => true,
            OpenTarget::Terminal => {
                os != Os::Linux
                    || command_for(
                        OpenTarget::Terminal,
                        ".",
                        os,
                        &on_path,
                        std::env::var("TERMINAL").ok().as_deref(),
                    )
                    .is_some()
            }
        })
        .map(|t| OpenOption {
            target: t,
            label: t.label(os).to_string(),
        })
        .collect()
}

/// Open `path` with `target` on this machine (detached; the child is reaped on a thread).
pub fn open(target: OpenTarget, path: &str) -> Result<(), String> {
    let os = Os::current();
    let available = |p: &str| which::which(p).is_ok();
    let terminal = std::env::var("TERMINAL").ok();
    let command = command_for(target, path, os, &available, terminal.as_deref())
        .ok_or_else(|| format!("{} is not available on this system", target.label(os)))?;
    // A terminal/explorer opened for a file should show its folder.
    let dir = Path::new(path);
    let cwd = if dir.is_dir() {
        Some(dir)
    } else {
        dir.parent()
    };
    let cwd = matches!(target, OpenTarget::Terminal | OpenTarget::Editor)
        .then_some(cwd)
        .flatten();
    launch(&command, cwd)
}

pub fn launch(command: &OpenCommand, cwd: Option<&Path>) -> Result<(), String> {
    let mut c = Command::new(&command.program);
    c.args(&command.args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if let Some(d) = cwd {
        c.current_dir(d);
    }
    let mut child = c
        .spawn()
        .map_err(|e| format!("cannot start {}: {e}", command.program))?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all(_: &str) -> bool {
        true
    }
    fn none(_: &str) -> bool {
        false
    }

    #[test]
    fn editors_use_their_cli_everywhere() {
        for os in [Os::Linux, Os::MacOs, Os::Windows] {
            let c = command_for(OpenTarget::Vscode, "/p", os, &all, None).unwrap();
            assert_eq!(
                (c.program.as_str(), c.args),
                ("code", vec!["/p".to_string()])
            );
            assert_eq!(
                command_for(OpenTarget::Zed, "/p", os, &all, None)
                    .unwrap()
                    .program,
                "zed"
            );
        }
    }

    #[test]
    fn file_manager_per_os() {
        let prog = |os| {
            command_for(OpenTarget::Files, "/p", os, &all, None)
                .unwrap()
                .program
        };
        assert_eq!(prog(Os::Linux), "xdg-open");
        assert_eq!(prog(Os::MacOs), "open");
        assert_eq!(prog(Os::Windows), "explorer");
    }

    #[test]
    fn terminal_per_os() {
        let mac = command_for(OpenTarget::Terminal, "/p", Os::MacOs, &all, None).unwrap();
        assert_eq!(
            (mac.program.as_str(), mac.args),
            (
                "open",
                vec!["-a".to_string(), "Terminal".into(), "/p".into()]
            )
        );
        let win = command_for(OpenTarget::Terminal, "C:\\p", Os::Windows, &all, None).unwrap();
        assert_eq!(win.program, "cmd");
        assert!(win.args.contains(&"C:\\p".to_string()) && win.args.contains(&"/D".to_string()));
    }

    #[test]
    fn linux_terminal_selection() {
        let only = |name: &'static str| move |p: &str| p == name;
        assert_eq!(
            command_for(OpenTarget::Terminal, "/p", Os::Linux, &none, None),
            None
        );
        assert_eq!(
            command_for(
                OpenTarget::Terminal,
                "/p",
                Os::Linux,
                &only("konsole"),
                None
            )
            .unwrap()
            .args,
            ["--workdir", "/p"]
        );
        assert_eq!(
            command_for(
                OpenTarget::Terminal,
                "/p",
                Os::Linux,
                &only("gnome-terminal"),
                None
            )
            .unwrap()
            .args,
            ["--working-directory=/p"]
        );
        assert_eq!(
            command_for(OpenTarget::Terminal, "/p", Os::Linux, &only("kitty"), None)
                .unwrap()
                .args,
            ["--directory", "/p"]
        );
        assert!(
            command_for(OpenTarget::Terminal, "/p", Os::Linux, &only("xterm"), None)
                .unwrap()
                .args
                .is_empty()
        );
        // $TERMINAL wins when it exists.
        let c = command_for(OpenTarget::Terminal, "/p", Os::Linux, &all, Some("foot")).unwrap();
        assert_eq!(c.program, "foot");
    }

    #[test]
    fn labels_follow_os() {
        assert_eq!(OpenTarget::Files.label(Os::MacOs), "Finder");
        assert_eq!(OpenTarget::Files.label(Os::Windows), "Explorer");
        assert!(
            detect_installed(Os::current())
                .iter()
                .any(|o| o.target == OpenTarget::Files)
        );
    }
}
