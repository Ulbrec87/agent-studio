# Agent Studio — Project Instructions

Rust terminal UI that runs **Claude Code**, **Codex** and **OpenCode** CLI agents in parallel,
each in its own PTY, with a background daemon that keeps them alive when the TUI closes.
It is a port of **Hydra** (Electron, `~/PhpstormProjects/hydra`, MIT © jpdlr) — when in doubt about
behavior, read the Hydra source: it is the reference implementation.

## Read first

- `docs/plans/2026-11-04-porting-plan.md` — feature inventory, architecture, phases, **decisions (§8)**.
- `docs/architecture.md` — processes, crates, protocol, data files.
- `docs/porting-map.md` — Hydra file → Rust module, with status. **Update it when you port something.**
- `docs/AGENTLOG.md` — gotchas found so far. **Append to it** when you hit a non-obvious problem
  (short title + what to do instead).

## Commands

```bash
cargo build                         # debug build
cargo run -- <args>                 # run the binary (default: TUI)
cargo run -- daemon --foreground    # run the daemon in the foreground (dev)
cargo test --workspace              # all tests
cargo fmt --all --check             # formatting
cargo clippy --workspace --all-targets -- -D warnings   # lints, must be clean
cargo build --release               # release profile (thin LTO, stripped), what release.yml builds
cargo run -- import-hydra [--dir D] [-y]   # import Hydra config/agents/keybindings (read-only on Hydra's files)
cargo run -- update-check           # ask GitHub Releases for a newer version (no daemon needed)
```

A phase/step is done only when **fmt, clippy (-D warnings) and tests** all pass.

Desktop app (Node 22 via nvm; always `source ~/.nvm/nvm.sh && nvm use 22` first):

```bash
cd gui && npm ci && npm run dev         # browser only, mock bridge, http://localhost:1420
cd crates/as-gui && cargo run -- --dev-bridge --home /tmp/as-dev   # no window: prints `DEV_BRIDGE ws://127.0.0.1:PORT token=TOKEN`
#   then open http://localhost:1420/?bridge=ws://127.0.0.1:PORT&token=TOKEN (npm run dev running) = the UI against a real daemon
cd gui && npm run typecheck && npm run lint && npm test && npm run build
cd crates/as-gui && ../../gui/node_modules/.bin/tauri dev   # native window (needs gui/node_modules; gui/dist for plain cargo build)
cd crates/as-gui && cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test   # after gui build
```

The Tauri CLI is the `@tauri-apps/cli` devDependency of `gui` (no `cargo install tauri-cli`). Run it from `crates/as-gui`
(the CLI finds `tauri.conf.json` from the working directory; `npm run tauri` from `gui/` does not).

For dev, isolate state with `AGENT_STUDIO_HOME=/tmp/as-dev cargo run -- …` so you never touch the
real daemon/config in `~/.config/agent-studio`.

## Layout

```
gui/                   Vite + React 18 + TS front end (Hydra renderer, adapted); gui/src/bridge = window.hydra shim
crates/as-gui/         Tauri 2 shell, its OWN Cargo workspace (needs WebKit libs; excluded from the root workspace)
src/main.rs            bin `agent-studio` — clap: (default) tui | daemon | ls | new | send | kill | rm | restart |
                       status | stop | import-hydra | update-check | version; `oneshot.rs` runs the one-shot
                       commands, `out.rs` prints without panicking on a closed pipe, `logging.rs` sets up tracing
crates/as-core/        domain logic, NO UI and NO socket code (agents/PTY, providers, config,
                       workspace, sessions, profiles, overview, usage, headless, git, fs, skills, open_in,
                       terminals (free shells), hydra_import, keybindings catalog, updates)
crates/as-proto/       serde types for daemon <-> client messages (requests, responses, events)
crates/as-daemon/      local-socket server, lock file, lifecycle, MCP server (rmcp), notifications, update loop
crates/as-tui/         ratatui app: views, widgets, keymap, daemon client
docs/                  index (README.md), plans, architecture, key bindings, porting map, releasing, agent log
.github/workflows/     ci.yml (fmt, clippy, tests on 3 OSes), release.yml (tag `v*` / manual dry run)
```

Dependency direction: `as-tui → as-proto → as-core`, `as-daemon → as-proto → as-core`.
`as-tui` must never depend on `as-daemon` internals (only on the protocol).

## Rules

- **Cross-platform** (Linux, macOS, Windows). No Unix-only APIs without a `cfg` alternative.
  PTYs via `portable-pty`, IPC via `interprocess` local sockets (Unix socket / named pipe),
  paths via `as_core::paths`. Never hardcode `/` separators or `~`.
- **Reuse crates** over hand-rolled code when they don't add heavy bloat (see the plan §2.2).
  Git and PRs: shell out to `git` / `gh` (no libgit2).
- **File formats** of external tools (Claude `~/.claude*/projects/**/*.jsonl`,
  `~/.claude*/sessions/<pid>.json`, `~/.codex/sessions`, …) must be parsed **tolerantly**:
  unknown/missing fields never panic or fail the whole scan.
- No `unwrap()`/`expect()` in non-test code except for true invariants (comment why).
  Errors: `anyhow` in binaries/daemon glue, `thiserror` for typed errors in `as-core`.
- Logging with `tracing` only (no `println!` in the daemon; the TUI must never print to stdout
  while the alternate screen is active; one-shot CLI output uses `outln!`/`out!` from `src/out.rs`, never bare
  `println!`, so a closed pipe does not panic).
- Pure logic ported from Hydra's `shared/` comes **with its tests**: port the `*.test.ts` cases.
- UI rendering tests use ratatui's `TestBackend` (+ `insta` snapshots where useful).
- Keep modules small and focused; prefer one module per Hydra service.
- User-facing text in English. Names: tools exposed by the MCP server are `studio_*`.
- Config/workspace are **our own** files (fresh start). Hydra data is only read by the manual
  "Import from Hydra" action.

## Keys (TUI)

Prefix key **`Ctrl+A`** (configurable) like tmux. While a terminal is focused every other key goes to
the agent PTY; app commands are `prefix <key>`. Quitting with running agents asks:
*Quit + stop N agents / Detach (keep running) / Cancel*.

## Git workflow

- **The initial port (phases P0–P10 of the plan) is complete.** History note: it was committed and pushed
  directly to `origin/main`, one commit per phase. **From now on everything goes through feature branches and
  pull requests** (`feat/<name>`, `fix/<name>`, `docs/<name>`), never a direct push to `main`.
- Conventional Commits (`feat(core): …`, `fix(tui): …`, `docs: …`). CI must be green on Linux, macOS and Windows.
- When you port or change something from Hydra, update `docs/porting-map.md` in the same commit; releases follow
  `docs/releasing.md` (P11, remote control, stays deferred). See `CONTRIBUTING.md` for the full workflow.
