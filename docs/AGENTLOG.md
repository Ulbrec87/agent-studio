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
  P10 added the MCP server approval and the custom API-key prompt to the same gate (see "P10 gotchas"). Verified by hand with a real
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

## P6 (Overview) gotchas

- **Claude only writes a pending tool use to the transcript when it finishes.** With Claude Code 2.1.318 the
  `AskUserQuestion` (and a permission prompt) is not in `<session>.jsonl` while the prompt is on screen, so `waitingText`
  can be empty for a needs-you agent ("Asked you a question" without the question). A `Bash` approval that was written
  before the prompt shows "Run: ...". Hydra has the same limit; nothing to fix on our side.
- **A user with Claude's `auto` permission mode never sees prompts.** To produce a needs-you in a manual check, press
  `Shift+Tab` in the agent until the footer says "manual mode", or use `AskUserQuestion`.
- **sysinfo 0.40: `System::new()` returns a `Result`**; `refresh_processes(ProcessesToUpdate, remove_dead)` and
  `Process::parent()` are what the process tree needs (`ProcessesToUpdate::All` for the table, `Some(&[pid])` for liveness).
- **Matching an agent to its registry entry across a login shell.** `agent.pid` is the PTY child, which with
  `shellMode: login|auto` is the shell wrapping `claude`; the registry pid is `claude` itself. Tier 2 therefore also
  accepts descendants of the agent pid (process table read at most every 10 s, only for unmatched agents). A brand-new
  agent can be unmatched for a few seconds (no session id yet, stale process table); it is `working/idle` by output until then.
