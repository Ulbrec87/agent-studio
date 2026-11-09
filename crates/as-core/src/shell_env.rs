//! Repairing `PATH` for processes that were not started from a login shell (port of Hydra's `electron/util/fix-path.ts`).
//!
//! A daemon started by a systemd user unit, or an app started from a desktop launcher / the Dock, inherits a minimal `PATH`
//! without `~/.local/bin`, nvm, bun, cargo or npm-global directories, so `which claude` fails although it works in a terminal.
//!
//! - Unix: ask the user's login shell (`$SHELL -ilc`) for its `PATH`, merge it with ours (login entries first) and append the
//!   usual install locations that are still missing. Hydra did this on macOS only; Linux needs it just as much.
//! - Windows: append the usual global npm / Bun / Node / Scoop / Cargo locations (Hydra's list).
//!
//! The parsing and merging are pure functions with unit tests; [`fix_path`] is the only part with side effects.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

/// Set this environment variable (to anything) to leave `PATH` alone.
pub const SKIP_ENV: &str = "AGENT_STUDIO_NO_FIX_PATH";

/// How long the login shell may take (rc files can be slow or even interactive).
const SHELL_TIMEOUT: Duration = Duration::from_secs(5);
const MARKER_START: &str = "__AS_PATH__=";
const MARKER_END: &str = "__END__";

/// The operating systems whose fallback list differs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Linux,
    Macos,
    Windows,
}

impl Platform {
    pub fn current() -> Self {
        if cfg!(windows) {
            Self::Windows
        } else if cfg!(target_os = "macos") {
            Self::Macos
        } else {
            Self::Linux
        }
    }

    fn separator(self) -> char {
        if self == Self::Windows { ';' } else { ':' }
    }
}

/// The environment values the fallback list depends on.
#[derive(Debug, Clone, Default)]
pub struct FallbackEnv {
    pub home: Option<PathBuf>,
    /// `%APPDATA%` (Windows).
    pub appdata: Option<PathBuf>,
    /// `%LOCALAPPDATA%` (Windows).
    pub local_appdata: Option<PathBuf>,
}

impl FallbackEnv {
    pub fn from_process(platform: Platform) -> Self {
        let var = |name: &str| std::env::var_os(name).filter(|v| !v.is_empty());
        let home = if platform == Platform::Windows {
            var("USERPROFILE").or_else(|| var("HOME"))
        } else {
            var("HOME")
        };
        Self {
            home: home.map(PathBuf::from),
            appdata: var("APPDATA").map(PathBuf::from),
            local_appdata: var("LOCALAPPDATA").map(PathBuf::from),
        }
    }
}

/// The `PATH` value printed by the login shell: the text between the markers. Rc files may print anything around it, and
/// the marker text can appear more than once (e.g. a prompt echo), so the *last* start marker that is followed by an end marker wins.
pub fn parse_marked_path(output: &str) -> Option<String> {
    let mut found = None;
    let mut rest = output;
    while let Some(start) = rest.find(MARKER_START) {
        let after = &rest[start + MARKER_START.len()..];
        if let Some(end) = after.find(MARKER_END) {
            let value = after[..end].trim();
            if !value.is_empty() {
                found = Some(value.to_string());
            }
            rest = &after[end + MARKER_END.len()..];
        } else {
            break;
        }
    }
    found
}

/// Entries of `login` first, then those of `current`, then `fallbacks`; empty entries and duplicates are dropped
/// (the first occurrence keeps its position). `separator` is the PATH separator of the target platform.
pub fn merge_paths(login: &str, current: &str, fallbacks: &[PathBuf], separator: char) -> String {
    let mut merged: Vec<String> = Vec::new();
    let mut push = |entry: &str| {
        if !entry.is_empty() && !merged.iter().any(|seen| seen == entry) {
            merged.push(entry.to_string());
        }
    };
    login.split(separator).for_each(&mut push);
    current.split(separator).for_each(&mut push);
    for fallback in fallbacks {
        push(&fallback.to_string_lossy());
    }
    merged.join(&separator.to_string())
}

