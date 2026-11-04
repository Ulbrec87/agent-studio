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

## P1 (core agents) gotchas

- **`gen` is a reserved keyword in edition 2024.** Don't name variables/fields `gen`; the manager uses `run_id`
  (a per-spawn counter) to ignore stale reader/exit callbacks of a replaced PTY (Hydra's `managed.pty !== pty`).
- **`std::env::set_var` is `unsafe` in edition 2024** and the workspace warns on `unsafe_code`. `shell::fixed_path()` therefore
  *returns* the fixed PATH; put it into the agent env overrides instead of mutating the daemon's own env.
- **portable-pty `kill()` is not SIGTERM.** Graceful stop needs `nix::sys::signal::kill(pid, SIGTERM)` (cfg(unix),
  `nix` pinned to 0.28 to share the version portable-pty already pulls in); Windows has no graceful signal, `kill()` terminates.
- **Drop the PTY slave after spawn** (`drop(pair.slave)`), or the reader never sees EOF when the child exits. On Windows ConPTY
  the reader only ends when the pseudo-console closes, so the exit watcher waits at most 500 ms for the reader before announcing the exit.
- **Spawning needs `env_clear()`**: portable-pty's `CommandBuilder` starts from the parent env, which would leak the
  `CLAUDE_CODE_*` markers that `build_agent_env` strips. We always pass the complete env.
- **Keep PTY bytes as bytes.** A read can end inside a UTF-8 sequence or an escape sequence; the Codex alt-screen filter holds back a
  trailing partial `ESC[?1049` until the next chunk (a lone trailing ESC is delayed until more output arrives).
- **Hydra's session-id regex is loose** (`session[ _-]?id?[:=\s]+([a-z0-9-]{8,})`, so "session management" matches "management").
  Ported as is; the capture only applies while the agent has no session id and falls back to a UUID pattern. P4's catalog
  discovery should win over it (`set_session_id`).
- **Per-field tolerant config.** One bad value (e.g. `"theme": "neon"`) must not reset the whole `config.json`: `ConfigStore` applies
  patches key by key and ignores values that don't deserialize. Corrupt JSON is copied to `config.json.corrupt` before defaults are written.
- **Tests that wait for threads**: use a poll helper (`wait_for`) instead of fixed sleeps; the fake PTY (`agents::fake_pty`) lets tests
  emit output/exit deterministically. File mtimes can be coarse, so the config reload test sets the mtime explicitly.