- **An agent whose very first poll already says needs-you has an `appeared` event, not a `status` one**, so it does not
  notify (Hydra's rule). It cannot really happen: an agent starts, then waits.
- **The tracker never writes to disk inside a poll**; the history store buffers lines and the daemon flushes every 5 s
  (and on shutdown). Delta point lines list keys in sorted order (Hydra: insertion order); readers do not care.
- **Snapshot tests must not depend on the machine's time zone**: the Overview reads `OverviewState::clock()`; tests set
  `clock_override` (UTC, fixed `now`). In production it is `Local` and `Utc::now()`.
- **Daemon tests**: the Overview loop starts a poll at once and pushes an `OverviewSnapshot` to every connected client
  (even an empty one), so a raw-socket test that reads "the next frame" must skip events. `DaemonOptions::overview_config_dirs`
  keeps tests away from the real `~/.claude*/sessions`.
- **Manual checks write into the real Claude profile.** New agents create `<profile>/projects/-tmp-...` session files, add
  lines to `<profile>/history.jsonl` and trust entries (`projects["/tmp/..."]`) to `<profile>/.claude.json`; only the project
  folders are safe to delete afterwards.

## Verifying: never pipe `cargo test` into grep for the pass/fail decision
A pipe hides cargo's exit code, and a failing test binary stops the run before later crates' tests
execute. Run `cargo test --workspace -q > log 2>&1; echo $?` and read the exit code; grep the log only
for the summary.

## Raw-socket tests must skip pushed events (P6)
Since P6 the daemon broadcasts `OverviewSnapshot`/`OverviewEvents` to every client, raw connections
included, so a test reading "the next frame" can get an event instead of its response. Use the
`next_response` helper in `crates/as-daemon/tests/daemon.rs`.

## P7 (usage & limits) gotchas

- **`reqwest` 0.13 feature names.** `default-features = false, features = ["blocking", "json", "rustls"]` is the OpenSSL-free set
  (`rustls` = rustls + aws-lc-rs + the platform verifier). `rustls-no-provider` would need a crypto provider installed by hand.
  `cargo add --features` merges with what is there, so check `Cargo.toml` after it. The blocking client is created lazily on the
  first request and must be used from a blocking thread (`spawn_blocking`), never from async code. aws-lc-sys needs a C compiler
  (fine on Linux/macOS; on Windows MSVC it may want NASM or cmake): switch to the `ring` provider if that bites.
- **Never let a credential reach a formatter.** `credentials::Secret` has a redacted `Debug`; the HTTP request keeps the bearer in
  one, errors from the transport carry no text (`HttpError` is a unit struct) and tests assert the token is absent from every
  `Debug`/JSON of results, the cache and the logs. Keep new code that touches the token behind `Secret::expose()`.
- **The OAuth usage endpoint answers both shapes.** `limits[]` (`kind`, `percent` integer 0-100: `1` is 1 %, a float strictly in
  (0,1) is a fraction) and the legacy `five_hour`/`seven_day*` keys; parse `limits[]` first, legacy only when it yields nothing.
  `~/.claude` with an old `expiresAt` simply reads "Sign-in expired" until a Claude agent runs in that profile (we never refresh).
- **Usage rows are per profile, ordered like `list_profiles`.** With `CLAUDE_CONFIG_DIR` set (as in this repo's own shell), that
  profile comes right after the default one. Not a bug.
- **Daily totals are in the local zone; compare with the same zone.** A cross-check against `jq`/python must bucket by local
  midnight (`astimezone()`), dedupe by `message.id:requestId` and skip `<synthetic>`; finished days then match to the token. Today's
  figure keeps moving while a session writes logs.
- **Hash a path for the Keychain service the way Claude Code does:** `Claude Code-credentials-<first 8 hex of sha256(configDir)>`
  for non-default profiles (the default profile has no suffix).
- **Snapshot tests of the usage overlay** share the Overview tests' fixed clock (`overview_tests::NOW`, UTC); fixtures live in
  `as_tui::usage::model::fixtures` (`cfg(test)`). New insta snapshots land as `.snap.new`: review, then run once with
  `INSTA_UPDATE=always` and delete leftover `.snap.new` files.
- **Daemon tests must pin the usage sources.** `DaemonOptions::usage_options` (sources, profiles, http, credentials) keeps tests off
  the real `~/.claude*`, `~/.codex` and the network; leave `sources: Some(vec![])` in new test harnesses.

## Test portability pitfalls (CI runs Linux, macOS and Windows)

- **No `/tmp/...` as a real directory.** A PTY/process spawn with a missing cwd fails on Windows. Use
  `std::env::temp_dir()` or `tempfile` whenever the path is really used (strings in pure-logic tests are fine).
- **Compare paths as `Path`/`PathBuf` (or their components), never as `/`-joined strings.** `h.join(".claude/projects")`
  keeps a `/` on Windows, so `to_string_lossy()` comparisons differ; build with `.join(a).join(b)`.
- **A failed spawn is not an error on Windows.** The manager retries through `cmd.exe`, which starts fine and exits
  non-zero: the agent becomes Errored a moment later. Tests must wait for the status, not read it at creation.
- **Named pipes stay "busy" while a server-side instance is open.** Drop in-process `DaemonClient`s before probing or
  stopping a daemon in a test, and wrap daemon lifecycle tests in `tokio::time::timeout`. `pid_alive` is always true on
  Windows (the socket probe decides), so a stale lock is detected by the failed connect only.
- **Events are broadcast, not replayed.** A test that waits for a notification must subscribe before the cause: the
  daemon's startup scan can fire the alert before the client connects (flaky `usage_limit` test). Start with the feature
  off, subscribe, then enable it (the hot-reloaded config re-evaluates).
- CI jobs have `timeout-minutes: 25` so a hung test fails fast.

## P8 (MCP manager + headless) gotchas

- **rmcp 3.8 API.** `#[tool_router]` on the impl and `#[tool_handler]` on `impl ServerHandler` (no `tool_router` field needed any more);
  `StreamableHttpService::new(factory, Arc::default(), StreamableHttpServerConfig::default().with_cancellation_token(..))` is a tower
  service you mount with `axum::Router::nest_service("/mcp", ..)` (axum is not pulled in by rmcp: add `axum` with `tokio` + `http1`).
  The config is `#[non_exhaustive]`: use the `with_*` builders. Server info is `ServerConfig::new(caps).with_server_info(..)`. Clients:
  `ClientConfig::default().serve_with_lifecycle(StreamableHttpClientTransport::from_config(cfg.auth_header(token)), ClientLifecycleMode::Auto{..})`
  (dev-dependency features `client` + `transport-streamable-http-client-reqwest`). Hosts other than loopback get 403 by default.
- **Claude asks to approve a project `.mcp.json` server at start**, and the daemon-typed first prompt (+ Enter) answered that dialog
  ("continue without") so the manager never saw the tools. The manager workspace therefore gets `.claude/settings.local.json` with
  `enabledMcpjsonServers: ["studio"]`. Verified by hand: Claude calls `studio_list_agents` ("Called studio") with the bearer header.
- **`claude -p --output-format stream-json` needs `--verbose`** (Claude Code 2.1.x: "When using --print, --output-format=stream-json
  requires --verbose", exit 1). Hydra's argument list is outdated; `build_headless_args` adds it. The stderr of a failing run is kept
  in the log and in the run's `error`, which is how this was found.
- **Cancel must reach the tree.** The headless child is its own process group (`process_group(0)`) so `killpg` stops the login-shell
  wrapper and `claude` with its children; Windows uses `taskkill /PID <pid> /T /F`. Pipes held open by a grandchild would block the
  reader tasks forever: after the exit the readers get 2 s.
- **A UI test that scrolls needs content taller than the window.** `LogView::up` is clamped to what does not fit, so scroll tests use
  a short terminal. Snapshot tests of the panel use `overview_tests::NOW` (UTC clock) like the Overview ones.
- **Manual checks create real Claude files**: the manager/headless runs trusted `/tmp/as8*-manager-workspace` and `/tmp/as8w` in
  `<profile>/.claude.json`, added lines to `history.jsonl` and `projects/-tmp-as8*` session folders; only the project folders are safe to delete.

## Windows: ConPTY needs a cursor-position reply; interprocess' tokio connect can block forever

- **A ConPTY child never runs until the terminal answers `ESC [ 6 n`.** conhost emits a DSR query on startup and holds
  the child; with nobody replying, `wait()` never returns and the agent stays Running forever (no output, no exit).
  `NativePtyBackend` wraps the reader (`DsrResponder`) and answers `ESC [ 1 ; 1 R` on Windows. Diagnose with a tiny
  spawn test that logs raw reads: the first read is exactly `"\x1b[6n"`.
- **interprocess' tokio named-pipe connect has no timeout, and `ConnectWaitMode` is ignored by its `local_socket`
  wrapper.** While the pipe is busy (no listening instance, e.g. a stopped daemon whose last server instances still
  linger) it parks a `spawn_blocking` thread in `WaitNamedPipe(INFINITE)`. A `tokio::time::timeout` cannot cancel that
  thread and the runtime's shutdown (end of a `#[tokio::test]`) waits for it forever: the test hangs after all
  assertions passed and the in-test timeout never fires. On Windows `transport::Conn` is therefore tokio's own
  `NamedPipeClient` with a bounded async retry on `ERROR_PIPE_BUSY`.
- Debugging a CI hang: run one crate per step with `timeout-minutes` + `continue-on-error` (not `cargo test | tail`, a
  killed step loses piped output), add stage `eprintln!`s and a watchdog thread that `exit`s after 40 s.

## P9 git gotchas

- **Parse `status --porcelain=v2 --branch -z`, not v1.** `-z` disables quoting (spaces/unicode paths arrive raw) and a rename
  record is followed by a *separate* NUL-terminated original path. Use `--untracked-files=all` so untracked files are listed
  one by one (a directory entry cannot be staged/committed per file).
- **Run git with `GIT_LITERAL_PATHSPECS=1` and `-c core.quotepath=off`.** Otherwise file names with `*`/`?` are globbed and
  non-ASCII names in `diff` headers come out as `"\303\251"`. Diffs also pass `--src-prefix=a/ --dst-prefix=b/` so a user's
  `diff.noprefix` cannot break the parser. `GIT_OPTIONAL_LOCKS=0` keeps read-only `status` from touching `index.lock`.
- **`git commit -- <paths>` needs `--only` semantics and known paths.** `GitRunner::commit` runs `add -- files` first, then
  `commit --only -- files`, so other staged files stay staged; an empty list commits just the index (Hydra's add-all is `commit_all`).
- **`git diff` never shows untracked files.** A per-file diff of an untracked file uses `diff --no-index -- /dev/null <file>`
  (exit code 1 means "differences", accepted) so the diff view works for new files.
- **`worktree remove` without `--force` is the safety net.** git refuses when the worktree has modified or untracked files, so
  the agent manager never deletes uncommitted work; the branch is deleted with `-d` (kept if it has unmerged commits).
  Hydra used `--force` + `branch -D` from *inside* the worktree; we run it from the main worktree (`main_worktree`).
- **`.worktrees/` is added to `.git/info/exclude`** of the repository (Hydra put worktrees there without ignoring them, which
  showed up as untracked noise in the main repo).
- **Tests: isolate git with env, not `-c`.** `GIT_CONFIG_GLOBAL=<nonexistent>`, `GIT_CONFIG_NOSYSTEM=1` and
  `GIT_CONFIG_COUNT/KEY_n/VALUE_n` (`commit.gpgsign=false`, `init.defaultBranch=main`) work for every command including
  `clone`. On macOS the temp dir is a symlink (`/var` -> `/private/var`) and `rev-parse --show-toplevel` returns the real
  path: compare with `fs::canonicalize` on both sides.
- **Child pipes can outlive git.** A grandchild (ssh, credential helper) may keep stdout open after git exited, so the runner
  waits at most 2 s for the readers instead of joining them; on timeout the child is killed and the readers are abandoned.

## P9 (panels, terminals, settings) gotchas

- **Modals swallow the prefix.** Inside Git/Files/Settings panels `Ctrl+A t` is not a command: `Ctrl+A` is ignored and `t` is a panel
  key. During a manual check, keystrokes meant for a free terminal ended up in the Files panel and, once it closed, in the agent
  (they answered Claude's trust dialog). Look at the footer (`FILES`, `GIT`, ...) before typing; `q` closes every panel.
- **Free terminals are not agents.** They have their own `PtyBackend` use (`terminals::TerminalManager`), their own subscription key
  (`term:<id>` in the same per-connection set) and no run ids: the pump (`agents::pty::pump`) reports the exit once the reader had
  500 ms to drain. Since P10 the manager uses the same threads through `pty::pump_with_eof` (its callbacks carry the run id and call `on_eof`).
- **Subscribe is subscribe-then-snapshot for terminals.** A chunk arriving in the gap shows up twice (in the buffer and as an event);
  the other order would lose it. Agents avoid this with the dispatcher task; shells were not worth the extra machinery.
- **Panel replies are keyed strings.** `Effect::Req { key }` -> `Msg::Reply { key }`: stale answers (a diff for a file the cursor left)
  are dropped by a sequence number in the key (`git:diff:<n>`, `fs:file:<n>`, `fs:search:<n>`). New requests should do the same.
- **A ratatui `Terminal` smaller than `app.screen` panics in tests.** `App::on_resize` first, then draw with the same size.
- **Case-insensitive sort puts `blob.bin` before `README.md`.** `fs::list_dir` sorts dirs first, then by lowercase name.
- **`git panel dir` = the agent's `project_dir`.** For a worktree agent that already is the worktree path; the main checkout is
  `git worktree list`'s first entry (`GitRunner::main_worktree`).
- **Manual checks create real Claude files** again (trust entries for `/tmp/as9repo/.worktrees/...` in the profile's `.claude.json`,
  `history.jsonl`, `projects/-tmp-as9repo-*`): only the project folders are safe to delete.

## P10 gotchas

- **Rust ignores SIGPIPE: `println!` panics on a closed pipe.** `agent-studio status | head -1` died with "failed printing to stdout".
  One-shot CLI output goes through the `outln!`/`out!` macros of `src/out.rs` (BrokenPipe = quiet exit 0, other errors = message + exit 1).
  Never add a bare `println!` to the one-shot commands.
- **The update check reuses the usage `HttpClient`, with an empty bearer.** `ReqwestHttp` skips `Authorization` when the bearer is empty.
  GitHub needs a `User-Agent`; `404` on `releases/latest` means "no release yet", not an error. A test that builds a `DaemonOptions`
  literal must set `updates: false` (or inject `update_http`), otherwise the daemon would call GitHub 10 s after the start.
- **`cargo install` is not a published crate yet.** The hint for a cargo install says `cargo install --git <repo> --locked --force`,
  matching the README. Switch it to `cargo install agent-studio --locked` when the crate is on crates.io.
- **`[profile.release]` keeps `panic = "unwind"`.** Background tasks of the daemon and the PTY threads must not abort the whole process on a
  panic in one of them. `strip = true` removes symbols: a crash report from a release binary has no function names.
- **macOS x86_64 is a cross build on the arm runner** (`macos-latest`), so the release workflow does not run that binary (no smoke test).
  `macos-13` (the old Intel runner) no longer exists. The Linux arm build uses the free `ubuntu-24.04-arm` runner.
- **Release assets are named `agent-studio-<tag>-<target triple>.tar.gz|zip`.** `as_core::updates::select_asset` matches on the triple and
  the extension: change both together.
- **The prefix in panels is handled before the panel sees the key** (`App::modal_prefix`), so `Ctrl+A` is never a text-editing key in the
  panels that do not take text; panels that do (git prompt, file search, Settings edit, headless search) are excluded by `modal_takes_prefix`.
  A new panel with a text field must be added there.
- **Footer hints are built from `Keymap::hint_for(id)`**, which returns the first label of a command (`Ctrl+A g`, `sidebar j`, `grid x`).
  Scope words are stripped for hints that already say where they apply. New hints should use catalog ids, not hard-coded letters.
- **Toast height depends on the wrapped body** (`ui::toasts::height_for`, at most 6 body lines + ellipsis). Stack math must call it,
  not a constant.
- **More Claude first-run dialogs in the `TrustGate`**: MCP approval ("New MCP server found in .mcp.json", "Use this and all future MCP
  servers", "Continue without using this MCP server") and the API key prompt ("Detected a custom API key", "Do you want to use this API
  key?"). Phrases were taken from Claude Code's current wording, not from a live capture of every variant: if Claude rewords a dialog the
  startup input is no longer held (the symptom is the dialog answered by the delayed `/rename`). Add the new phrase to `DIALOG_PHRASES`.
- **Manual checks of the update banner** need a local server: `AGENT_STUDIO_UPDATE_URL=http://127.0.0.1:<port>/latest` with a JSON file
  (`tag_name`, `html_url`, `draft`, `prerelease`, `assets`) served by `python3 -m http.server`. Never fake data against api.github.com.

## Known-gaps round (side-by-side diff, syntect, terminal split tree, terminals.json)

- **syntect features.** `default-features = false, features = ["default-syntaxes", "default-themes", "regex-fancy"]` is the whole set (no
  `onig`, no C, nothing extra for Windows); `default-syntaxes` already pulls `parsing` + `dump-load`. The embedded default syntaxes are
  Sublime's old set: **no TOML, TypeScript, Kotlin, Dockerfile**. `highlight::imp::alias` maps `ts/tsx/jsx` to JavaScript, `toml/ini/cfg` to Java
  Properties, `kt` to Java ...; add to it instead of bundling more syntaxes. Use the `*_newlines` syntax set and feed lines with a trailing `\n`
  (strip it from the spans again), otherwise rules that anchor on `$` misbehave.
- **`HighlightLines` borrows the syntax set and theme**: keep both in a `static OnceLock` (`highlight::imp::assets`, loaded by `warm()` on a
  thread when the Git/Files panel opens, ~50-100 ms) so the highlighter is `'static` and can live in the panel state. It is neither `Clone` nor `Eq`:
  `LazyLines` / `DiffHighlight` wrap it with a "clone = empty cache, eq = always" impl so the panel structs keep deriving both.
- **Highlighting is lazy and time-boxed.** At most ~30 ms per frame (`FRAME_BUDGET`); when it runs out the renderer sets `app.dirty` and the next
  tick (`FRAME_INTERVAL`) continues. A test that asserts on colors must draw until `app.dirty` stays false (`settle()` in `panel_tests`), a debug
  build is slow enough to split the work. Always process at least one line per call or a slow machine never finishes.
- **Feature unification hides the no-feature build.** `cargo test --workspace` builds `as-tui` with `highlight` (the root crate's default),
  `cargo test -p as-tui` without. Tests that need syntect are `#[cfg(feature = "highlight")]` and a plain-text twin asserts the other case; CI also
  runs `cargo clippy -p as-tui --all-targets` for the no-feature build.
- **Diff rows keep both layouts.** `DiffView.rows` (unified) is the source, `split` is derived (`pair_rows`) once; `scroll`, `max_scroll`,
  `next_hunk` follow the *active* layout (`split_active`), which the renderer sets every frame from `diffLayout` and the panel width, remapping
  `scroll` so the same code stays on top. Code that reads `diff.hunks` directly is unified-only; use `hunk_rows()`.
- **Theme gained `diff_add_bg` / `diff_del_bg`.** A struct literal of `Theme` (none today) needs them; snapshots are text only, colors are
  asserted on buffer cells (`fg_colors_of_row`, `bg == theme.diff_*_bg`).
- **Free-terminal ids are new after a restart.** Restoring (`TerminalManager::restore_project`) spawns fresh shells, so pane ids and tab ids are
  regenerated (`grp-<n>` from the process-wide counter, or a restored `grp-3` would collide with a new tab). Compare layouts across a restart by
  shape (`map_panes` to the reading-order index), never by id.
- **Saving the layout must stop before the shells are killed.** `kill_all` at shutdown removes every pane from the layout and each removal would
  schedule a save of the *empty* layout. `Daemon::shutdown_services` writes `terminals.json` first and sets `terminals_frozen`; `persist_terminals`
  is a no-op afterwards. `export()` also returns the saved layouts of projects whose shells were never respawned, otherwise the first save after a
  restart would drop every project the user did not open.
- **Restoring spawns PTYs, so it runs on a blocking thread** (`GetTerminalLayout` handler, `spawn_blocking`). The TUI asks for the layout when the
  panel opens, which is the "first use" trigger; nothing is spawned at daemon start. `freeTerminalMaxCount` caps the *restore* regardless of the policy.
- **A daemon-level restart test starts the second daemon in a fresh data dir** (copying `terminals.json`), not on the same one: a named pipe
  that was just closed can stay busy on Windows (see the ConPTY / named pipe entry above).
- **`prefix Up/Down` were taken** (`next-agent` / `prev-agent`), so pane focus is `prefix Alt+arrows` and resize `prefix Ctrl+arrows`. Some terminals
  (macOS Terminal.app, tmux without `xterm-keys`) swallow modified arrows: the palette entries and `keybindings.json` rebinding are the way out.

## GUI (Tauri) G0 gotchas

- **Tauri runs `beforeDevCommand`/`beforeBuildCommand` with the cwd = the parent of the dir holding `tauri.conf.json`** (here `crates/`),
  so the commands are `npm --prefix ../gui run ...`, not `../../gui`. And the Tauri CLI finds the config from the working directory: run
  `../../gui/node_modules/.bin/tauri dev` from `crates/as-gui`; `npm run tauri -- dev` from `gui/` says "no app found".
- **A plain `cargo build` of the shell loads `devUrl`, not the embedded `gui/dist`** (no `custom-protocol` feature). To check the embedded build
  and its CSP: `cargo build --features tauri/custom-protocol` with no Vite server running (a leftover Vite on :1420 silently fakes success).
- **Tauri test binaries do not start on Windows** (missing common-controls manifest). Keep unit-testable code in the crate's lib without
  `tauri`, put Tauri glue in the bin and set `test = false` on the bin.
- **`cat > file` with no heredoc waits for stdin forever** and the tool call hangs for minutes: check the command, not npm.
- **Hydra's renderer assumes Electron's `File.path`** for dropped files; webviews have none (typed as optional in `gui/src/env.d.ts`).
  OS drops need Tauri's drag-drop event (G2).
- **Under Xvfb** set `WEBKIT_DISABLE_DMABUF_RENDERER=1`; Pillow's `ImageGrab.grab(xdisplay=None)` takes the screenshot (no xdotool/import needed).
- **ESLint 9 + react-hooks 7** flags Hydra's Tooltip ref forwarding (`react-hooks/immutability`); the rule is off in `gui/eslint.config.mjs`.

## GUI (Tauri) G1 gotchas

- **Concurrent Tauri `invoke`s are not ordered.** Keystrokes sent as separate `daemon_request` calls can reach the daemon out of order, so the
  bridge chains `sendInput`/`sendRawInput`/`resizeAgent` per agent (`serial()` in `daemonApi.ts`) instead of firing them in parallel.
- **Hydra's `useAgents` merges the buffer with live chunks by suffix/prefix** (`mergeBufferedOutput`): `getAgentBuffer` must return the *whole*
  daemon buffer (we re-`Subscribe` each time), not just what was streamed since; otherwise the history is dropped. The daemon's `Subscribe`
  is idempotent for a connection.
- **The daemon's config lacks `terminalFontFamily/FontSize/EnableWebgl`.** `GetConfig` fills Hydra's defaults in the bridge; changes to those three keys
  go to `localStorage` (G2: move them into `AppConfig`). Global YOLO: the daemon toggles the agents but Hydra also stores `globalYolo` in the config, so the bridge writes it.
- **Hydra has no "agent list changed" event** (its renderer only sees agents it created). The bridge adds `onAgentsChanged` (from `agents_changed` and
  from a reconnect) and `useAgents` reloads the list; without it CLI/TUI-created agents never appear.
- **Initial render vs daemon start.** `installHydra` waits up to 4 s for `daemon-status: connected` before rendering (the shell starts the daemon in the
  background); if it was down, `DaemonBanner` reloads the page once when it comes up so every hook loads real data.
- **The GUI reconnect restarts a stopped daemon** (`ensure_daemon`, like the TUI): `agent-studio stop --force` while the GUI is open brings a new daemon up
  within a second. Close the GUI first when you want it to stay down.
- **Browser dev bridge**: unix socket paths are short-limited, use `--home /tmp/asXX`; Playwright with system Chrome needs `--no-sandbox` here;
  `getByRole('button', {name: 'Grid'})` did not find the header buttons (use `locator('button', {hasText: /^Grid$/})`); the xterm text is in `.xterm-rows`.
- **A repeated trust prompt is not shown for a folder that was trusted before**: e2e scripts must not wait for it unconditionally. Real `claude` agents in a
  test folder create `~/.claude*/projects/-tmp-asg1-proj` (+ `history.jsonl` lines and a trust entry in `<profile>/.claude.json`); only the project folder is safe to delete.
- **tokio-tungstenite 0.28**: `Message::text(String)`, `Message::Text(Utf8Bytes)` (`.as_str()`); the handshake callback's `Err(ErrorResponse)` trips
  clippy `result_large_err` (allow it on the closure).


## GUI (Tauri) G2a gotchas

- **`DaemonBanner` reloaded the whole page on *every* reconnect** (it set its "rendered without daemon" flag on any disconnect, contradicting its own comment),
  so a daemon restart threw away open panels and terminals. Only a page that *started* without a daemon reloads now (`everConnected`); after a drop the bridge
  resubscribes and announces fresh terminal layouts. Check reconnect behaviour with `window.__x` markers in the page: they vanish on a reload.
- **Monaco keeps the command registered at mount.** `CodeEditor` registered `onSave` once, so Ctrl+S in the editor saved the *old* content of the first file
  (Hydra has the same bug; the App-level Ctrl+S handler is swallowed by Monaco). Call the latest handler through a ref (`onSaveRef`).
- **Double-tapping Ctrl opens the shortcuts dialog** (Hydra feature, 350 ms). Browser automation that presses `Control+End` then `Control+s` quickly triggers it:
  wait ~600 ms between Ctrl combos.
- **The editor tree (Pierre) lives in a shadow DOM**: Playwright `getByText` does not find its rows; click by coordinates or use the bridge from `page.evaluate`.
- **`Path` drops a trailing `.`** (`/x/.` -> `/x`), so `list_dirs` splits the typed text at the last separator instead; the typed prefix `.` (dot folders) works.
- **A dangling symlink pointing outside the project passes `resolve_safe`** (nothing to canonicalize, it falls back to an existing ancestor). `fs::write_file`
  therefore resolves symlinks itself and refuses anything that does not end inside the project; writes follow inner symlinks (the link stays) and keep the file mode.
- **`write_atomic` touches the directory** (temporary file + rename): the polling watcher reports a `rename` for the folder after every save. Harmless
  (the tree reloads once); do not "fix" it by ignoring directory events.
- **Terminal output has no project in its event**: the bridge remembers terminal id -> project from the layouts it sees (`onFreeTerminalOutput(id, projectDir, data)`).
  A scratch project (the data dir) hosts the preflight test terminal; its layout events are dropped so it never shows up in a panel.
- **Tauri plugin versions**: `tauri-plugin-opener` resolves to `2.x` only when pinned (`cargo add tauri-plugin-opener@2`; the newest on crates.io is a 3.0 alpha).
  Plugin permissions are checked when the shell is built: a typo in `capabilities/default.json` fails `cargo build`, but whether a dialog really opens is only visible in a real session.
- **Hydra `gitCommit` without files means `git add -A`**; the daemon's `GitCommit { files: [] }` commits only what is staged. The bridge stages everything not fully staged first.
- **E2E setup**: a fake `claude` script (`--version` + `exec cat`) first in `PATH` of the dev bridge and `{"terminalShellMode":"direct"}` in the temp home's
  `config.json` give a running agent without a real Claude session; `agent-studio --home H new -p <repo>` creates it, and the editor/git panels need one.

## GUI (Tauri) G2b gotchas

- **Oddity (a), "garbled terminal after a restart": a restart that does not start in the UI left the old session on screen.** The daemon clears the agent's
  buffer on `restart`, but the renderer only cleared its own copy in `useAgents.restartAgent` (the restart button). Restarts from the CLI/TUI/MCP, YOLO and
  model switches and global YOLO produced no event at all (only `agent_status: running`, no `starting`), so the xterm kept the old screen and the new
  process drew below it (inline TUIs such as Claude use relative cursor moves, so the result looked garbled). Reproduced before the fix with a fake `claude`
  that draws text without clearing the screen: after `agent-studio restart <id>` and after `toggleYolo` the UI showed `RUN 10 / RUN 11 / RUN 12` stacked while a fresh page
  (daemon buffer replay) showed only the last run; after the fix both are identical (3 restarts x CLI/YOLO). Not the cause: replay at another size (the manager respawns with the
  last known PTY size) and the buffer (cleared). **Fix**: `AgentEvent::Restarted` -> `Event::AgentRestarted { agent_id }` sent before the new process writes; the bridge turns it
  into `onAgentRestarted`, `useAgents` clears the output (and the queued old tail), the TUI blanks its emulator. A marker in the output stream (e.g. a reset sequence) was rejected:
  the daemon's documented "Subscribe may repeat the last chunk" window could replay it over a freshly loaded buffer; a separate event is not replayed.
- **TerminalPane stayed blank after a setting recreated its xterm** (font size/family, cursor, WebGL): the output effect only runs when `rawOutput` changes, so the new empty terminal
  waited for the next chunk (forever for an idle agent). The recreation now flushes the output it already has (`flushOutputRef`).
- **Oddity (b), "another agent's `/rename ...` line in a terminal": not reproducible on the current tree.** Checked: the daemon (two agents created and renamed concurrently, same and different
  project folders: each buffer holds only its own `/rename`), 150 rounds of concurrent `Subscribe` of two agents through the dev bridge (0 mixed buffers, compared by pid), the bridge's
  per-agent streams (regression test with crossing answers), `useAgents` (regression test), and the UI (4 rounds of creating two agents and switching between them while they start, renaming the
  other agent while one is selected: no foreign line). The most likely explanation of the G1 observation is oddity (a): output of an earlier session stays on screen under the next one,
  which looks like a foreign line when the same fake `cat` agent is restarted. If it shows up again: compare `getAgentBuffer(id)` of both agents first (bridge/daemon) and the
  xterm's text second (`.xterm-rows`); `TerminalPane` is keyed by agent id in `ChatView`, but not in every caller.
- **The browser dev bridge has no shell**: `RawTransport.shell` of the WebSocket transport only knows `log_event`. `open_in` / `list_open_targets` therefore stay daemon requests there, and in e2e runs
  the daemon (not the browser) starts the opener: put a fake `xdg-open` first in the daemon's `PATH`, otherwise a test run opens a real file manager/editor.
- **`CloseRequested` is async here**: the decision needs the daemon, so the handler calls `prevent_close()` and finishes with `app.exit(0)` itself (`as_gui::quit::decide_close` is the pure part).
  Under Xvfb there is no window manager and no `xdotool`/`wmctrl`: a close is a `WM_DELETE_WINDOW` ClientMessage sent with Xlib (ctypes works) and clicks go through XTest.
- **`agent-studio <cmd>` auto-starts a daemon**: after `Quit All` a later `agent-studio status` in the same shell started a *new* daemon, with the shell's real `HOME` (it imported the real Claude
  sessions as idle agents into the temp home). In e2e scripts run every CLI call with the same fixture environment as the app, and stop the daemon last.
- **`gui-keybindings.json` is not created at start** (Hydra writes its file at startup and rewrites it to append new defaults, which also rewrites a user's edits): it is created from the defaults when the
  user opens it. Hard links (the atomic create) fail on some file systems (FAT/exFAT, some network shares): `create_template` then falls back to an exclusive create.
- **Claude user skills cannot be toggled** (`group: "user"`): Claude Code has no per-skill switch, the daemon's `toggle` refuses it and the panel snaps the checkbox back (Hydra wrote a meaningless entry into `enabledPlugins`).
- **Update hint vs Hydra's install methods**: Hydra's `UpdateInstallMethod` is `brew | direct | unknown`; cargo/winget/scoop/chocolatey map to `unknown` (the "Download" button) and the real instruction travels in the
  extension field `upgradeHint`, which `UpdateBanner`/`UpdatePanel` show. Mock server check: `AGENT_STUDIO_UPDATE_URL=http://127.0.0.1:PORT/latest` (the daemon reads it at start).
