// Release builds on Windows use the GUI subsystem (no console window behind the app). Daemon mode still works: the daemon
// is spawned detached with null stdio by `ensure_daemon`, and `daemon --foreground` simply has no console to print to
// (it logs to the file in `logs_dir()` either way).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod shell;

use anyhow::Result;
use as_gui::{Mode, parse_args, run_daemon, run_dev_bridge};

fn main() -> Result<()> {
    // First thing, while the process is single-threaded: a launcher/systemd PATH lacks ~/.local/bin, nvm, bun... (the GUI
    // process and the daemon it hosts both need to find `claude`, editors, `gh`).
    as_core::shell_env::fix_path();
    match parse_args(std::env::args_os().skip(1))? {
        Mode::Daemon { home, foreground } => run_daemon(home, foreground),
        Mode::DevBridge { home } => run_dev_bridge(home),
        Mode::Gui { home } => shell::run_gui(home),
    }
}
