//! Terminal modes of the TUI, and running an external program on the real terminal (an editor, a
//! pager, a shell): the TUI leaves the alternate screen and raw mode, the program gets
//! stdin/stdout/stderr, and the TUI is restored afterwards. Reusable for every "open in $EDITOR"
//! action ([`run_on_terminal`]).

use std::io::{self, Write};
use std::path::Path;

use anyhow::{Context, Result};
use ratatui::crossterm::event::{
    DisableBracketedPaste, DisableFocusChange, DisableMouseCapture, EnableBracketedPaste,
    EnableFocusChange, EnableMouseCapture,
};
use ratatui::crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::crossterm::{cursor, execute};

/// Raw mode, alternate screen, bracketed paste, focus events (and the mouse when asked).
pub fn enter_terminal(mouse: bool) -> Result<()> {
    enable_raw_mode().context("enabling raw mode")?;
    let mut out = io::stdout();
    execute!(
        out,
        EnterAlternateScreen,
        EnableBracketedPaste,
        EnableFocusChange
    )?;
    if mouse {
        execute!(out, EnableMouseCapture)?;
    }
    // Save the window title (xterm title stack) so it can be restored on exit.
    out.write_all(b"\x1b[22;0t")?;
    out.flush()?;
    Ok(())
}

/// Best-effort restore of the normal terminal; safe to call twice.
pub fn restore_terminal(mouse: bool) {
    let mut out = io::stdout();
    if mouse {
        let _ = execute!(out, DisableMouseCapture);
    }
    let _ = out.write_all(b"\x1b[23;0t");
    let _ = execute!(
        out,
        DisableFocusChange,
        DisableBracketedPaste,
        LeaveAlternateScreen,
        cursor::Show
    );
    let _ = disable_raw_mode();
    let _ = out.flush();
}

/// Leave the TUI, run `f` with the normal terminal (it may read the keyboard), and re-enter. The
/// caller must not be reading terminal events meanwhile and should redraw everything afterwards.
pub fn run_on_terminal<R>(mouse: bool, f: impl FnOnce() -> R) -> Result<R> {
    restore_terminal(mouse);
    let result = f();
    enter_terminal(mouse)?;
    Ok(result)
}

/// A program to run with the terminal handed over.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalCommand {
    pub program: String,
    pub args: Vec<String>,
}

/// The editor command for a file: `$VISUAL`, then `$EDITOR`, then a platform default. Arguments in
/// the variable (`code -w`) are kept; the file is appended.
pub fn editor_command(path: &Path, visual: Option<&str>, editor: Option<&str>) -> ExternalCommand {
    let chosen = [visual, editor]
        .into_iter()
        .flatten()
        .map(str::trim)
        .find(|v| !v.is_empty());
    let default = if cfg!(windows) { "notepad" } else { "vi" };
    let mut words = chosen.unwrap_or(default).split_whitespace();
    let program = words.next().unwrap_or(default).to_string();
    let mut args: Vec<String> = words.map(str::to_string).collect();
    args.push(path.to_string_lossy().into_owned());
    ExternalCommand { program, args }
}

/// [`editor_command`] with the editor from the environment.
pub fn editor_command_from_env(path: &Path) -> ExternalCommand {
    editor_command(
        path,
        std::env::var("VISUAL").ok().as_deref(),
        std::env::var("EDITOR").ok().as_deref(),
    )
}

/// Run the command to completion with inherited stdio. Returns an error text when it could not
/// start or exited unsuccessfully.
pub fn run_blocking(cmd: &ExternalCommand) -> Result<(), String> {
    match std::process::Command::new(&cmd.program)
        .args(&cmd.args)
        .status()
    {
        Ok(status) if status.success() => Ok(()),
        Ok(status) => Err(format!("{} exited with {status}", cmd.program)),
        Err(err) => Err(format!("could not run {}: {err}", cmd.program)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_visual_over_editor_over_the_default() {
        let p = Path::new("/tmp/k.json");
        let c = editor_command(p, Some("nvim"), Some("nano"));
        assert_eq!((c.program.as_str(), c.args.len()), ("nvim", 1));
        assert_eq!(editor_command(p, None, Some("nano")).program, "nano");
        assert_eq!(editor_command(p, Some("  "), Some("nano")).program, "nano");
        let d = editor_command(p, None, None);
        assert_eq!(d.program, if cfg!(windows) { "notepad" } else { "vi" });
        assert_eq!(d.args, ["/tmp/k.json"]);
    }

    #[test]
    fn editor_arguments_are_kept_and_the_file_comes_last() {
        let c = editor_command(Path::new("/x/y.json"), Some("code --wait"), None);
        assert_eq!(c.program, "code");
        assert_eq!(c.args, ["--wait", "/x/y.json"]);
    }

    #[cfg(unix)]
    #[test]
    fn runs_programs_and_reports_failures() {
        let ok = ExternalCommand {
            program: "true".into(),
            args: vec![],
        };
        assert_eq!(run_blocking(&ok), Ok(()));
        let bad = ExternalCommand {
            program: "false".into(),
            args: vec![],
        };
        assert!(run_blocking(&bad).unwrap_err().contains("exited"));
        let missing = ExternalCommand {
            program: "definitely-not-a-program-xyz".into(),
            args: vec![],
        };
        assert!(
            run_blocking(&missing)
                .unwrap_err()
                .contains("could not run")
        );
    }
}
