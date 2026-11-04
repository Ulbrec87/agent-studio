//! Background daemon: local-socket server, client registry, event fan-out,
//! lock file and lifecycle, MCP server and OS notifications.

use as_core::paths::Paths;

/// Run the daemon. Implemented in P2.
pub fn run(_paths: Paths, _foreground: bool) -> anyhow::Result<()> {
    Ok(())
}
