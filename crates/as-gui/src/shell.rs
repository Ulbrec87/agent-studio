//! Tauri glue (binary only, see the note in `lib.rs`): commands, daemon event forwarding and the window.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use as_core::open_in::OpenOption;
use as_core::paths::Paths;
use as_gui::link::{DaemonLink, DaemonStatus, Outbound, Sink, ensure_connector};
use as_gui::quit::{self, CloseDecision};
use as_gui::{AppInfo, build_app_info, gui_log, logging, native, resolve_paths};
use tauri::{Emitter, Manager};

/// Event names the front end listens to (`gui/src/bridge/tauriTransport.ts`).
const EVENT_DAEMON: &str = "daemon-event";
const EVENT_STATUS: &str = "daemon-status";
/// Emitted to the window when it is closed while agents run (payload: the number of running agents), see `as_gui::quit`.
const EVENT_CONFIRM_QUIT: &str = "confirm-quit";
/// How long a close request waits for the daemon's status before it just closes.
const STATUS_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

#[tauri::command]
fn app_info(paths: tauri::State<'_, Paths>) -> AppInfo {
    build_app_info(&paths)
}

/// Forwards a protocol request (JSON of `as_proto::Request`) to the daemon and returns the JSON of its `Response`.
#[tauri::command]
async fn daemon_request(
    link: tauri::State<'_, Arc<DaemonLink>>,
    request: serde_json::Value,
) -> Result<serde_json::Value, String> {
    link.request(request).await
}

/// The current connection state (the front end asks once at start, then follows `daemon-status`).
#[tauri::command]
fn daemon_status(link: tauri::State<'_, Arc<DaemonLink>>) -> DaemonStatus {
    link.status()
}

/// `window.hydra.logEvent`: a structured renderer event into the GUI log file (fire and forget, never fails).
#[tauri::command]
fn log_event(event: serde_json::Value) {
    gui_log::record(&event);
}

/// Open `path` with `target` in this process (the daemon may run without a display).
#[tauri::command]
async fn open_in(target: String, path: String) -> Result<(), String> {
    tokio::task::spawn_blocking(move || native::open_in(&target, &path))
        .await
        .map_err(|err| format!("open failed: {err}"))?
}

/// Editors, file manager and terminal usable from this process.
#[tauri::command]
async fn list_open_targets() -> Result<Vec<OpenOption>, String> {
    tokio::task::spawn_blocking(native::list_open_targets)
        .await
        .map_err(|err| format!("detection failed: {err}"))
}

/// The user's answer in the quit dialog: `stop_agents` stops every agent and the daemon first ("Quit All"); otherwise only
/// the window goes away and the daemon with its agents keeps running ("Keep Running").
#[tauri::command]
async fn quit_app(
    app: tauri::AppHandle,
    link: tauri::State<'_, Arc<DaemonLink>>,
    stop_agents: bool,
) -> Result<(), String> {
    if stop_agents && let Err(err) = link.request(quit::shutdown_request(true)).await {
        // Exit anyway: the user asked to quit, and a daemon we cannot reach has nothing left to protect.
        tracing::warn!(%err, "stopping the daemon before quitting failed");
    }
    tracing::info!(stop_agents, "quitting");
    app.exit(0);
    Ok(())
}

/// A close request of the main window: close at once when nothing runs, otherwise ask the front end first.
async fn on_close_requested(window: tauri::Window) {
    let link = window.state::<Arc<DaemonLink>>().inner().clone();
    let answer =
        match tokio::time::timeout(STATUS_TIMEOUT, link.request(quit::status_request())).await {
            Ok(answer) => answer,
            Err(_) => Err("the daemon did not answer in time".to_string()),
        };
    match quit::decide_close(&answer) {
        CloseDecision::Close => {
            tracing::info!("closing the window (nothing is running)");
            window.app_handle().exit(0);
        }
        CloseDecision::Confirm { running } => {
            tracing::info!(running, "asking before closing the window");
            if let Err(err) = window.emit(EVENT_CONFIRM_QUIT, running) {
                // Without a dialog the user could never close the window: better close than trap them.
                tracing::warn!(%err, "cannot ask the front end, closing");
                window.app_handle().exit(0);
            }
        }
    }
}

/// Open the desktop window and block until it is closed. Closing the window never stops the daemon or its agents.
pub fn run_gui(home: Option<PathBuf>) -> Result<()> {
    let paths = resolve_paths(home)?;
    let _guard = logging::init(&paths, logging::GUI_LOG_PREFIX, false);
    tracing::info!("starting desktop app");
    let link = DaemonLink::new();
    tauri::Builder::default()
        // Native pieces of the Hydra API: the folder dialog (`selectDirectory`) and opening links (`openExternal`).
        // Capabilities in `capabilities/default.json` allow only `dialog:allow-open` and web/mail links.
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .manage(paths.clone())
        .manage(link.clone())
        .invoke_handler(tauri::generate_handler![
            app_info,
            daemon_request,
            daemon_status,
            log_event,
            open_in,
            list_open_targets,
            quit_app
        ])
        .on_window_event(|window, event| {
            if window.label() != "main" {
                return;
            }
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                // The decision needs the daemon (async): hold the close and finish it ourselves.
                api.prevent_close();
                let window = window.clone();
                tauri::async_runtime::spawn(on_close_requested(window));
            }
        })
        .setup(move |app| {
            let handle = app.handle().clone();
            let sink: Sink = Arc::new(move |out| {
                let result = match out {
                    Outbound::Event(event) => handle.emit(EVENT_DAEMON, event),
                    Outbound::Status(status) => handle.emit(EVENT_STATUS, status),
                };
                if let Err(err) = result {
                    tracing::debug!(%err, "cannot emit to the webview");
                }
            });
            // Never blocks window creation: the daemon is started and connected in the background.
            tauri::async_runtime::spawn(link.run(ensure_connector(paths), sink));
            Ok(())
        })
        .run(tauri::generate_context!())?;
    Ok(())
}
