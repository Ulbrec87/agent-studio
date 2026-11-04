//! Agent Studio entry point: clap dispatch and logging init.

mod logging;
mod oneshot;

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

    /// Capture the mouse in the TUI (wheel scrolls the scrollback, clicks pick agents).
    #[arg(long)]
    mouse: bool,

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
    /// List agents.
    Ls,
    /// Create and start an agent.
    New {
        /// Project directory the agent works in.
        #[arg(long, short = 'p', value_name = "DIR")]
        project: PathBuf,
        /// claude, codex or opencode (default: the configured default provider).
        #[arg(long)]
        provider: Option<String>,
        /// Model id (default: the configured default model).
        #[arg(long, short = 'm')]
        model: Option<String>,
        /// Display name (default: generated).
        #[arg(long, short = 'n')]
        name: Option<String>,
        /// Skip permission prompts.
        #[arg(long)]
        yolo: bool,
        /// First prompt sent to the agent.
        #[arg(long)]
        prompt: Option<String>,
    },
    /// Send text to an agent and submit it.
    Send {
        id: String,
        #[arg(required = true, num_args = 1..)]
        text: Vec<String>,
    },
    /// Stop an agent's process (it stays in the list).
    Kill { id: String },
    /// Remove an agent.
    Rm { id: String },
    /// Restart an agent.
    Restart { id: String },
    /// Show the daemon status (starts it when needed).
    Status,
    /// Stop the daemon.
    Stop {
        /// Stop running agents too (otherwise refused while agents run).
        #[arg(long)]
        force: bool,
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
        Some(Command::Version) => Ok(()),
        Some(command) => oneshot::run(&paths, command),
        None => {
            let _guard = logging::init(&paths, "agent-studio", false);
            tracing::info!("starting tui");
            let mut options = as_tui::TuiOptions::from_env();
            options.mouse |= cli.mouse;
            as_tui::run_with(paths, options)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_oneshot_commands() {
        let cli = Cli::parse_from(["agent-studio", "send", "ab12", "hello", "world"]);
        let Some(Command::Send { id, text }) = cli.command else {
            panic!("expected send")
        };
        assert_eq!(
            (id.as_str(), text.join(" ").as_str()),
            ("ab12", "hello world")
        );
        let cli = Cli::parse_from(["agent-studio", "new", "--project", "/p", "--yolo"]);
        assert!(matches!(cli.command, Some(Command::New { yolo: true, .. })));
        let cli = Cli::parse_from(["agent-studio", "stop", "--force"]);
        assert!(matches!(cli.command, Some(Command::Stop { force: true })));
    }

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
