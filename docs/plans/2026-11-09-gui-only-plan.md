# GUI-only Agent Studio — plan (2026-11-09)

> Replaces the remaining G3/G4 of `2026-11-05-gui-tauri-plan.md`. User request of 2026-11-09.

## 1. Findings

| # | Problem | Root cause |
|---|---------|------------|
| 1 | "No Supported CLI Found" although `claude --version` works | The daemon was started by a systemd user unit. systemd's PATH has no `~/.local/bin` (where `claude` lives), so `which claude` in the daemon fails. The same happens for an app started from a desktop launcher/Dock with nvm/bun/npm-global tools. Hydra solves it with `fixPath()` (login-shell PATH on macOS, extra dirs on Windows). |
| 2 | Dropdowns white text on white background in dark mode | Native `<select>` popups in WebKitGTK/WKWebView follow the OS `color-scheme`, not our CSS: we never declare `color-scheme`, and `option` has no colors. Chromium (Hydra) inherits the select colors, WebKit does not. |
| 3 | A separately managed daemon | Not needed. Hydra also runs a daemon, but **the app spawns it hidden** (same binary), and on close it stops it when no agent runs; with running agents it leaves it alive. The systemd unit was our install step, not a requirement — it is removed. Remaining gap: our app never stops an idle daemon on close, and an outdated daemon (older version than the app) is reused. |
| 4 | Terminal (TUI/CLI) version not wanted | The TUI (`as-tui`, ~33k lines), the CLI binary (`src/`) and their docs/CI/release parts go. |

## 2. Steps (each a branch + PR, CI green on 3 OS)

### A. `fix/gui-path-selects-daemon` — the bugs (DONE, branch committed locally; PR pending)
1. `as_core::shell_env::fix_path()` (port of Hydra `fix-path.ts`, but for macOS **and Linux**): read `PATH` from `$SHELL -ilc` (timeout 5 s,
   marker-delimited), merge with the current PATH (no duplicates), fallback dirs (`~/.local/bin`, `~/.bun/bin`, `~/.cargo/bin`,
   `/usr/local/bin`, `/opt/homebrew/bin`, npm global); Windows: Hydra's list. Called at daemon start (before any provider lookup) and
   in the GUI process. Tests for merge/parse.
2. Dark selects: `color-scheme: light|dark` per theme in `tokens.css`, `option`/`optgroup` colors from tokens, check every `<select>`.
3. Daemon lifecycle like Hydra: on window close with **0 running agents → shutdown the daemon** (no orphan process); on connect,
   if the daemon's version/protocol differs from the app and nothing runs → restart it, else show the banner.

### B. `refactor/gui-only` — remove the terminal app (DONE, branch committed locally; PR pending)
1. Delete `crates/as-tui`, root `src/` (CLI) and the root package; root `Cargo.toml` becomes a pure workspace.
2. `crates/as-gui` joins the root workspace (one `Cargo.lock`, one target dir); its binary becomes **`agent-studio`**
   (hosts the daemon with `agent-studio daemon`, hidden). Keep `--dev-bridge`.
3. Remove the TUI prefix-key keybindings (`as_core::keybindings`, `keybindings.json`, `docs/tui-keys.md`); GUI keybindings stay.
4. Commands the CLI had and the GUI lacks: Hydra import (Settings button, already there), update check (already) — nothing lost.
5. CI: one job per OS with WebKit deps (fmt, clippy, tests, front end). Release: Tauri bundles (AppImage + deb, dmg, msi) instead
   of CLI archives (this is the old G4).
6. Docs: CLAUDE.md, README, architecture, porting-map, releasing, CONTRIBUTING.

### C. `refactor/cleanup` — refactors made possible by B
1. Dead code in `as-core`/`as-proto`/`as-daemon` used only by the TUI/CLI (find with clippy `dead_code` once `pub` items lose callers;
   e.g. `terminal_tree` split tree, one-shot status formatting, install-method detection for `cargo install`).
2. Generate the TypeScript protocol types from Rust (`ts-rs`) instead of hand-written copies in `gui/src/bridge` — removes drift.
3. Updates: detect "installed from a bundle" (AppImage/deb/dmg/msi) instead of cargo/archives; Tauri updater later.

### D. G3 polish (unchanged scope)
Window state, tray, native menus/Cmd+Q through the quit flow, visual pass against Hydra.

Not tagging v0.1.0 until the user says so.
