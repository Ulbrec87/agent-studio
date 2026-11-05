//! Printing for the one-shot commands that survives a closed pipe.
//!
//! Rust ignores SIGPIPE, so `agent-studio status | head -1` makes `println!` panic with
//! "failed printing to stdout: Broken pipe". These macros write through [`write_out`] instead: a
//! `BrokenPipe` ends the program quietly (the reader has all it wanted), any other write error is
//! reported on stderr and exits with status 1.

use std::io::{self, Write};

/// `println!` that exits quietly when stdout is closed.
macro_rules! outln {
    () => { $crate::out::write_out(format_args!("\n")) };
    ($($arg:tt)*) => { $crate::out::write_out(format_args!("{}\n", format_args!($($arg)*))) };
}

/// `print!` that exits quietly when stdout is closed (flushes, for prompts).
macro_rules! out {
    ($($arg:tt)*) => { $crate::out::write_out(format_args!($($arg)*)) };
}

pub fn write_out(args: std::fmt::Arguments<'_>) {
    let mut stdout = io::stdout().lock();
    finish(stdout.write_fmt(args).and_then(|()| stdout.flush()));
}

/// What to do with the result of a write to stdout.
fn finish(result: io::Result<()>) {
    if let Err(err) = result {
        std::process::exit(exit_code(&err));
    }
}

/// `0` for a closed pipe (silent), `1` for anything else (after saying so on stderr).
fn exit_code(err: &io::Error) -> i32 {
    if err.kind() == io::ErrorKind::BrokenPipe {
        0
    } else {
        // Writing to stderr can fail as well; nothing more to do then.
        let _ = writeln!(io::stderr(), "agent-studio: cannot write to stdout: {err}");
        1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_closed_pipe_is_a_quiet_success() {
        let err = io::Error::from(io::ErrorKind::BrokenPipe);
        assert_eq!(exit_code(&err), 0);
    }

    #[test]
    fn other_write_errors_fail() {
        let err = io::Error::other("disk full");
        assert_eq!(exit_code(&err), 1);
    }

    /// A writer that is closed from the start, like the read end of `| head` after it quit.
    struct Closed;
    impl Write for Closed {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::ErrorKind::BrokenPipe.into())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn writing_to_a_closed_pipe_reports_broken_pipe() {
        let err = writeln!(Closed, "x").expect_err("closed");
        assert_eq!(exit_code(&err), 0);
    }
}
