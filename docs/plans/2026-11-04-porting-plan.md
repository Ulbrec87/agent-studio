# Agent Studio — Porting plan (Hydra → Rust TUI)

> Status: **APPROVED 2026-11-04** — decisions in §8
> Source project: `~/PhpstormProjects/hydra` (Electron + React + node-pty, v0.2.66, MIT © jpdlr)
> Target: `~/PhpstormProjects/agent-studio`, a Rust terminal UI, remote `https://github.com/kawashita86/agent-studio.git`

## Progress

Tick a phase when it is pushed to `origin/main` with fmt + clippy + tests green.

- [x] P0 Bootstrap
- [x] P1 Core agents
- [x] P2 Daemon + protocol
- [x] P3 TUI MVP
- [x] P4 Sessions
- [x] P5 Grid + broadcast + palette
- [x] P6 Overview
- [x] P7 Usage & limits
- [ ] P8 MCP manager + headless
- [ ] P9 Git, files, free terminals, settings, skills, open-in
- [ ] P10 Polish
- [ ] P11 Remote control (deferred, see §8)

---

## 1. What Hydra is

A desktop app that runs several AI coding CLIs (**Claude Code**, **Codex**, **OpenCode**) side by side,
each in its own PTY, grouped by project, with persistence, an overview of "who needs me", usage/limits
tracking and a manager agent that orchestrates the others through MCP.

### 1.1 Architecture

```
┌────────────── Electron app (client) ──────────────┐        ┌──────────── Hydra daemon (node) ─────────────┐
│ React renderer: Sidebar, Header, Chat / Grid /    │  IPC   │ owns every PTY — survives closing the window │
│ Overview views, panels (Git, Editor, Usage, ...)  │◄──────►│ HTTP + WebSocket on a Unix socket            │
│ xterm.js renders agent output                     │ preload│ AgentManager, OverviewTracker, HistoryStore, │
│ main.ts: window, IPC handlers, DaemonClient       │ + REST │ Headless, MCP server, Usage, Workspace, ...  │
└───────────────────────────────────────────────────┘  + WS  └──────────────────────────────────────────────┘
```

* `electron/daemon/*` — standalone daemon (lock file, lifecycle, REST routes + WS events, ~1.5k lines).
* `electron/main.ts` + `ipc/handlers.ts` — window + bridge to the daemon (Electron-only plumbing).
* `shared/*` — pure logic (types, keybindings, overview status/buckets/waits, usage pricing/periods/parsers).

### 1.2 Feature inventory (backend lines ≈ 25k incl. shared; renderer ≈ 15k)

