//! Agent Studio desktop shell.
//!
//! The shell stays thin: it hosts the React front end (`gui/`), answers a few native questions (`app_info`) and, from G1 on,
//! forwards daemon requests and events (`daemon_request`, `daemon-event`). The same binary also *is* the daemon when started
//! as `<exe> daemon --home <dir>` (that is how `as_proto::lifecycle::ensure_daemon` spawns it), see [`parse_args`].
//!
//! This library deliberately does **not** use `tauri`: test binaries that link Tauri fail to start on Windows (missing
//! side-by-side manifest), so everything unit-testable lives here and the Tauri glue lives in `shell.rs` (bin only).

pub mod devbridge;
pub mod link;
pub mod logging;

use std::path::PathBuf;

use anyhow::{Result, bail};
use as_core::paths::Paths;
use serde::Serialize;

pub const APP_NAME: &str = "Agent Studio";

/// What the binary was asked to do, decided from argv before anything else starts.
#[derive(Debug, PartialEq, Eq)]
pub enum Mode {
    /// Open the desktop window (any argv that is not `daemon ...` or `--dev-bridge`). `--home <dir>` picks the data dir.
    Gui { home: Option<PathBuf> },
    /// `--dev-bridge [--home <dir>]`: no window, the daemon plumbing behind a localhost WebSocket (dev/test tool, see [`devbridge`]).
    DevBridge { home: Option<PathBuf> },
    /// Run the background daemon, exactly like `agent-studio daemon [--home <dir>] [--foreground]`.
    Daemon {
        home: Option<PathBuf>,
        foreground: bool,
    },
}

/// Decide the mode from the arguments *after* the program name.
///
/// Only a first argument of `daemon` selects daemon mode, so Tauri/WebKit flags or a bare launch always open the GUI.
/// In daemon mode both `--home <dir>` and `--home=<dir>` are accepted (the CLI's `--home` is global, so it may come
/// before or after the subcommand; `ensure_daemon` puts it after) and unknown flags are an error, as they are in the CLI.
pub fn parse_args<I, S>(args: I) -> Result<Mode>
where
    I: IntoIterator<Item = S>,
    S: Into<std::ffi::OsString>,
{
    let mut args = args.into_iter().map(Into::into);
    let first = args.next();
    if first.as_ref().is_none_or(|first| first != "daemon") {
        return Ok(parse_gui_args(first.into_iter().chain(args)));
    }
    let mut home = None;
    let mut foreground = false;
    while let Some(arg) = args.next() {
        let text = arg.to_string_lossy();
        if text == "--foreground" {
            foreground = true;
        } else if text == "--home" {
            let Some(dir) = args.next() else {
                bail!("--home needs a directory");
            };
            home = Some(PathBuf::from(dir));
        } else if let Some(dir) = text.strip_prefix("--home=") {
            home = Some(PathBuf::from(dir));
        } else {
            bail!("unexpected argument for daemon: {text}");
        }
    }
    Ok(Mode::Daemon { home, foreground })
}

/// GUI-mode arguments: only `--dev-bridge` and `--home` are ours; anything else (Tauri/WebKit flags) is ignored.
fn parse_gui_args(args: impl Iterator<Item = std::ffi::OsString>) -> Mode {
    let mut home = None;
    let mut dev_bridge = false;
    let mut args = args;
    while let Some(arg) = args.next() {
        let text = arg.to_string_lossy();
        if text == "--dev-bridge" {
            dev_bridge = true;
        } else if text == "--home" {
            home = args.next().map(PathBuf::from);
        } else if let Some(dir) = text.strip_prefix("--home=") {
            home = Some(PathBuf::from(dir));
        }
    }
    if dev_bridge {
        Mode::DevBridge { home }
    } else {
        Mode::Gui { home }
    }
}

pub fn resolve_paths(home: Option<PathBuf>) -> Result<Paths> {
    let paths = match home {
        Some(home) => Paths::at(home),
        None => Paths::resolve()?,
    };
    paths.ensure()?;
    Ok(paths)
}

/// Run the daemon in this process (blocks until it stops).
pub fn run_daemon(home: Option<PathBuf>, foreground: bool) -> Result<()> {
    let paths = resolve_paths(home)?;
    let _guard = logging::init(&paths, "daemon", foreground);
    tracing::info!(foreground, "starting daemon (hosted by the desktop binary)");
    as_daemon::run(paths, foreground)
}

