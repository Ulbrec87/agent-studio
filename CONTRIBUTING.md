# Contributing

Thanks for helping. Agent Studio is a desktop app (Tauri shell + React front end in `gui/`) on top of a Rust workspace; start with
`docs/README.md` for the map of the docs and `docs/architecture.md` for how the processes and crates fit together.

## Build, test, lint

Needs Node 22 and Rust; on Linux the WebKit libraries (`libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev
librsvg2-dev libsoup-3.0-dev`). Build the front end first: Tauri embeds `gui/dist` at compile time, so every Rust command needs it.

```bash
cd gui && npm ci && npm run build                        # front end (npm run dev = browser with a mock bridge)
npm run typecheck && npm run lint && npm test            # front-end checks (still in gui/)
cargo build                                              # debug build of the workspace
cargo test --workspace                                   # all Rust tests
cargo fmt --all --check                                  # formatting
cargo clippy --workspace --all-targets -- -D warnings    # lints, must be clean
cd crates/as-gui && ../../gui/node_modules/.bin/tauri dev                # native window, live front end
cd crates/as-gui && ../../gui/node_modules/.bin/tauri build --no-bundle  # release binary (target/release/agent-studio)
```

A change is done when the **front-end checks, fmt, clippy and the tests** pass. Check the exit codes (`cargo test ...; echo $?`) instead
of piping the output into `grep`: a pipe hides a failing test binary.

For manual runs use a throwaway data directory so you never touch your real daemon and config:
`agent-studio --home /tmp/as-dev` (or `AGENT_STUDIO_HOME=/tmp/as-dev`; keep the path short: Unix socket paths are limited to ~104 bytes).
A fresh home imports every Claude/Codex session on your machine as idle agents; put `{"importSessionsOnStartup": false}` into
`<home>/config.json` first. `agent-studio --dev-bridge --home /tmp/as-dev` runs the daemon plumbing without a window for browser tests.

## Workflow

- **Feature branches and pull requests** for everything (`feat/<name>`, `fix/<name>`, `docs/<name>`, `refactor/<name>`); never push to `main` directly.
  (The initial port, phases P0 to P10, was committed straight to `main`; that is over.)
- **Conventional Commits**: `feat(core): ...`, `fix(gui): ...`, `docs: ...`, `test: ...`, `ci: ...`, `chore: ...`. One logical change per commit.
- CI (`.github/workflows/ci.yml`, job `check (<os>)`) runs the front-end checks, fmt, clippy and the whole test suite on **Linux, macOS and Windows**. All three must be green. You
  cannot run Windows locally, so read `docs/AGENTLOG.md` ("Test portability pitfalls", "Windows: ConPTY ...") before touching PTY, socket,
  path or process code, and before writing tests that spawn processes or compare paths.
- Releases: see `docs/releasing.md`.

## Conventions

- Cross-platform: no Unix-only API without a `cfg` alternative; paths through `as_core::paths`, never hard-coded `/` or `~`.
- `as-core` has no UI and no socket code; the dependency direction is `as-gui -> as-daemon -> as-proto -> as-core`.
- No `unwrap()`/`expect()` outside tests except for true invariants (say why in a comment). `anyhow` in binaries and daemon glue,
  `thiserror` for typed errors in `as-core`.
- Parse the files of other tools (`~/.claude*/...`, `~/.codex/...`) tolerantly: unknown or missing fields never panic or fail a whole scan.
- Logging with `tracing` only; the daemon and the app never print (the `--dev-bridge` announcement line is the only stdout output).
- Reuse a crate over hand-rolled code when it does not add heavy bloat. Git and PRs shell out to `git` / `gh`.
- Tests: pure logic ported from Hydra comes with its tests; front-end code is tested with Vitest (`gui/`); anything that waits for threads polls
  (`wait_for`), never sleeps for a fixed time; tests never use the network, the real `~/.claude*` or the real daemon.
- User-facing text in English; MCP tools are named `studio_*`.

## Docs you must keep in sync

- `docs/porting-map.md`: when you port or change something from Hydra, update its row (`done` / `dropped` / `deferred` with a one-line
  reason; no stale `todo`).
- `docs/architecture.md`: protocol additions (requests, responses, events), data files, crates.
- `docs/AGENTLOG.md`: append a short entry (title + what to do instead) when you hit something non-obvious.
- `README.md` for anything a user sees (install, config keys).
