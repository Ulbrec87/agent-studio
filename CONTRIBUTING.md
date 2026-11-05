# Contributing

Thanks for helping. Agent Studio is a Rust workspace; start with `docs/README.md` for the map of the docs and
`docs/architecture.md` for how the processes and crates fit together.

## Build, test, lint

```bash
cargo build                                              # debug build
cargo run -- <args>                                      # run the binary (default: the TUI)
cargo run -- daemon --foreground                         # the daemon in the foreground (dev)
cargo test --workspace                                   # all tests
cargo fmt --all --check                                  # formatting
cargo clippy --workspace --all-targets -- -D warnings    # lints, must be clean
cargo build --release                                    # what the release workflow builds
```

A change is done when **fmt, clippy and the tests** pass. Check the exit codes (`cargo test ...; echo $?`) instead of piping the output
into `grep`: a pipe hides a failing test binary.

For manual runs use a throwaway data directory so you never touch your real daemon and config:
`AGENT_STUDIO_HOME=/tmp/as-dev cargo run -- ...` (keep the path short: Unix socket paths are limited to ~104 bytes). A fresh home imports
every Claude/Codex session on your machine as idle agents; put `{"importSessionsOnStartup": false}` into `<home>/config.json` first.
Stop the daemon afterwards (`agent-studio --home /tmp/as-dev stop --force`).

## Workflow

- **Feature branches and pull requests** for everything (`feat/<name>`, `fix/<name>`, `docs/<name>`); never push to `main` directly.
  (The initial port, phases P0 to P10, was committed straight to `main`; that is over.)
- **Conventional Commits**: `feat(core): ...`, `fix(tui): ...`, `docs: ...`, `test: ...`, `ci: ...`, `chore: ...`. One logical change per commit.
- CI (`.github/workflows/ci.yml`) runs fmt, clippy and the whole test suite on **Linux, macOS and Windows**. All three must be green. You
  cannot run Windows locally, so read `docs/AGENTLOG.md` ("Test portability pitfalls", "Windows: ConPTY ...") before touching PTY, socket,
  path or process code, and before writing tests that spawn processes or compare paths.
- Releases: see `docs/releasing.md`.

## Conventions

- Cross-platform: no Unix-only API without a `cfg` alternative; paths through `as_core::paths`, never hard-coded `/` or `~`.
- `as-core` has no UI and no socket code; the dependency direction is `as-tui -> as-proto -> as-core` and `as-daemon -> as-proto -> as-core`
  (`as-tui` never depends on `as-daemon`).
- No `unwrap()`/`expect()` outside tests except for true invariants (say why in a comment). `anyhow` in binaries and daemon glue,
  `thiserror` for typed errors in `as-core`.
- Parse the files of other tools (`~/.claude*/...`, `~/.codex/...`) tolerantly: unknown or missing fields never panic or fail a whole scan.
- Logging with `tracing` only; the daemon never prints, the TUI never writes to stdout while the alternate screen is active, and one-shot
  CLI output goes through `outln!`/`out!` (they survive a closed pipe).
- Reuse a crate over hand-rolled code when it does not add heavy bloat. Git and PRs shell out to `git` / `gh`.
- Tests: pure logic ported from Hydra comes with its tests; UI rendering is tested with ratatui's `TestBackend` (and `insta` snapshots:
  review the `.snap.new` files, accept with `INSTA_UPDATE=always` once, delete leftovers); anything that waits for threads polls
  (`wait_for`), never sleeps for a fixed time; tests never use the network, the real `~/.claude*` or the real daemon.
- User-facing text in English; MCP tools are named `studio_*`.

## Docs you must keep in sync

- `docs/porting-map.md`: when you port or change something from Hydra, update its row (`done` / `dropped` / `deferred` with a one-line
  reason; no stale `todo`).
- `docs/architecture.md`: protocol additions (requests, responses, events), data files, crates.
- `docs/tui-keys.md`: keys, views, and the `keybindings.json` format. The footer and the help overlay are generated from the keymap.
- `docs/AGENTLOG.md`: append a short entry (title + what to do instead) when you hit something non-obvious.
- `README.md` for anything a user sees (install, config keys, CLI).
