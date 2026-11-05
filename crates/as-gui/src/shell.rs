//! Tauri glue (binary only, see the note in `lib.rs`): commands and the window.

use anyhow::Result;
use as_core::paths::Paths;
use as_gui::{AppInfo, build_app_info, logging, resolve_paths};

#[tauri::command]
fn app_info() -> Result<AppInfo, String> {
    let paths = Paths::resolve().map_err(|err| err.to_string())?;
    Ok(build_app_info(&paths))
}

/// Seam for G1: forwards a protocol request to the daemon and returns its response. Not wired yet.
#[tauri::command]
async fn daemon_request(request: serde_json::Value) -> Result<serde_json::Value, String> {
    let _ = request;
    Err("not wired (G1)".to_string())
}

/// Open the desktop window and block until it is closed.
pub fn run_gui() -> Result<()> {
    let paths = resolve_paths(None)?;
    let _guard = logging::init(&paths, "gui", false);
    tracing::info!("starting desktop app");
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![app_info, daemon_request])
        .run(tauri::generate_context!())?;
    Ok(())
}
