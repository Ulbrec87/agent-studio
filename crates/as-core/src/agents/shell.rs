//! Shell wrapping (Hydra `electron/agents/shellWrapper.ts`) and the login-shell
//! PATH lookup (Hydra `electron/util/fix-path.ts`).

use std::collections::HashMap;
use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::types::{AppConfig, TerminalShellMode};

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ShellConfig {
    pub mode: TerminalShellMode,
    /// Absolute path to a custom shell. Only used when `mode == Custom`.
    pub path: String,
    /// Whitespace-separated shell args, e.g. `-lc`. Empty means the defaults for the chosen shell.
    pub args: String,
}

impl ShellConfig {
    pub fn from_config(config: &AppConfig) -> Self {
        Self {
            mode: config.terminal_shell_mode,
            path: config.terminal_shell_path.clone(),
            args: config.terminal_shell_args.clone(),
        }
    }

    /// Spawn the CLI directly (no wrapping).
    pub fn direct() -> Self {
        Self {
            mode: TerminalShellMode::Direct,
            ..Default::default()
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpawnSpec {
    pub cmd: String,
    pub args: Vec<String>,
}

/// Platform family, injectable so the wrapper is testable for every OS.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    Windows,
    MacOs,
    Linux,
}

impl Os {
    pub fn current() -> Self {
        if cfg!(windows) {
            Os::Windows
        } else if cfg!(target_os = "macos") {
            Os::MacOs
        } else {
            Os::Linux
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Style {
    Posix,
    Pwsh,
}

/// Quote one argv element for POSIX sh: single quotes, embedded quotes escaped.
fn sh_posix_quote(value: &str) -> String {
    if value.is_empty() {
        return "''".into();
    }
    let safe = value
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "_@%+=:,./-".contains(c));
    if safe {
        return value.to_string();
    }
    format!("'{}'", value.replace('\'', r"'\''"))
}

/// Quote one argv element for PowerShell (single-quoted literal).
fn pwsh_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

/// Whitespace-split shell args. Intentionally simple: users who need quoting
/// should use `custom` mode with a wrapper script.
fn split_args(input: &str) -> Vec<String> {
    input.split_whitespace().map(str::to_string).collect()
}

fn command_string(style: Style, spec: &SpawnSpec) -> String {
    match style {
        Style::Posix => std::iter::once(spec.cmd.as_str())
            .chain(spec.args.iter().map(String::as_str))
            .map(sh_posix_quote)
            .collect::<Vec<_>>()
            .join(" "),
        // `& 'cmd' 'arg1' 'arg2'`: call operator so paths with spaces work.
        Style::Pwsh => {
            let args: Vec<String> = spec.args.iter().map(|a| pwsh_quote(a)).collect();
            format!("& {} {}", pwsh_quote(&spec.cmd), args.join(" "))
        }
    }
}

fn default_login_shell(os: Os, env: &HashMap<String, String>) -> (String, Vec<String>, Style) {
    let non_empty = |k: &str| env.get(k).filter(|v| !v.is_empty()).cloned();
    if os == Os::Windows {
        let shell = non_empty("PWSH").unwrap_or_else(|| "powershell.exe".into());
        return (
            shell,
            vec!["-NoLogo".into(), "-NoProfile".into(), "-Command".into()],
            Style::Pwsh,
        );
    }
    (
        non_empty("SHELL").unwrap_or_else(|| "/bin/bash".into()),
        vec!["-lc".into()],
        Style::Posix,
    )
}

/// Wrap a raw command through a user-selectable shell so login rc files (PATH,
/// nvm, rbenv, ...) are picked up. Fixes "command not found" on distros that
/// populate PATH via the shell profile rather than the system environment.
pub fn wrap_with_shell(
    spec: &SpawnSpec,
    config: &ShellConfig,
    os: Os,
    env: &HashMap<String, String>,
) -> SpawnSpec {
    let mode = match config.mode {
        TerminalShellMode::Auto => {
            if os == Os::Windows {
                TerminalShellMode::Direct
            } else {
                TerminalShellMode::Login
            }
        }
        other => other,
    };
    match mode {
        TerminalShellMode::Direct | TerminalShellMode::Auto => spec.clone(),
        TerminalShellMode::Custom => {
            if config.path.is_empty() {
                return spec.clone();
            }
            let style = if os == Os::Windows {
                Style::Pwsh
            } else {
                Style::Posix
            };
            let default_args = if style == Style::Pwsh {
                "-Command"
            } else {
                "-lc"
            };
            let shell_args = split_args(if config.args.is_empty() {
                default_args
            } else {
                &config.args
            });
            let mut args = shell_args;
            args.push(command_string(style, spec));
            SpawnSpec {
                cmd: config.path.clone(),
                args,
            }
        }
        TerminalShellMode::Login => {
            let (cmd, default_args, style) = default_login_shell(os, env);
            let mut args = if config.args.is_empty() {
                default_args
            } else {
                split_args(&config.args)
            };
            args.push(command_string(style, spec));
            SpawnSpec { cmd, args }
        }
    }
}

/// [`wrap_with_shell`] for the current OS and process environment.
pub fn wrap_with_shell_current(spec: &SpawnSpec, config: &ShellConfig) -> SpawnSpec {
    wrap_with_shell(spec, config, Os::current(), &super::env::process_env())
}

// ── PATH fixing (fix-path.ts) ───────────────────────────────────────────────

const PATH_MARKER: &str = "__PATH__=";
const LOGIN_SHELL_TIMEOUT: Duration = Duration::from_secs(5);

/// Extract the PATH printed after [`PATH_MARKER`]; rc files may print noise before it.
pub fn parse_path_marker(output: &str) -> Option<String> {
    output
        .lines()
        .find_map(|line| {
            line.find(PATH_MARKER)
                .map(|i| line[i + PATH_MARKER.len()..].trim().to_string())
        })
        .filter(|p| !p.is_empty())
}

/// Ask an interactive login shell for its `$PATH` (`<shell> -ilc 'echo -n __PATH__=$PATH'`, 5 s
/// timeout). Returns `None` on Windows, on failure or on timeout.
pub fn login_shell_path(shell: &str) -> Option<String> {
    login_shell_path_with_timeout(shell, LOGIN_SHELL_TIMEOUT)
}

pub fn login_shell_path_with_timeout(shell: &str, timeout: Duration) -> Option<String> {
    if cfg!(windows) {
        return None;
    }
    let mut child = Command::new(shell)
        .args(["-ilc", "echo \"__PATH__=$PATH\""])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    // Drain on a thread so a chatty rc file can never fill the pipe and block the shell.
    let reader = std::thread::spawn(move || {
        let mut buf = String::new();
        let _ = stdout.read_to_string(&mut buf);
        buf
    });
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break;
            }
        }
    }
    parse_path_marker(&reader.join().ok()?)
}

/// Append entries that are not already in `current` (order kept, empty entries dropped).
pub fn append_path_entries(current: &str, entries: &[String], delimiter: char) -> String {
    let mut merged: Vec<String> = Vec::new();
    for e in current.split(delimiter).filter(|e| !e.is_empty()) {
        if !merged.iter().any(|m| m == e) {
            merged.push(e.to_string());
        }
    }
    for e in entries.iter().filter(|e| !e.is_empty()) {
        if !merged.iter().any(|m| m == e) {
            merged.push(e.clone());
        }
    }
    merged.join(&delimiter.to_string())
}

fn windows_extra_path_entries(env: &HashMap<String, String>) -> Vec<String> {
    let join = |base: Option<&String>, rest: &[&str]| -> Option<String> {
        let mut p = std::path::PathBuf::from(base?);
        p.extend(rest);
        Some(p.to_string_lossy().into_owned())
    };
    let home = env.get("USERPROFILE").or_else(|| env.get("HOME"));
    let appdata = env.get("APPDATA");
    let local = env.get("LOCALAPPDATA");
    [
        join(appdata, &["npm"]),
        join(local, &["Programs", "nodejs"]),
        join(home, &["scoop", "shims"]),
        join(home, &[".bun", "bin"]),
        join(home, &[".cargo", "bin"]),
        join(home, &[".local", "bin"]),
        join(local, &["fnm_multishells"]),
        Some(r"C:\ProgramData\chocolatey\bin".to_string()),
        join(local, &["Microsoft", "WinGet", "Links"]),
        Some(r"C:\Program Files\nodejs".to_string()),
    ]
    .into_iter()
    .flatten()
    .collect()
}

fn unix_fallback_path_entries(env: &HashMap<String, String>) -> Vec<String> {
    let mut v = vec![
        "/usr/local/bin".to_string(),
        "/opt/homebrew/bin".to_string(),
    ];
    if let Some(home) = env.get("HOME") {
        v.extend(
            [".local/bin", ".bun/bin", ".nvm/versions/node/current/bin"]
                .map(|p| format!("{home}/{p}")),
        );
    }
    v
}

/// The PATH the daemon should hand to agents so installed CLIs are found even
/// when it was started from a GUI/launcher with a minimal PATH. Port of Hydra
/// `fixPath`, but it returns the value instead of mutating the process env
/// (`set_var` is unsafe in edition 2024): put it into agent env overrides.
///
/// * Windows: current PATH + common global npm/Bun/Scoop/Cargo/... locations.
/// * macOS: PATH of the user's login shell, else current PATH + common bin dirs.
/// * Linux: PATH of the login shell when obtainable, else `None` (leave PATH alone;
///   `auto` shell mode already runs agents through the login shell).
pub fn fixed_path(env: &HashMap<String, String>, os: Os) -> Option<String> {
    let current = env
        .get("PATH")
        .or_else(|| env.get("Path"))
        .cloned()
        .unwrap_or_default();
    match os {
        Os::Windows => Some(append_path_entries(
            &current,
            &windows_extra_path_entries(env),
            ';',
        )),
        Os::MacOs => {
            let shell = env
                .get("SHELL")
                .filter(|s| !s.is_empty())
                .map_or("/bin/zsh", String::as_str);
            login_shell_path(shell).or_else(|| {
                Some(append_path_entries(
                    &current,
                    &unix_fallback_path_entries(env),
                    ':',
                ))
            })
        }
        Os::Linux => {
            let shell = env.get("SHELL").filter(|s| !s.is_empty())?;
            login_shell_path(shell)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn spec() -> SpawnSpec {
        SpawnSpec {
            cmd: "/usr/local/bin/claude".into(),
            args: vec!["--yolo".into(), "hello world".into()],
        }
    }
    fn cfg(mode: TerminalShellMode, path: &str, args: &str) -> ShellConfig {
        ShellConfig {
            mode,
            path: path.into(),
            args: args.into(),
        }
    }
    fn env(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    #[test]
    fn direct_mode_passes_the_command_through_unchanged() {
        let out = wrap_with_shell(
            &spec(),
            &cfg(TerminalShellMode::Direct, "", ""),
            Os::MacOs,
            &env(&[]),
        );
        assert_eq!(out, spec());
    }

    #[test]
    fn auto_mode_on_posix_wraps_through_the_user_login_shell() {
        let out = wrap_with_shell(
            &spec(),
            &cfg(TerminalShellMode::Auto, "", ""),
            Os::Linux,
            &env(&[("SHELL", "/bin/zsh")]),
        );
        assert_eq!(out.cmd, "/bin/zsh");
        assert_eq!(out.args[0], "-lc");
        assert!(out.args[1].contains("/usr/local/bin/claude"));
        assert!(out.args[1].contains("'hello world'"));
    }

    #[test]
    fn auto_mode_on_windows_stays_direct() {
        let out = wrap_with_shell(
            &spec(),
            &cfg(TerminalShellMode::Auto, "", ""),
            Os::Windows,
            &env(&[]),
        );
        assert_eq!(out, spec());
    }

    #[test]
    fn login_mode_falls_back_to_bin_bash_when_shell_is_unset() {
        let out = wrap_with_shell(
            &spec(),
            &cfg(TerminalShellMode::Login, "", ""),
            Os::Linux,
            &env(&[]),
        );
        assert_eq!(out.cmd, "/bin/bash");
        assert_eq!(out.args[0], "-lc");
    }

    #[test]
    fn login_mode_on_windows_uses_powershell_with_command() {
        let out = wrap_with_shell(
            &spec(),
            &cfg(TerminalShellMode::Login, "", ""),
            Os::Windows,
            &env(&[]),
        );
        assert!(out.cmd.to_lowercase().contains("powershell"));
        assert!(out.args.contains(&"-Command".to_string()));
        assert!(out.args.join(" ").contains("'/usr/local/bin/claude'"));
    }

    #[test]
    fn custom_mode_uses_the_provided_shell_path_and_args() {
        let out = wrap_with_shell(
            &spec(),
            &cfg(TerminalShellMode::Custom, "/usr/bin/fish", "-lc"),
            Os::Linux,
            &env(&[]),
        );
        assert_eq!(out.cmd, "/usr/bin/fish");
        assert_eq!(out.args[0], "-lc");
        assert!(out.args[1].contains("/usr/local/bin/claude"));
    }

    #[test]
    fn custom_mode_with_empty_path_is_ignored() {
        let out = wrap_with_shell(
            &spec(),
            &cfg(TerminalShellMode::Custom, "", ""),
            Os::Linux,
            &env(&[]),
        );
        assert_eq!(out, spec());
    }

    #[test]
    fn quotes_args_containing_single_quotes_safely_for_posix() {
        let tricky = SpawnSpec {
            cmd: "echo".into(),
            args: vec!["it's tricky".into()],
        };
        let out = wrap_with_shell(
            &tricky,
            &cfg(TerminalShellMode::Login, "", ""),
            Os::Linux,
            &env(&[("SHELL", "/bin/bash")]),
        );
        assert!(out.args[1].contains(r"'it'\''s tricky'"));
    }

    #[test]
    fn quoting_edge_cases() {
        assert_eq!(sh_posix_quote(""), "''");
        assert_eq!(
            sh_posix_quote("a-b_c.d/e:f=g,h@i%j+k"),
            "a-b_c.d/e:f=g,h@i%j+k"
        );
        assert_eq!(sh_posix_quote("a b"), "'a b'");
        assert_eq!(pwsh_quote("it's"), "'it''s'");
    }

    #[test]
    fn parses_the_path_marker_ignoring_rc_noise() {
        assert_eq!(
            parse_path_marker("welcome!\n__PATH__=/a:/b\n"),
            Some("/a:/b".into())
        );
        assert_eq!(parse_path_marker("nothing"), None);
        assert_eq!(parse_path_marker("__PATH__=\n"), None);
    }

    #[test]
    fn appends_only_missing_path_entries() {
        let out = append_path_entries("/a::/b", &["/b".into(), "/c".into(), String::new()], ':');
        assert_eq!(out, "/a:/b:/c");
    }

    #[test]
    fn windows_fixed_path_appends_common_locations() {
        let e = env(&[
            ("PATH", r"C:\Windows"),
            ("USERPROFILE", r"C:\Users\me"),
            ("APPDATA", r"C:\Users\me\AppData\Roaming"),
        ]);
        let p = fixed_path(&e, Os::Windows).unwrap();
        assert!(p.starts_with(r"C:\Windows;"));
        assert!(p.contains("chocolatey"));
        assert!(p.contains("npm"));
    }

    #[cfg(unix)]
    #[test]
    fn login_shell_path_runs_a_real_shell() {
        let p = login_shell_path("/bin/sh").expect("sh prints its PATH");
        assert!(!p.is_empty());
        assert_eq!(login_shell_path("/definitely/not/a/shell"), None);
    }
}
