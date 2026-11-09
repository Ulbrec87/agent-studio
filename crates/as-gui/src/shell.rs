//! Tauri glue (binary only, see the note in `lib.rs`): commands, daemon event forwarding and the window.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use as_core::open_in::OpenOption;
use as_core::paths::Paths;
use as_gui::link::{DaemonLink, DaemonStatus, Outbound, Sink, ensure_connector};
use as_gui::quit::{self, CloseDecision, ExitGate};
use as_gui::{AppInfo, build_app_info, gui_log, logging, native, resolve_paths};
use tauri::{Emitter, Manager};
use tauri_plugin_window_state::StateFlags;

/// Event names the front end listens to (`gui/src/bridge/tauriTransport.ts`).
/// The label of the one window (`tauri.conf.json`).
const MAIN_WINDOW: &str = "main";
const EVENT_DAEMON: &str = "daemon-event";
const EVENT_STATUS: &str = "daemon-status";
/// Emitted to the window when it is closed while agents run (payload: the number of running agents), see `as_gui::quit`.
const EVENT_CONFIRM_QUIT: &str = "confirm-quit";
/// How long a close request waits for the daemon's status before it just closes.
const STATUS_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);
/// How long closing an idle app waits for the daemon to acknowledge its shutdown.
/// The window is shown at the latest this long after start even if the page did not finish loading.
const SHOW_FALLBACK: std::time::Duration = std::time::Duration::from_secs(4);
const SHUTDOWN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

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
    gate: tauri::State<'_, Arc<ExitGate>>,
    stop_agents: bool,
) -> Result<(), String> {
    if stop_agents && let Err(err) = link.request(quit::shutdown_request(true)).await {
        // Exit anyway: the user asked to quit, and a daemon we cannot reach has nothing left to protect.
        tracing::warn!(%err, "stopping the daemon before quitting failed");
    }
    tracing::info!(stop_agents, "quitting");
    gate.begin_exit();
    app.exit(0);
    Ok(())
}

/// Exit for good (the flow's decision is made): the gate lets this `ExitRequested` through.
fn exit_now(app: &tauri::AppHandle) {
    app.state::<Arc<ExitGate>>().begin_exit();
    app.exit(0);
}

/// A request to leave the app, from closing the window or from the OS (Cmd+Q, app menu, Dock): exit at once when nothing runs,
/// otherwise ask the front end first.
async fn run_quit_flow(app: tauri::AppHandle) {
    let link = app.state::<Arc<DaemonLink>>().inner().clone();
    let answer =
        match tokio::time::timeout(STATUS_TIMEOUT, link.request(quit::status_request())).await {
            Ok(answer) => answer,
            Err(_) => Err("the daemon did not answer in time".to_string()),
        };
    match quit::decide_close(&answer) {
        CloseDecision::Close => {
            tracing::info!("quitting (the daemon could not be asked)");
            exit_now(&app);
        }
        CloseDecision::CloseAndStopDaemon => {
            tracing::info!("quitting (nothing is running): stopping the daemon");
            // Best effort: a slow or dead daemon never keeps the window open.
            let request = link.request(quit::shutdown_request(false));
            if let Err(err) = tokio::time::timeout(SHUTDOWN_TIMEOUT, request).await {
                tracing::warn!(%err, "the daemon did not confirm its shutdown in time");
            }
            exit_now(&app);
        }
        CloseDecision::Confirm { running } => {
            tracing::info!(running, "asking before quitting");
            let asked = app.get_webview_window(MAIN_WINDOW).is_some_and(|window| {
                // A Cmd+Q while the window is minimized or hidden must show the question.
                let _ = window.unminimize();
                let _ = window.show();
                let _ = window.set_focus();
                window.emit(EVENT_CONFIRM_QUIT, running).is_ok()
            });
            if !asked {
                // Without a dialog the user could never quit: better exit than trap them.
                tracing::warn!("cannot ask the front end, quitting");
                exit_now(&app);
            }
        }
    }
}

/// Shows the main window (created hidden, see `tauri.conf.json`); harmless when it is already visible.
fn show_main_window(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window(MAIN_WINDOW)
        && !window.is_visible().unwrap_or(false)
    {
        let _ = window.show();
        let _ = window.set_focus();
    }
}

/// Open the desktop window and block until it is closed. Closing the window stops an idle daemon; with agents running the daemon and its agents live on.
pub fn run_gui(home: Option<PathBuf>) -> Result<()> {
    let paths = resolve_paths(home)?;
    let _guard = logging::init(&paths, logging::GUI_LOG_PREFIX, false);
    tracing::info!("starting desktop app");
    let link = DaemonLink::new();
    let app = tauri::Builder::default()
        // Size, position and maximized state survive restarts (clamped to a visible monitor by the plugin). Visibility is left out:
        // the window is created hidden (no white flash) and shown by `show_when_ready` once the page has loaded.
        .plugin(
            tauri_plugin_window_state::Builder::default()
                .with_state_flags(StateFlags::SIZE | StateFlags::POSITION | StateFlags::MAXIMIZED)
                .build(),
        )
        // Native pieces of the Hydra API: the folder dialog (`selectDirectory`) and opening links (`openExternal`).
        // Capabilities in `capabilities/default.json` allow only `dialog:allow-open` and web/mail links.
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .manage(paths.clone())
        .manage(link.clone())
        .manage(Arc::new(ExitGate::new()))
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
            if window.label() != MAIN_WINDOW {
                return;
            }
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                // The decision needs the daemon (async): hold the close and finish it ourselves.
                api.prevent_close();
                tauri::async_runtime::spawn(run_quit_flow(window.app_handle().clone()));
            }
        })
        .on_page_load(|webview, payload| {
            if webview.label() == MAIN_WINDOW
                && matches!(payload.event(), tauri::webview::PageLoadEvent::Finished)
            {
                show_main_window(webview.app_handle());
            }
        })
        .setup(move |app| {
            // If the page never reports `Finished` (broken bundle, dev server down) the window must still appear.
            let late = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                tokio::time::sleep(SHOW_FALLBACK).await;
                show_main_window(&late);
            });
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
        .build(tauri::generate_context!())?;
    app.run(|app, event| {
        // Cmd+Q, the app menu's Quit and the Dock's Quit end up here without a window event: same flow as closing the window.
        if let tauri::RunEvent::ExitRequested { code, api, .. } = event
            && app.state::<Arc<ExitGate>>().intercepts(code)
        {
            api.prevent_exit();
            tauri::async_runtime::spawn(run_quit_flow(app.clone()));
        }
    });
    Ok(())
}