| # | Area | What it does | Key source |
|---|------|--------------|------------|
| F1 | **Agents** | Spawn provider CLI in a PTY (cols/rows, env, cwd), 2 MB output ring buffer, status `starting/running/idle/errored`, kill (graceful → force after 5 s), restart, remove, rename, set model, YOLO per agent + global, max 10 concurrent, idle detection (no output 5 s), submit queue (write text, wait 100 ms, `\r`, extra `\r` for Claude paste confirm), initial prompt, Claude `/rename`, model detection from output, Codex alt-screen stripping | `agents/AgentManager.ts`, `providers.ts`, `shellWrapper.ts`, `agentEnv.ts` |
| F2 | **Providers** | claude / codex / opencode: args builder, headless args, resume, yolo flag, preflight (`which` + `--version`), model catalog (static + discovered from CLIs) | `providers.ts`, `ProviderModelCatalog.ts` |
| F3 | **Shell wrapping** | `auto/direct/login/custom` — run CLI through `$SHELL -lc` so rc PATH works | `shellWrapper.ts`, `util/fix-path.ts` |
| F4 | **Worktree mode** | Agent gets its own `git worktree` on branch `hydra/<name>-<id>` | `AgentManager.create`, `GitService` |
| F5 | **Workspace persistence** | Agent set saved to `workspace.json`, restored at launch | `workspace/WorkspaceStore.ts` |
| F6 | **Session catalog / resume** | Scan `~/.claude*/projects/**/*.jsonl` and `~/.codex/sessions`, filter by age/prefix/query, hidden sessions, import limit, session-id discovery with backoff after spawn | `sessions/*` |
| F7 | **Claude profiles** | Multiple `CLAUDE_CONFIG_DIR`s (`~/.claude`, `~/.claude-<name>`); per-project mapping, longest prefix wins | `sessions/claudeProfiles.ts`, `shared/claudeProfiles.ts` |
| F8 | **Views** | Chat (one agent, terminal + input bar), Grid (tiles per project + "Running" tab, broadcast bar, expand tile), Overview | `components/ChatView`, `GridView`, `Overview` |
| F9 | **Sidebar** | Project tree, agents, filter chips, sort, search, profile chip, manager pinned top | `components/Sidebar` |
| F10 | **Overview** | Needs-you (Claude registry `~/.claude/sessions/<pid>.json` + transcript tail "what it asks"), counts, sessions list/repos/board, details drawer, last-hour chart, timeline, events log, waits today/7 days; history JSONL 8 days / 20 MB; process tree to map PIDs | `overview/*`, `shared/overview/*` |
| F11 | **Notifications** | Toasts + OS notifications: idle, waiting, errored, headless done, usage limit; window title `(2) Hydra`; sounds | `notifications`, `daemon/DaemonNotificationService.ts` |
| F12 | **MCP manager agent** | HTTP MCP server on 127.0.0.1:random; manager workspace with `.mcp.json` + `CLAUDE.md`; tools `hydra_list_agents/create_agent/send_prompt/get_output/broadcast/kill_agent/restart_agent` | `mcp/*` |
| F13 | **Headless runs** | Non-interactive `claude -p --output-format stream-json`, JSONL log per run, list/search/cancel/log tail | `headless/HeadlessOrchestrator.ts` |
| F14 | **Usage & limits** | Parse Claude/Codex logs → tokens + estimated cost (pricing table), periods day/week/month, per model/project; Claude plan limits via `api.anthropic.com/api/oauth/usage` with OAuth creds; alerts; report | `usage/*`, `shared/usage/*` |
| F15 | **Git panel** | status, log, diff, diff stats, commit, push, branches (list/checkout/create), file contents for diff viewer, PR review via `gh` | `git/GitService.ts`, `components/GitPanel` |
| F16 | **Editor panel** | File tree, Monaco editor (edit + save), fs watch, fuzzy file search (`mod+p`) | `fs/FileSystemService.ts`, `components/EditorPanel` |
| F17 | **Free terminals** | Plain shells per project (`mod+j`), split groups, LRU/idle lifecycle policies, scrollback replay | daemon free-terminal routes |
| F18 | **Settings** | Config JSON (≈40 keys), keybindings JSON (user-editable, hot reload), Skills tab (enable/disable Claude plugins & Codex skills), Claude profiles, shortcuts | `config/*`, `skills/SkillScanner.ts` |
| F19 | **Command palette / shortcuts overlay** | 28 commands, fuzzy palette, `?` overlay | `shared/keybindings.ts`, `CommandPalette` |
| F20 | **Open in…** | Launch VS Code / Cursor / Zed / … on the project | `EDITOR_REGISTRY` |
| F21 | **Remote control** | Firebase (Firestore inbox/outbox) session, QR code for a phone PWA (`hydra-remote/`), Cloud Functions backend | `remote/*`, `hydra-remote`, `firebase-backend` |
| F22 | **Updates** | electron-updater / brew upgrade / GitHub release check | `updates/UpdateService.ts` |
| F23 | **Observability** | JSONL logs with rotation, diagnostics export bundle, optional remote error endpoint | `observability/*` |
| F24 | **Packaging / CI** | electron-builder, Homebrew cask, AUR, winget, GH Actions | `packaging`, `.github` |

---

## 2. Target architecture (Rust)

Same split as Hydra, because it is what makes agents survive closing the UI (tmux-style):

```
agent-studio            → TUI client (default). Starts the daemon if not running, then attaches.
agent-studio daemon     → background process owning PTYs, state, MCP server, trackers.
agent-studio mcp-stdio  → (optional) stdio↔daemon bridge for MCP clients that prefer stdio.
agent-studio ls|send|kill|… → small scripting CLI over the same socket (cheap win, optional).
```

* **Transport**: Unix domain socket (`$XDG_RUNTIME_DIR/agent-studio/daemon.sock`), length-delimited
  JSON frames (`tokio-util::codec::LengthDelimitedCodec` + `serde_json`). Request/response with ids +
  server-pushed events (output, status, overview, usage, notifications). No HTTP/WS needed between our own
  two processes — less code and fewer crates than Hydra's REST+WS.
