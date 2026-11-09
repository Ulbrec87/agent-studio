# Agent Studio — Project Instructions

Desktop app (Rust + **Tauri 2**, React front end in `gui/`) that runs **Claude Code**, **Codex** and **OpenCode** CLI agents in
parallel, each in its own PTY, with a background daemon (hosted by the same binary) that keeps them alive when the window closes.
It is a port of **Hydra** (Electron, `~/PhpstormProjects/hydra`, MIT © jpdlr) — when in doubt about
behavior, read the Hydra source: it is the reference implementation. There is **no terminal UI and no CLI**: the product is the
desktop app only (the TUI and CLI were removed, see `docs/plans/2026-11-09-gui-only-plan.md`).

## Read first

- `docs/plans/2026-11-04-porting-plan.md` — feature inventory, architecture, phases, **decisions (§8)**.
- `docs/architecture.md` — processes, crates, protocol, data files.
- `docs/porting-map.md` — Hydra file → Rust module, with status. **Update it when you port something.**
- `docs/AGENTLOG.md` — gotchas found so far. **Append to it** when you hit a non-obvious problem
  (short title + what to do instead).

## Commands

Node 22 via nvm; always `source ~/.nvm/nvm.sh && nvm use 22` first. Linux needs the WebKit dev libraries
(`libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev libsoup-3.0-dev`) for **every** Rust command,
and `gui/dist` must exist (`cd gui && npm ci && npm run build`) because Tauri embeds it at compile time.

```bash
cd gui && npm ci && npm run dev         # browser only, mock bridge, http://localhost:1420
cd gui && npm run typecheck && npm run lint && npm test && npm run build
UPDATE_TS_BINDINGS=1 cargo test -p as-proto --test ts_bindings   # regenerate gui/src/bridge/generated after changing a wire type
cargo build                             # whole workspace (debug)
cargo fmt --all --check                 # formatting
cargo clippy --workspace --all-targets -- -D warnings   # lints, must be clean
cargo test --workspace                  # all tests
cd crates/as-gui && ../../gui/node_modules/.bin/tauri dev            # native window (starts Vite itself)
cd crates/as-gui && ../../gui/node_modules/.bin/tauri build --no-bundle   # release binary: target/release/agent-studio
cd crates/as-gui && ../../gui/node_modules/.bin/tauri build --bundles deb # installers (appimage, deb, dmg, msi)
cargo run -p as-gui -- --dev-bridge --home /tmp/as-dev   # no window: prints `DEV_BRIDGE ws://127.0.0.1:PORT token=TOKEN`
#   then open http://localhost:1420/?bridge=ws://127.0.0.1:PORT&token=TOKEN (npm run dev running) = the UI against a real daemon
```

The Tauri CLI is the `@tauri-apps/cli` devDependency of `gui` (no `cargo install tauri-cli`). Run it from `crates/as-gui`
(the CLI finds `tauri.conf.json` from the working directory; `npm run tauri` from `gui/` does not).
A change is done only when **front-end checks, fmt, clippy (-D warnings) and tests** all pass (CI runs exactly this on 3 OSes).

For dev, isolate state with `--home /tmp/as-dev` or `AGENT_STUDIO_HOME=/tmp/as-dev` so you never touch the real
daemon/config in `~/.config/agent-studio`.

## Layout

```
gui/                   Vite + React 18 + TS front end (Hydra renderer, adapted); gui/src/bridge = window.hydra shim
crates/as-gui/         Tauri 2 shell, bin `agent-studio` (workspace member): window, `daemon` subcommand (hidden, hosts the
                       daemon), `--dev-bridge`, quit flow, native glue; `tauri.conf.json` (bundles)
crates/as-core/        domain logic, NO UI and NO socket code (agents/PTY, providers, config,
                       workspace, sessions, profiles, overview, usage, headless, git, fs, skills, open_in,
                       terminals (free shells), hydra_import, gui_keybindings, updates, shell_env)
crates/as-proto/       serde types for daemon <-> client messages (requests, responses, events), client + lifecycle
crates/as-daemon/      local-socket server, lock file, lifecycle, MCP server (rmcp), notifications, update loop
docs/                  index (README.md), plans, architecture, porting map, releasing, agent log
.github/workflows/     ci.yml (front end + fmt, clippy, tests on 3 OSes), release.yml (tag `v*` / manual dry run: Tauri bundles)
```

Dependency direction: `as-gui → as-daemon → as-proto → as-core` (the shell talks to the daemon through the protocol, and also
hosts it). The root `Cargo.toml` is a virtual workspace (shared package metadata, lints, `[profile.release]`).

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
- Logging with `tracing` only (no `println!` in the daemon or the app; the only stdout line is the `--dev-bridge` announcement).
- Pure logic ported from Hydra's `shared/` comes **with its tests**: port the `*.test.ts` cases.
- Front-end tests are Vitest (`gui/`); shell logic that can be tested without linking Tauri lives in the `as_gui` library part.
- Keep modules small and focused; prefer one module per Hydra service.
- User-facing text in English. Names: tools exposed by the MCP server are `studio_*`.
- Config/workspace are **our own** files (fresh start). Hydra data is only read by the manual
  "Import from Hydra" action.

## Git workflow

- **The initial port (phases P0–P10 of the plan) is complete.** History note: it was committed and pushed
  directly to `origin/main`, one commit per phase. **From now on everything goes through feature branches and
  pull requests** (`feat/<name>`, `fix/<name>`, `docs/<name>`), never a direct push to `main`.
- Conventional Commits (`feat(core): …`, `fix(gui): …`, `docs: …`). CI must be green on Linux, macOS and Windows.
- When you port or change something from Hydra, update `docs/porting-map.md` in the same commit; releases follow
  `docs/releasing.md` (P11, remote control, stays deferred). See `CONTRIBUTING.md` for the full workflow.
