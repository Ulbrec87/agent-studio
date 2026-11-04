# Agent log

Mistakes, gotchas and lessons learned. Append an entry when something non-obvious bites you.
Format: `## <short title>` + what happened + what to do instead.

## Hydra gotchas inherited (from Hydra's AGENTLOG / source)

- **Parent Claude session env leaks into agents.** When launched from inside Claude Code, vars like
  `CLAUDE_CODE_CHILD_SESSION` make the spawned `claude` stop saving its transcript. Strip the list in
  `as_core::agents::env::PARENT_SESSION_ENV_VARS` before spawning (keep `CLAUDE_CONFIG_DIR`).
- **Claude needs a second Enter for pasted text.** Submitting input = write text, wait ~100 ms, `\r`,
  and for Claude wait ~100 ms and send another `\r` (it shows `[Pasted text …]` and asks to confirm).
- **`--name` doesn't name Claude sessions.** Hydra writes `/rename <name>\r` ~1.5 s after spawn and
  delays the initial prompt to ~3 s.
- **Codex toggles the alternate screen.** Strip `ESC[?47h/l`, `ESC[?1047h/l`, `ESC[?1049h/l` from Codex
  output so scrollback keeps working.
- **CLI not found from GUI/daemon.** PATH set in shell rc files is missing → spawn through the login
  shell (`$SHELL -lc '<cmd>'`) in `auto`/`login` shell mode; Windows `.cmd` shims need `cmd.exe /c`.
- **Claude session registry is undocumented** (`<configDir>/sessions/<pid>.json`): parse tolerantly,
  check the pid is alive, ignore files > 64 KB.
- **`cargo add --features crate/feature` for a workspace-root dep.** Passing `tracing-subscriber/env-filter` for a
  multi-package `cargo add` did not set features; edit Cargo.toml by hand (`env-filter`, `json`).
- **Version flag.** clap's auto `--version` prints only the crate version; we use `disable_version_flag` plus our own
  `-V/--version` so output is `agent-studio <version>` like the `version` subcommand.
