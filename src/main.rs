//! Agent Studio entry point: clap dispatch and logging init.

mod logging;

use std::path::PathBuf;

use anyhow::Result;
use as_core::paths::Paths;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "agent-studio", disable_version_flag = true, about)]
struct Cli {
    /// Data root (same as AGENT_STUDIO_HOME).
    #[arg(long, global = true, value_name = "DIR")]
    home: Option<PathBuf>,

    /// Print the version.
    #[arg(short = 'V', long)]
    version: bool,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Run the background daemon.
    Daemon {
        /// Stay attached to the terminal and log to stderr.
        #[arg(long)]
        foreground: bool,
    },
    /// Print the version.
    Version,
}

fn version_line() -> String {
    format!("agent-studio {}", as_core::VERSION)
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    if cli.version || matches!(cli.command, Some(Command::Version)) {
        println!("{}", version_line());
        return Ok(());
    }
    let paths = match cli.home {
        Some(home) => Paths::at(home),
        None => Paths::resolve()?,
    };
    paths.ensure()?;
    match cli.command {
        Some(Command::Daemon { foreground }) => {
            let _guard = logging::init(&paths, "daemon", foreground);
            tracing::info!(foreground, "starting daemon");
            as_daemon::run(paths, foreground)
        }
        _ => {
            let _guard = logging::init(&paths, "agent-studio", false);
            tracing::info!("starting tui");
            as_tui::run(paths)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_line_has_name() {
        assert!(version_line().starts_with("agent-studio "));
    }

    #[test]
    fn parses_daemon_foreground() {
        let cli = Cli::parse_from(["agent-studio", "--home", "/x", "daemon", "--foreground"]);
        assert!(matches!(
            cli.command,
            Some(Command::Daemon { foreground: true })
        ));
        assert_eq!(cli.home, Some(PathBuf::from("/x")));
    }
}