/// The install locations of CLI tools that are appended when missing.
pub fn fallback_dirs(platform: Platform, env: &FallbackEnv) -> Vec<PathBuf> {
    let home = env.home.as_deref();
    let under = |base: Option<&Path>, parts: &[&str]| -> Option<PathBuf> {
        base.map(|base| {
            parts
                .iter()
                .fold(base.to_path_buf(), |acc, part| acc.join(part))
        })
    };
    let mut dirs: Vec<Option<PathBuf>> = Vec::new();
    match platform {
        Platform::Windows => {
            dirs.push(under(env.appdata.as_deref(), &["npm"]));
            dirs.push(under(env.local_appdata.as_deref(), &["Programs", "nodejs"]));
            dirs.push(under(home, &["scoop", "shims"]));
            dirs.push(under(home, &[".bun", "bin"]));
            dirs.push(under(home, &[".cargo", "bin"]));
            dirs.push(under(home, &[".local", "bin"]));
            dirs.push(under(env.local_appdata.as_deref(), &["fnm_multishells"]));
            dirs.push(Some(PathBuf::from(r"C:\ProgramData\chocolatey\bin")));
            dirs.push(under(
                env.local_appdata.as_deref(),
                &["Microsoft", "WinGet", "Links"],
            ));
            dirs.push(Some(PathBuf::from(r"C:\Program Files\nodejs")));
        }
        Platform::Linux | Platform::Macos => {
            dirs.push(under(home, &[".local", "bin"]));
            dirs.push(under(home, &[".bun", "bin"]));
            dirs.push(under(home, &[".cargo", "bin"]));
            dirs.push(under(home, &[".npm-global", "bin"]));
            dirs.push(Some(PathBuf::from("/usr/local/bin")));
            if platform == Platform::Macos {
                dirs.push(Some(PathBuf::from("/opt/homebrew/bin")));
            } else {
                dirs.push(Some(PathBuf::from("/snap/bin")));
            }
        }
    }
    dirs.into_iter().flatten().collect()
}

/// The shell to ask: `$SHELL`, else `/bin/zsh` on macOS and `/bin/sh` elsewhere.
pub fn login_shell(shell_env: Option<&str>, platform: Platform) -> String {
    match shell_env.map(str::trim).filter(|s| !s.is_empty()) {
        Some(shell) => shell.to_string(),
        None if platform == Platform::Macos => "/bin/zsh".to_string(),
        None => "/bin/sh".to_string(),
    }
}

/// Run `shell -ilc 'printf ...'` and return what it printed (`None` on failure or timeout).
fn read_login_shell_path(shell: &str) -> Option<String> {
    let script = format!("printf '{MARKER_START}%s{MARKER_END}' \"$PATH\"");
    let mut child = Command::new(shell)
        .arg("-ilc")
        .arg(script)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        use std::io::Read;
        let mut bytes = Vec::new();
        let _ = stdout.read_to_end(&mut bytes);
        let _ = tx.send(bytes);
    });
    match rx.recv_timeout(SHELL_TIMEOUT) {
        Ok(bytes) => {
            let _ = child.wait();
            parse_marked_path(&String::from_utf8_lossy(&bytes))
        }
        Err(_) => {
            // Hung rc file: kill the shell (that also ends the reader thread) and fall back to the fixed list.
            let _ = child.kill();
            let _ = child.wait();
            None
        }
    }
}