/// `agent-studio-gui --dev-bridge`: serve the daemon plumbing over a localhost WebSocket until Ctrl+C. Prints one line
/// `DEV_BRIDGE ws://127.0.0.1:<port> token=<token>` on stdout. Dev/test tool only.
pub fn run_dev_bridge(home: Option<PathBuf>) -> Result<()> {
    use std::io::Write;
    let paths = resolve_paths(home)?;
    let _guard = logging::init(&paths, "gui-dev-bridge", true);
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async move {
        let link = link::DaemonLink::new();
        let hub = devbridge::Hub::new();
        let token = devbridge::new_token();
        let info = serde_json::to_value(build_app_info(&paths))?;
        let bound = devbridge::bind(link.clone(), hub.clone(), token.clone(), info).await?;
        let mut out = std::io::stdout();
        writeln!(out, "{}", devbridge::announce_line(bound.addr, &token))?;
        out.flush()?;
        let run = link.run(link::ensure_connector(paths), hub.sink());
        tokio::select! {
            () = run => {}
            _ = tokio::signal::ctrl_c() => {}
        }
        bound.task.abort();
        Ok(())
    })
}

/// Answer of the `app_info` command (camelCase JSON, mirrored by `AppInfo` in `gui/src/bridge/transport.ts`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    pub name: String,
    pub version: String,
    pub data_dir: String,
    /// Node-style platform name (`linux`, `darwin`, `win32`), because the Hydra renderer compares against those.
    pub platform: String,
}

/// Map Rust's `std::env::consts::OS` to Node's `process.platform` names.
pub fn node_platform(os: &str) -> &str {
    match os {
        "macos" => "darwin",
        "windows" => "win32",
        other => other,
    }
}

pub fn build_app_info(paths: &Paths) -> AppInfo {
    AppInfo {
        name: APP_NAME.to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        data_dir: paths.root().to_string_lossy().into_owned(),
        platform: node_platform(std::env::consts::OS).to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Mode> {
        parse_args(args.iter().copied())
    }

    #[test]
    fn no_arguments_opens_the_gui() {
        assert_eq!(parse(&[]).unwrap(), Mode::Gui { home: None });
    }

    #[test]
    fn other_arguments_open_the_gui() {
        assert_eq!(
            parse(&["--some-webkit-flag"]).unwrap(),
            Mode::Gui { home: None }
        );
        // `daemon` only counts as the *first* argument.
        assert_eq!(
            parse(&["--home", "/x", "daemon"]).unwrap(),
            Mode::Gui {
                home: Some(PathBuf::from("/x"))
            }
        );
    }

    #[test]
    fn dev_bridge_flag_with_and_without_home() {
        assert_eq!(
            parse(&["--dev-bridge"]).unwrap(),
            Mode::DevBridge { home: None }
        );
        assert_eq!(
            parse(&["--home=/h", "--dev-bridge"]).unwrap(),
            Mode::DevBridge {
                home: Some(PathBuf::from("/h"))
            }
        );
        assert_eq!(
            parse(&["--dev-bridge", "--home", "/h"]).unwrap(),
            Mode::DevBridge {
                home: Some(PathBuf::from("/h"))
            }
        );
    }

    #[test]
    fn daemon_as_ensure_daemon_spawns_it() {
        // Exactly the argv of `as_proto::lifecycle::spawn_daemon`: `daemon --home <root>`.
        assert_eq!(
            parse(&["daemon", "--home", "/data"]).unwrap(),
            Mode::Daemon {
                home: Some(PathBuf::from("/data")),
                foreground: false
            }
        );
    }

    #[test]
    fn daemon_foreground_and_equals_form() {
        assert_eq!(
            parse(&["daemon", "--foreground", "--home=/d"]).unwrap(),
            Mode::Daemon {
                home: Some(PathBuf::from("/d")),
                foreground: true
            }
        );
        assert_eq!(
            parse(&["daemon"]).unwrap(),
            Mode::Daemon {
                home: None,
                foreground: false
            }
        );
    }

    #[test]
    fn daemon_rejects_bad_arguments() {
        assert!(parse(&["daemon", "--home"]).is_err());
        assert!(parse(&["daemon", "--nope"]).is_err());
    }

    #[test]
    fn app_info_reports_name_version_dir_and_node_platform() {
        let paths = Paths::at(PathBuf::from("data-root"));
        let info = build_app_info(&paths);
        assert_eq!(info.name, "Agent Studio");
        assert_eq!(info.version, env!("CARGO_PKG_VERSION"));
        assert_eq!(PathBuf::from(&info.data_dir), PathBuf::from("data-root"));
        assert_eq!(info.platform, node_platform(std::env::consts::OS));
        let json = serde_json::to_value(&info).unwrap();
        assert!(json.get("dataDir").is_some(), "camelCase keys: {json}");
    }

    #[test]
    fn node_platform_names() {
        assert_eq!(node_platform("macos"), "darwin");
        assert_eq!(node_platform("windows"), "win32");
        assert_eq!(node_platform("linux"), "linux");
    }
}