* **Output fan-out**: daemon keeps the raw byte ring buffer per agent (2 MB, as Hydra); on attach the client
  receives the buffer then live chunks; the **client** feeds them into a terminal emulator (`vt100`) per
  agent and renders it with ratatui. Resize from the client is forwarded to the PTY.
* **Fallback / simplicity option**: the core is a library crate, so a `--no-daemon` single-process mode is
  just "run core in-process" (useful for tests and debugging).

### 2.1 Cargo workspace

```
agent-studio/
├── Cargo.toml                (workspace)
├── crates/
│   ├── as-core/              domain: types, config, providers, agent manager (PTY), sessions, profiles,
│   │                         workspace store, overview (registry, status, history, waits), usage,
│   │                         headless, git (CLI wrapper), skills, fs search. No UI, no socket.
│   ├── as-proto/             request/response/event enums shared by daemon + client (serde)
│   ├── as-daemon/            socket server, lock file, lifecycle, MCP server (rmcp), notifications
│   └── as-tui/               ratatui app: views, widgets, keymap, client side of the protocol
├── src/main.rs  (bin `agent-studio`, clap subcommands → daemon / tui / cli)
├── docs/ (plans, architecture, decisions, porting map)
└── CLAUDE.md
```

### 2.2 Crates (reuse instead of rewrite)

