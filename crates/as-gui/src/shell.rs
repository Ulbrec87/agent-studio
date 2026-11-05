//! Tauri glue (binary only, see the note in `lib.rs`): commands, daemon event forwarding and the window.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use as_core::paths::Paths;
use as_gui::link::{DaemonLink, DaemonStatus, Outbound, Sink, ensure_connector};
use as_gui::{AppInfo, build_app_info, logging, resolve_paths};
use tauri::Emitter;

/// Event names the front end listens to (`gui/src/bridge/tauriTransport.ts`).
const EVENT_DAEMON: &str = "daemon-event";
const EVENT_STATUS: &str = "daemon-status";

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

/// Open the desktop window and block until it is closed. Closing the window never stops the daemon or its agents.
pub fn run_gui(home: Option<PathBuf>) -> Result<()> {
    let paths = resolve_paths(home)?;
    let _guard = logging::init(&paths, "gui", false);
    tracing::info!("starting desktop app");
    let link = DaemonLink::new();
    tauri::Builder::default()
        .manage(paths.clone())
        .manage(link.clone())
        .invoke_handler(tauri::generate_handler![
            app_info,
            daemon_request,
            daemon_status
        ])
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
