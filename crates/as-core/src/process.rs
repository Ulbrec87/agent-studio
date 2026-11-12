//! Commands for non-interactive background work. PTYs and user-requested external
//! terminals have their own launch paths and must not use these constructors.

use std::ffi::OsStr;
use std::process::Command;

/// Build a background command without allocating a console on Windows.
/// Redirecting stdio alone does not prevent a console window from appearing.
pub fn background_command(program: impl AsRef<OsStr>) -> Command {
    let mut command = Command::new(program);
    configure_background(&mut command);
    command
}

fn configure_background(_command: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        _command.creation_flags(CREATE_NO_WINDOW);
    }
}

/// Async counterpart with the same Windows console policy.
pub fn background_tokio_command(program: impl AsRef<OsStr>) -> tokio::process::Command {
    background_command(program).into()
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::process::{Output, Stdio};

    // Ask the child itself: output redirection can work even when Windows created
    // an unwanted console. GetConsoleWindow must return NULL as well.
    const PROBE: &str = r#"
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
Add-Type -TypeDefinition 'using System; using System.Runtime.InteropServices; public static class ConsoleProbe { [DllImport("kernel32.dll")] public static extern IntPtr GetConsoleWindow(); }'
if ([ConsoleProbe]::GetConsoleWindow() -ne [IntPtr]::Zero) { exit 1 }
[Console]::Out.Write('no-console')
[Console]::Error.Write('stderr-ok')
"#;

    fn assert_no_console(output: Output) {
        assert!(output.status.success(), "{output:?}");
        assert_eq!(output.stdout, b"no-console");
        assert_eq!(output.stderr, b"stderr-ok");
    }

    #[test]
    fn background_child_has_no_console_and_preserves_output() {
        let mut command = background_command("powershell.exe");
        command
            .args(["-NoProfile", "-NonInteractive", "-Command", PROBE])
            .stdin(Stdio::null());
        assert_no_console(command.output().unwrap());
    }

    #[tokio::test]
    async fn async_background_child_has_no_console_and_preserves_output() {
        let mut command = background_tokio_command("powershell.exe");
        command
            .args(["-NoProfile", "-NonInteractive", "-Command", PROBE])
            .stdin(Stdio::null());
        assert_no_console(command.output().await.unwrap());
    }
}