| Need | Crate | Notes / why |
|------|-------|-------------|
| TUI | `ratatui` + `crossterm` | de-facto standard |
| PTY | `portable-pty` (wezterm) | cross-platform PTY, resize, child kill |
| Terminal emulation of agent output | `vt100` + `tui-term` (widget) | parses ANSI into a screen grid and renders into ratatui. Fallback if Claude Code's UI renders badly: `alacritty_terminal` (heavier, more accurate) behind the same trait |
| Async runtime | `tokio` (+ `tokio-util` codec) | daemon IO, timers, child processes |
| Serialization | `serde`, `serde_json` | config/workspace/JSONL compatible with Hydra formats |
| CLI | `clap` (derive) | subcommands |
| Errors / logs | `anyhow`, `thiserror`, `tracing`, `tracing-subscriber` (json), `tracing-appender` (rolling) | replaces Logger/Observability |
| Paths | `directories` | XDG / macOS dirs |
| MCP server | `rmcp` (official Rust SDK) with streamable-HTTP transport (`axum` comes in with it) | same tools as Hydra |
| HTTP client | `reqwest` (rustls, json) | Claude OAuth usage endpoint, release check |
| Fuzzy matching | `nucleo-matcher` | palette, file search, session search |
| File walking | `ignore` (ripgrep's walker) | gitignore-aware file search / tree |
| Text input | `tui-textarea` | chat input bar, commit message, prompts |
| Desktop notifications | `notify-rust` | OS notifications (Linux/macOS) |
| Process table | `sysinfo` | map PIDs ↔ Claude registry (processTree.ts) |
| Time | `chrono` (or `jiff`) | timestamps, periods |
| IDs | `uuid` | agent/run ids |
| Clipboard | OSC 52 escape (no crate) + optional `arboard` | copy output / paths |
| Syntax highlight (diff/file viewer) | `syntect` *(optional, feature-gated)* | only if file viewer kept |
| QR in terminal | `qrcode` *(only if remote control kept)* | renders as unicode blocks |
| Git | **shell out to `git` / `gh`** (like Hydra) | avoids libgit2 bloat; worktrees + PRs need the CLIs anyway |
| Tests | `insta` (snapshot of rendered buffers), `tempfile`, `pretty_assertions` | ratatui `TestBackend` for UI snapshots |

---

## 3. TUI design

```
┌ Agent Studio ─ [Chat] Grid  Overview ─────────── ⚠ 2 need you ─ usage 5h 42% · wk 18% ─ YOLO off ┐
│ ▸ ★ manager           │ api-refactor · claude · sonnet · running · ~/proj/api (main)               │
│ ▾ ~/proj/api     work │┌──────────────────────────────────────────────────────────────────────┐  │
│   ● api-refactor  run ││                                                                      │  │
│   ◐ tests         wait││     <live vt100 render of the agent's PTY>                           │  │
│   ○ migrations    idle││                                                                      │  │
│ ▾ ~/proj/web          ││                                                                      │  │
│   ✖ ui-fix       err  │└──────────────────────────────────────────────────────────────────────┘  │
│ / filter…             │ > prompt input (Enter send · Alt+Enter newline · Tab: focus terminal)     │
└ ^A ? help · ^A n new · ^A g grid · ^A o overview · ^A p palette ────────────────────────────────┘
```

* **Focus model**: two modes, like tmux/zellij.
  * *Terminal focus*: every key goes to the agent PTY (so Claude's own UI, permission prompts, arrows work).
  * *App focus*: navigation, panels, palette. A **prefix key** (default `Ctrl+A`, configurable) switches /
    runs a command (`prefix n` new agent, `prefix 1-9` jump, `prefix g` grid …). `mod+…` shortcuts from
    Hydra don't exist in terminals, so the keybinding file maps commands → TUI keys instead.
* **Views**: Chat (sidebar + one terminal + optional input bar), Grid (N tiles, each a scaled-down vt100
  render; tile focus forwards input; broadcast bar), Overview (cards as ratatui blocks: needs-you list,
  counts with sparklines, sessions table/board, events, last-hour bar chart, timeline as colored cells,
  waits bars).
* **Overlays/popups**: New agent dialog, command palette, session picker (resume), headless runs,
  usage report, git panel, settings, help/shortcuts, notifications as toasts at bottom-right.
* **Themes**: Hydra's palettes (terracotta dark, midnight, light) mapped to RGB colors; fallback to
  16-color when the terminal lacks truecolor.
* **Mouse**: optional (click sidebar/tiles, scroll terminal scrollback).

---

## 4. Feature mapping & proposed scope

| Hydra | Port | Notes |
|-------|------|-------|
| F1 Agents, F2 Providers, F3 Shell wrapping, F5 Workspace | **Port 1:1** | core of the app |
| F4 Worktree mode | **Port** | git CLI |
| F6 Sessions / resume, F7 Claude profiles | **Port** | file formats identical |
| F8 Chat + Grid views, F9 Sidebar | **Port (TUI-adapted)** | input bar optional, terminal focus default |
| F10 Overview | **Port** (phase 3) | all cards, rendered with ratatui charts |
| F11 Notifications | **Port** | toasts + `notify-rust` + terminal title `(2) Agent Studio` + bell instead of sounds |
| F12 MCP manager | **Port** | `rmcp`; tools renamed `studio_*` (or keep `hydra_*`? see Q) |
| F13 Headless runs | **Port** | |
| F14 Usage & limits | **Port** | parsers + pricing + OAuth limits |
| F15 Git panel | **Port, reduced** | status / diff (colored unified diff) / commit / push / branches / worktrees; PR review via `gh pr diff` read-only |
| F16 Editor panel | **Replace** | read-only file tree + preview + fuzzy file search; *edit* opens `$EDITOR` (suspends TUI) |
| F17 Free terminals | **Port** | plain shell PTYs per project; splits simplified (tabs + one split) |
| F18 Settings, Skills | **Port** | settings screen + "open config in $EDITOR"; skills toggle |
| F19 Palette / shortcuts | **Port** | |
| F20 Open in… | **Port** | spawn `code`/`cursor`/`zed`/`xdg-open` |
| F21 Remote control (Firebase + PWA) | **Drop / defer** (see Q) | needs Firebase project + separate web app |
| F22 Updates | **Reduce** | GitHub release check + "run `cargo install`/brew" hint, or `self_update` |
| F23 Observability | **Port, simplified** | `tracing` JSONL + rotation, `agent-studio diagnostics` bundle |
| F24 Packaging / CI | **New** | GH Actions: fmt, clippy, test; release binaries via `cargo-dist` later |

---

## 5. Phased implementation

Each phase ends with `cargo fmt --check && cargo clippy -- -D warnings && cargo test` green and a commit.

| Phase | Content | Deliverable |
|-------|---------|-------------|
| **P0 Bootstrap** | git init + remote, workspace skeleton, CI, CLAUDE.md, docs, config/paths, logging | `agent-studio --version` |
| **P1 Core agents** | types, config store (+ hot reload), providers + preflight, shell wrapper, env sanitizing, PTY agent manager (buffer, status, kill/restart, submit queue, idle detection), workspace store | unit tests on core |
| **P2 Daemon + protocol** | socket server, lock file, lifecycle (spawn-if-missing, version check, shutdown), event fan-out, client lib | `agent-studio ls` / `send` work against daemon |
| **P3 TUI MVP** | app shell, theme, sidebar, Chat view with vt100 terminal, terminal/app focus + prefix keymap, new agent dialog, rename/kill/restart/remove, YOLO toggles, toasts | usable daily driver |
| **P4 Sessions** | Claude/Codex session catalogs, profiles, resume picker, session-id discovery, import on startup, hidden sessions | resume works |
| **P5 Grid + broadcast + palette** | Grid view, Running tab, broadcast, command palette, help overlay, keybindings file | |
| **P6 Overview** | registry reader, process tree, status model, waiting text, tracker, history store, all cards, needs-you badge/title/notification | |
| **P7 Usage & limits** | log parsers, pricing, periods, store, OAuth limits client, header chip, report popup, alerts | |
| **P8 MCP manager + headless** | rmcp server, manager workspace, headless orchestrator + panel | |
| **P9 Git, files, free terminals, settings, skills, open-in** | panels | |
| **P10 Polish** | worktree mode, mouse, updates check, diagnostics, packaging, README | release v0.1.0 |

Shared pure logic (overview status/buckets/waits, usage pricing/periods/parsers, keybindings) is ported
**with its tests**: Hydra's `*.test.ts` fixtures are reused as Rust test cases to guarantee parity.

---

## 6. Risks

| Risk | Mitigation |
|------|-----------|
| `vt100` rendering glitches with Claude Code / Codex TUIs (synchronized output, alt screen, wide chars) | terminal-emulator trait; swap to `alacritty_terminal` if needed; test early in P3 |
| Key conflicts between our shortcuts and the agent CLI | prefix-key model; everything else passes through |
| Claude registry (`~/.claude/sessions/<pid>.json`) is undocumented | same tolerant parser as Hydra, fall back to output activity |
| Grid tiles are small → unreadable agent UI | tile shows last N lines / status summary; Enter zooms (like Hydra's expand) |
| OAuth usage endpoint is unofficial | optional, fails soft |

---

## 7. Docs that will be created

* `CLAUDE.md` — rules: build/test commands, architecture map, conventions, "keep docs in sync", agent log.
* `docs/architecture.md` — processes, crates, protocol, data files.
* `docs/porting-map.md` — Hydra file → Rust module mapping, with status.
* `docs/decisions/` — ADRs for the choices answered in §8.
* `docs/AGENTLOG.md` — gotchas found during development (same idea as Hydra's).

---

## 8. Decisions (Q&A of 2026-11-04)

| # | Topic | Decision |
|---|-------|----------|
| 1 | Process model | **Daemon + TUI client.** On quit with running agents the TUI asks: *Quit + stop N agents / Detach (keep running) / Cancel* (same as Hydra's quit dialog). |
| 2 | Remote control (F21) | **Deferred** to a later phase (after P10). |
| 3 | Editor (F16) | **Read-only** file tree + preview + fuzzy search; editing opens `$EDITOR` (TUI suspended). |
| 4 | Platforms | **Linux, macOS and Windows** from the start (portable-pty/ConPTY; daemon socket = Unix socket, named pipe on Windows via `interprocess`). |
| 5 | Prefix key | **`Ctrl+A`** (configurable). |
| 6 | Data | **Fresh** data in the OS config dir (`agent-studio`); a Settings action **"Import from Hydra"** imports config/workspace manually. MCP tools named `studio_*`. |
| 7 | Providers | **claude, codex, opencode.** |
| 8 | Git panel | Full Hydra GitService feature set (status, log, diff, stats, commit, push, branches, worktrees, PR review via `gh`); only the viewer is reduced: colored **unified diff** instead of Monaco/Pierre side-by-side. |
| 9 | License | **MIT**, crediting Hydra (jpdlr) and agent-lookout (Idris Olubisi). |
| 10 | Git workflow | During this initial port: **commit + push to `origin/main` after each phase**. Afterwards: feature branches + PRs. |
