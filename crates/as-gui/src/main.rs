// Release builds on Windows use the GUI subsystem (no console window behind the app). Daemon mode still works: the daemon
// is spawned detached with null stdio by `ensure_daemon`, and `daemon --foreground` simply has no console to print to
// (it logs to the file in `logs_dir()` either way).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod shell;

use anyhow::Result;
use as_gui::{Mode, parse_args, run_daemon};

fn main() -> Result<()> {
    match parse_args(std::env::args_os().skip(1))? {
        Mode::Daemon { home, foreground } => run_daemon(home, foreground),
        Mode::Gui => shell::run_gui(),
    }
}
