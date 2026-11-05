# Agent Studio GUI (Tauri) — plan

> Status: **APPROVED 2026-11-05** — answers in §7; G0 in progress
> Goal: a modern desktop app that looks and feels like Hydra (React front end), with the Rust daemon as backend.
> The TUI stays; the GUI is a second client of the same daemon.

## 1. Why this is feasible

* The daemon (`as-daemon`) and the protocol (`as-proto`) contain no UI. The TUI is just one client.
* Hydra's renderer (`~/PhpstormProjects/hydra/src`, ~18k lines TS/TSX + ~10k lines CSS, MIT) reaches its backend only through
  `window.hydra` (`electron/preload.ts`, 411 lines, ~130 IPC methods; 140 call sites in 51 files). Providing a compatible
  `window.hydra` backed by Tauri lets us **reuse the whole front end** instead of redesigning it.
* The Rust types already mirror Hydra's `shared/types.ts` (camelCase JSON), which keeps the adapter thin.

## 2. Architecture

```
┌──────────────── Tauri app "Agent Studio" ─────────────────┐
│ webview: React UI (Hydra renderer, adapted) + xterm.js    │
│   window.hydra  ── shim (TypeScript, gui/src/bridge) ───┐ │
│                                                         ▼ │
│ Rust shell (crates/as-gui): two generic commands          │
│   daemon_request(Request JSON) -> Response JSON           │
│   daemon event stream  -> webview events                  │      local socket      ┌─ daemon ─┐
│   native: folder picker, clipboard, window, tray, quit    │ ◄────────────────────► │ as-daemon│
│   ensure_daemon() at start (same as the TUI)              │   (as-proto client)    └──────────┘
└───────────────────────────────────────────────────────────┘
```

* The Rust shell stays thin: it forwards `as_proto::Request`/`Response`/`Event` as JSON. The mapping from Hydra's API to our
  protocol lives in TypeScript (`gui/src/bridge/*`), where it is cheap to change and to test.
* The GUI binary also accepts `daemon` as argument, so `ensure_daemon` (which spawns `current_exe daemon`) works without a
  sidecar. `agent-studio` (CLI/TUI) and the GUI share the same daemon and data directory.
* Terminal output is bytes: events carry base64 and xterm.js writes `Uint8Array`.

## 3. Layout in the repo

```
crates/as-gui/        Tauri 2 shell (Rust): commands, events, tray, quit flow, bundle config (tauri.conf.json)
gui/                  Vite + React + TypeScript front end
  src/                Hydra renderer, adapted (credited, MIT)
  src/bridge/         window.hydra shim: Hydra API -> as-proto requests, event fan-out
  shared/             types copied from Hydra and kept in sync with as-core (generated or hand-checked)
```

## 4. Differences we must handle

| Topic | Hydra | Here |
|-------|-------|------|
| Keybindings | `mod+x` accelerators in the renderer | GUI uses Hydra's `mod+` defaults from its own `gui-keybindings.json`; the daemon's prefix-key file is TUI only |
| Editor panel | Monaco, edits and saves files | keep Monaco; add `FsWriteFile` to the daemon (path-safe, atomic) |
| Free terminals / layout | Hydra's group/pane layout | adapt to our split tree (`FreeTerminalNode`) |
| Remote control + Firebase | present | hidden (deferred, P11) |
| Updates | electron-updater | release check + download link/hint (no self-update); Tauri updater later |
| Observability | main-process logs | `tracing` logs via the daemon; renderer logs forwarded |
| Branding | "Hydra" name and logo | see §7 |
| Quit flow | "quit and stop N agents?" dialog | same dialog; tray icon keeps the app alive in the background (optional) |

## 5. Phases

| Phase | Content | Done when |
|-------|---------|-----------|
| **G0 Scaffold** | `gui/` (Vite, React 18, TS), copy Hydra renderer + styles, `crates/as-gui` Tauri 2 shell, workspace + `.gitignore`, `npm`/`cargo tauri dev` working, CI job that builds the front end and the Tauri crate (no bundle), docs | window opens with the real Hydra UI against a stub bridge |
| **G1 Core bridge** | commands/events, `ensure_daemon`, `window.hydra` for agents (create/kill/remove/restart/rename/model/yolo/input/resize/output), config, sessions + profiles, notifications, view mode, overview, usage | create an agent in the GUI, type in the terminal, see overview/usage |
| **G2 Everything else** | git panel, fs + editor (+`FsWriteFile`), free terminals, headless, skills, keybindings, MCP status, open-in, folder dialogs, clipboard, diagnostics, updates, Hydra import | every `window.hydra` method implemented or intentionally stubbed (documented) |
| **G3 Parity + polish** | visual pass against Hydra, theme tokens, light/dark/midnight, window state, tray, native menus, quit dialog, a11y checks, error boundaries | side-by-side with Hydra looks the same |
| **G4 Packaging + CI** | Tauri bundles: AppImage + deb (Linux), dmg (macOS), msi/nsis (Windows); extend `release.yml`; icons; docs/README; `CLAUDE.md` | tag run publishes CLI archives and desktop bundles |

Each phase = feature branch + PR, CI green on Linux/macOS/Windows, merged by squash (main is protected).

## 6. Verification strategy

* The bridge is pure TypeScript over two functions (`request`, `onEvent`), so it is unit-tested with Vitest against recorded
  protocol fixtures.
* For end-to-end checks without a native window, `crates/as-gui` has a **dev bridge** mode (`--dev-bridge`): the same commands
  exposed over a localhost WebSocket with a random token. The Vite UI then runs in a normal browser, where it can be driven and
  screenshotted with browser automation, against a real daemon in a temp home.
* The native window itself is smoke-tested under Xvfb (window opens, bridge connects) in CI on Linux.

## 7. Decisions (Q&A of 2026-11-05)

| # | Topic | Decision |
|---|-------|----------|
| 1 | Branding | **Agent Studio** name everywhere in the UI, new icon (placeholder, replaceable). Layout, colors and behavior stay like Hydra. |
| 2 | Packaging | **Yes**: AppImage + deb (Linux), dmg (macOS), msi (Windows) from G4, unsigned for now. |
| 3 | Plan | **Approved**: G0–G4 as in §5, each a feature branch + PR. |

Defaults chosen without asking (veto any of them): Node 22 LTS via nvm; Tauri 2; `crates/as-gui` is its own Cargo workspace
(it needs the WebKit system libraries, which must not be required by the core CI job); the desktop app accepts `daemon` as an
argument so it can host the daemon itself; editing files in the GUI is allowed (daemon gets `FsWriteFile`), the TUI stays read-only.

## Progress

- [x] G0 Scaffold
- [x] G1 Core bridge
- [ ] G2 Everything else
- [ ] G3 Parity + polish
- [ ] G4 Packaging + CI
