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
  `No, exit`), Hydra's delayed `/rename <name>\r` presses Enter on "No, exit" and the agent ends as `errored`. *Fixed in P4*
  (see "Trust prompt gate" below).
- **`agent-studio status`/`ls` start a daemon** when none runs (by design, `ensure_daemon`): after `stop` do not call them
  when you want to verify the daemon is gone; check the lock file / `pgrep -x agent-studio` instead. Do not `pkill -f` a
  pattern that appears in your own shell command line.
- **tmux for TUI checks.** `tmux new-session -d -x 130 -y 36 'AGENT_STUDIO_HOME=… agent-studio'` + `tmux send-keys` /
  `tmux capture-pane -p` is enough to drive the TUI (use `C-a` for the prefix); tmux has `focus-events` off by default.

## P4 (sessions) gotchas

- **Trust prompt gate (`agents::startup::TrustGate`).** Claude's startup input (`/rename`, initial prompt) is held while the
  "Is this a project you created or one you trust?" / "Yes, I trust this folder" dialog is on the agent's output
  (ANSI stripped, compared with all non-alphanumerics removed, because the UI places words with cursor moves). Released when
  (1) the user answered (Enter or a digit; arrow keys only move the selection and never release), (2) new output arrived
  after the answer that no longer shows the dialog and has >= 40 visible characters (the real UI), and (3) 1.5 s passed
  with no redraw of the dialog. Not released within 5 min: the held writes are dropped with a `warn` log; the process
  exiting drops them silently. Without a dialog the writes go out on Hydra's schedule once the CLI printed anything
  (or after 10 s of silence). `rename_agent` on a running Claude defers its `/rename` the same way. Only Claude is gated.
  Other first-run dialogs (MCP server approval, API-key prompt) are not detected. Verified by hand with a real
  untrusted folder: the dialog stays up, no exit, `/rename` and the prompt arrive after "Yes".
- **Session-id regexes tightened.** Hydra's `session[ _-]?id?[:=\s]+([a-z0-9-]{8,})` also took "session management" ->
  "management". Claude now needs a UUID after "session id", OpenCode a `ses_<10+ alphanumerics>` id anywhere. The generic
  "any UUID in the output" fallback was **removed** (it adopted UUIDs from the conversation); the catalog discovery is the
  real source. The regex runs on ANSI-stripped text (a colored "Session ID:" label used to break it).
- **Discovery polls the catalog, never under a lock.** The manager asks the `SessionLocator` from `spawn_blocking`
  (immediately with a forced rescan, then 2.5 s x 1.6^n, max 30 s); Claude stops once it has an id, Codex keeps checking
  until its session was modified since this run started (resume creates a new rollout). Imported agents never discover.
- **The Claude profile resolver runs under the manager lock** (called while spawning), so it must stay cheap: it looks for
  `<id>.jsonl` directly in each profile's `projects/*` (no catalog scan) and only then falls back to the project mapping.
- **Claude leaves `<id>.orphaned-<ts>.jsonl` copies** next to sessions; their header has the original session id, so the
  catalog dedupes by session id (newest wins). First lines can also be huge: only the first 256 KB of a transcript are
  parsed for the header (a session whose id/cwd are not in that head is skipped), message counts read at most 8 MB.
- **Fuzzy picker.** `nucleo-matcher` matches letters scattered over long prompts, so short queries match many rows; the
  list is sorted by score, and a new query must reset the cursor to the top (keeping the old selection scrolled the best
  match out of view). Typing is only a command in filter mode (`/`); in list mode `h a p s r j k` are commands.
- **Unix socket paths are limited to ~104 bytes** (`sun_path`): a test `AGENT_STUDIO_HOME` inside the long scratchpad path
  fails with "local socket name length exceeds capacity"; use `/tmp/as4h`-like short homes for manual runs.
- **Stop the TUI before `stop --force`.** A TUI that is still open reconnects and starts a new daemon right after it was
  stopped; quit the tmux session (or the TUI) first, then stop the daemon, then check with `pgrep -f 'daemon --home <dir>'`.
- **`CLAUDE_CONFIG_DIR` of the shell leaks into agents.** With it set (e.g. to `~/.claude-work`) new Claude sessions are
  saved in that profile; discovery looks in every profile so it still finds them, and `claudeProfileByProject` overrides it.

## P5 (grid, palette, keybindings) gotchas

- **Tiles must not resize the PTYs.** Every attached terminal (and its PTY) is kept at the *chat pane* size, whatever the
  view (`App::pane_size` ignores the broadcast bar; `sync_layout` resizes all terms, not only the selected one). A grid tile
  renders a window of that screen: rows end at the last non-blank row or the cursor row (`TerminalEmulator::content_rows`),
  columns start at the left edge unless the cursor is further right. Switching Chat <-> Grid never reflows Claude's UI.
- **`Terminal::clear()` after running an editor leaves a half-painted screen.** ratatui's `clear` asks the terminal for the
  cursor position (reads stdin) and the diff renderer then only repaints changed cells. After `suspend::run_on_terminal`
  replace the `Terminal` with a fresh `Terminal::new(..)`; stop reading events (`EventStream` dropped, ~150 ms grace) while
  the editor owns the keyboard.
- **The daemon's `Broadcast` resumes every agent of the project** (Hydra's `ensureProcess`), including the hundreds of idle
  `sess-*` agents imported at startup. The TUI therefore sends `SendInput` to each *running* agent of the tab instead.
- **Tests must not depend on the wall clock.** The grid hides non-active agents older than `sessionMaxAgeDays` (like Hydra's
  `recentGroups`), so fixtures dated in the past vanish; fixtures set `session_max_age_days: 0`.
- **The Running tab must stay when its agents stop.** The grid only follows the selected agent to its project tab when the
  selection changed on purpose (`GridState::follow`), otherwise the tab jumps away while the last running agent finishes.
- **Manual checks start real imports.** A fresh `AGENT_STUDIO_HOME` imports every session in `~/.claude*` as idle agents
  (`importSessionsOnStartup`); write `{"importSessionsOnStartup": false}` into `<home>/config.json` first.