/// Repair this process's `PATH`; does nothing when [`SKIP_ENV`] is set. Never fails: whatever cannot be determined is skipped.
///
/// # Safety argument
///
/// `std::env::set_var` is `unsafe` in edition 2024 because another thread may read the environment concurrently. Call this
/// **first thing in `main`**, before a tokio runtime, a Tauri builder or any other thread exists. The only thread it starts
/// itself (the reader of the shell's stdout) never touches the environment and has finished or been unblocked by the
/// `kill` before `set_var` runs.
#[allow(unsafe_code)]
pub fn fix_path() {
    if std::env::var_os(SKIP_ENV).is_some() {
        return;
    }
    let platform = Platform::current();
    let current = std::env::var("PATH").unwrap_or_default();
    let login = if platform == Platform::Windows {
        None
    } else {
        let shell = login_shell(std::env::var("SHELL").ok().as_deref(), platform);
        read_login_shell_path(&shell)
    };
    let fallbacks = fallback_dirs(platform, &FallbackEnv::from_process(platform));
    let merged = merge_paths(
        login.as_deref().unwrap_or(""),
        &current,
        &fallbacks,
        platform.separator(),
    );
    if merged != current {
        // SAFETY: see the function documentation: single-threaded start of `main`.
        unsafe { std::env::set_var("PATH", merged) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_value_between_markers() {
        assert_eq!(
            parse_marked_path("__AS_PATH__=/a:/b__END__").as_deref(),
            Some("/a:/b")
        );
    }

    #[test]
    fn ignores_junk_printed_by_rc_files() {
        let out = "Welcome!\nnvm: loaded\n__AS_PATH__=/a:/b__END__\ntrailing junk\n";
        assert_eq!(parse_marked_path(out).as_deref(), Some("/a:/b"));
    }

    #[test]
    fn last_complete_marker_wins_and_incomplete_is_ignored() {
        let out = "__AS_PATH__=/old__END__ x __AS_PATH__=/new__END__ __AS_PATH__=/cut";
        assert_eq!(parse_marked_path(out).as_deref(), Some("/new"));
        assert_eq!(parse_marked_path("__AS_PATH__=/a"), None);
        assert_eq!(parse_marked_path("nothing"), None);
        assert_eq!(parse_marked_path("__AS_PATH__=__END__"), None);
    }

    #[test]
    fn merge_puts_login_first_without_duplicates() {
        let fallbacks = [PathBuf::from("/x"), PathBuf::from("/b")];
        assert_eq!(
            merge_paths("/a:/b::/c", "/c:/d:/a", &fallbacks, ':'),
            "/a:/b:/c:/d:/x"
        );
    }

    #[test]
    fn merge_handles_empty_inputs_and_windows_separator() {
        assert_eq!(merge_paths("", "", &[], ':'), "");
        assert_eq!(merge_paths("", "/a", &[], ':'), "/a");
        assert_eq!(
            merge_paths("", r"C:\a;C:\b", &[PathBuf::from(r"C:\a")], ';'),
            r"C:\a;C:\b"
        );
    }

    #[test]
    fn linux_and_macos_fallbacks() {
        let env = FallbackEnv {
            home: Some(PathBuf::from("/home/u")),
            ..Default::default()
        };
        let linux = fallback_dirs(Platform::Linux, &env);
        for dir in [
            "/home/u/.local/bin",
            "/home/u/.bun/bin",
            "/home/u/.cargo/bin",
            "/home/u/.npm-global/bin",
            "/usr/local/bin",
            "/snap/bin",
        ] {
            assert!(linux.contains(&PathBuf::from(dir)), "{dir} in {linux:?}");
        }
        assert!(!linux.contains(&PathBuf::from("/opt/homebrew/bin")));
        let mac = fallback_dirs(Platform::Macos, &env);
        assert!(mac.contains(&PathBuf::from("/opt/homebrew/bin")));
        assert!(!mac.contains(&PathBuf::from("/snap/bin")));
        // Without a home only the absolute entries remain.
        let none = fallback_dirs(Platform::Linux, &FallbackEnv::default());
        assert_eq!(
            none,
            vec![PathBuf::from("/usr/local/bin"), PathBuf::from("/snap/bin")]
        );
    }

    #[test]
    fn windows_fallbacks_follow_hydra() {
        let env = FallbackEnv {
            home: Some(PathBuf::from("home")),
            appdata: Some(PathBuf::from("appdata")),
            local_appdata: Some(PathBuf::from("local")),
        };
        let dirs = fallback_dirs(Platform::Windows, &env);
        assert!(dirs.contains(&Path::new("appdata").join("npm")));
        assert!(dirs.contains(&Path::new("local").join("Programs").join("nodejs")));
        assert!(dirs.contains(&Path::new("home").join("scoop").join("shims")));
        assert!(dirs.contains(&PathBuf::from(r"C:\Program Files\nodejs")));
        assert_eq!(dirs.len(), 10);
    }

    #[test]
    fn login_shell_choice() {
        assert_eq!(
            login_shell(Some("/usr/bin/fish"), Platform::Linux),
            "/usr/bin/fish"
        );
        assert_eq!(login_shell(Some("  "), Platform::Macos), "/bin/zsh");
        assert_eq!(login_shell(None, Platform::Macos), "/bin/zsh");
        assert_eq!(login_shell(None, Platform::Linux), "/bin/sh");
    }

    #[cfg(unix)]
    #[test]
    fn reads_the_path_of_a_login_shell() {
        // `/bin/sh -ilc` works everywhere; whatever its PATH is, the marker protocol must deliver a non-empty value.
        let path = read_login_shell_path("/bin/sh").expect("sh prints PATH");
        assert!(!path.is_empty());
        assert!(read_login_shell_path("/nonexistent/shell").is_none());
    }
}
