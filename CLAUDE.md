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
```

A phase/step is done only when **fmt, clippy (-D warnings) and tests** all pass.

For dev, isolate state with `AGENT_STUDIO_HOME=/tmp/as-dev cargo run -- …` so you never touch the
real daemon/config in `~/.config/agent-studio`.

## Layout

```
src/main.rs            bin `agent-studio` — clap: (default) tui | daemon | ls | send | kill | version
crates/as-core/        domain logic, NO UI and NO socket code (agents/PTY, providers, config,
                       workspace, sessions, profiles, overview, usage, headless, git, skills, fs)
crates/as-proto/       serde types for daemon <-> client messages (requests, responses, events)
crates/as-daemon/      local-socket server, lock file, lifecycle, MCP server (rmcp), notifications
crates/as-tui/         ratatui app: views, widgets, keymap, daemon client
docs/                  plans, architecture, porting map, decisions, agent log
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
  while the alternate screen is active).
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

- **During the initial port (phases P0–P10 of the plan): commit and push directly to `origin/main`
  after each phase**, with Conventional Commits (`feat(core): …`, `docs: …`).
- **After the port is complete: feature branches + pull requests** (`feat/<name>`, `fix/<name>`),
  never push to `main` directly.
- Tick the phase in the plan's progress list and update `docs/porting-map.md` in the same commit.
