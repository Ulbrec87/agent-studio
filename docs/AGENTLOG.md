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

## P2 (daemon + protocol) gotchas

- **`futures-util` needs the `sink` feature** for `SinkExt` (`Framed::split()` / `.send()`); default features don't have it.
- **Detaching the daemon without `unsafe`.** `pre_exec(setsid)` is unsafe (workspace warns on `unsafe_code`). We use
  `Command::process_group(0)` + null stdio and the daemon ignores SIGHUP; that is enough to survive the terminal closing.
- **Subscribe must not duplicate or lose bytes.** The manager appends to its buffer and emits the event under one lock, but
  the dispatcher may still hold queued events at snapshot time. Subscribe therefore goes through the dispatcher task, which
  first drains the broadcast queue, then snapshots the buffer and registers the subscription (a microsecond window can still
  repeat the last chunk, never lose it).
- **Handle a client's requests sequentially.** Spawning a task per request would reorder `SendRaw` keystrokes.
- **Fake PTYs ignore signals**: daemon tests that stop running agents need a helper that calls `emit_exit` once `signals()`
  is non-empty, or `wait_all_stopped` burns its full 5 s.
- **Liveness = pid alive AND socket accepts.** A lock with a live (recycled) pid and a refusing socket is stale. On Windows
  the pid check is skipped (always "alive"), the socket probe decides.
- **No real interprocess crate source in registry until `cargo fetch`**: use `ListenerOptions::new().name(..).create_tokio()`
  and `Name` from `String::to_fs_name::<GenericFilePath>()`; it accepts the `\\.\pipe\...` path on Windows too.

## P3 (TUI) gotchas

- **`tui-term` / `tui-textarea` skipped.** `tui-textarea` 0.7 targets ratatui 0.29 (cannot be combined with ratatui 0.30);
  `tui-term` 0.3 pins its own `vt100`. We render `vt100::Screen` cell by cell (`term/render.rs`, ~60 lines) and have a small
  multi-line `widgets::TextArea`. Both sit behind `TerminalEmulator` / plain widgets, so swapping later is local.
- **vt100 0.18 API.** `Parser::new` takes `NonZeroU16` rows/cols; callbacks (`Parser::new_with_callbacks`) are how bell and
  unhandled CSI arrive. CLIs query the terminal (Codex asks `ESC[6n`); vt100 does not answer, so `term::vt::Replies` answers
  CPR/DSR/DA and the app sends the bytes back with `SendRaw`. **Replies produced while replaying the `Subscribe` buffer are
  dropped** (the question was answered long ago; replaying the answer would type garbage into the agent).
- **A background `bash -ilc` hangs on a tty.** The daemon used to be spawned with `process_group(0)`; the login shell used to
  discover PATH (`shell::fixed_path`) then ran in a background process group of the user's terminal, stopped on SIGTTIN and
  burned its whole 5 s timeout, so the first start of the TUI failed ("daemon did not start within 5s"). Fix: do not make
  the daemon a group leader; `as_daemon::run` calls `nix::unistd::setsid()` (safe fn, fails with EPERM only for a leader).
- **Output racing the `Subscribe` reply.** The reply (oneshot) and `AgentOutput` events (channel) are separate paths into the
  app, so live output can be processed before the buffer. `SubState::Requested(pending)` holds it back and replays it after the buffer.
- **Never write outside the `Buffer`.** `Buffer::set_string` panics outside its area; dialogs are drawn on an off-screen
  canvas (min 80x40) and copied back, so tiny terminals clip instead of crashing.
- **Panic hook only restores on the main thread**, otherwise a panic in a background task would leave raw mode while the app lives.
- **Claude's trust prompt vs `/rename`.** On a folder Claude does not trust yet ("Is this a project you trust?", default
  `No, exit`), Hydra's delayed `/rename <name>\r` presses Enter on "No, exit" and the agent ends as `errored`. Use a folder that
  was trusted once (run `claude` there) or fix in P4/P10 (detect the prompt before renaming).
- **`agent-studio status`/`ls` start a daemon** when none runs (by design, `ensure_daemon`): after `stop` do not call them
  when you want to verify the daemon is gone; check the lock file / `pgrep -x agent-studio` instead. Do not `pkill -f` a
  pattern that appears in your own shell command line.
- **tmux for TUI checks.** `tmux new-session -d -x 130 -y 36 'AGENT_STUDIO_HOME=… agent-studio'` + `tmux send-keys` /
  `tmux capture-pane -p` is enough to drive the TUI (use `C-a` for the prefix); tmux has `focus-events` off by default.
