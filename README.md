# Agent Studio

**Run Claude Code, OpenAI Codex and OpenCode agents in parallel, in one desktop app.**

Agent Studio is a desktop app (Rust + [Tauri 2](https://tauri.app), React front end) that runs several AI coding CLI agents side by
side, each in its own pseudo-terminal and grouped by project. A background daemon (hosted by the same binary) owns the agents, so they
keep working while the window is closed. It is a port of the desktop app [Hydra](https://github.com/jpdlr/hydra).

## Modern Workspace

Chat, Grid and Overview share the Modern Workspace layout, including persistent project navigation, compact toolbars, framed terminals and dashboard cards. All existing agent, editor, Git and terminal controls remain available.

Choose a color palette in **Settings → Appearance → Color Theme**: Light, Terracotta, Midnight (the default), or **Modern Workspace Soft**, a desaturated blue-slate palette. Changing colors keeps the same layout. Soft is stored locally in this webview; existing themes continue to use the daemon configuration. Returning to an existing theme clears the local Soft override after saving the selection.

At smaller window sizes, Grid stacks its toolbar and tiles. When several docked panels need more space than the window provides, the workspace can scroll horizontally so controls stay reachable. The editor and terminal split handles also support arrow keys; Home resets the editor split.

See [redesign notes and validation](docs/modern-workspace-redesign.md) for implementation boundaries and manual desktop checks.

## Features

- **Three views**: *Chat* (one agent, full terminal), *Grid* (tiles per project, with a broadcast bar that sends one message to every
  running agent) and *Overview* ("which agents need me?": counts, needs-you list with wait timers, last hour, sessions, events,
  timeline, waits).
- **Agents that outlive the window**: with agents running, closing the app can leave them running in the daemon; open the app again and
  you are back. With nothing running the daemon stops with the app.
- **Resume sessions** of Claude and Codex, import existing sessions as idle agents, several **Claude profiles**
  (`CLAUDE_CONFIG_DIR`) mapped per project, optional **git worktree per agent**.
- **Notifications** when an agent needs you, finishes or fails (in-app and OS notifications).
- **Usage and plan limits**: tokens and API-equivalent cost per day/week/month from the local logs, Claude plan windows (5 h / weekly)
  from your own sign-in, optional alerts.
- **Manager agent + MCP server**: a Claude agent that creates, lists, messages and stops the other agents through `studio_*` tools,
  and **headless runs** (a provider CLI run in the background, with a rendered log).
- **Git panel** (changes with a unified or side-by-side diff, stage, commit, push, branches, read-only PR review through `gh`),
  **files browser and editor**, **free terminals** (shells per project), **Open in** (VS Code, Cursor, Zed, file manager, ...),
  **skills** toggles.
- **Command palette**, rebindable keys (`gui-keybindings.json`), themes, settings screen, diagnostics export, update check, import from Hydra.

## Install

Requirements: the agent CLIs you want to use (`claude`, `codex`, `opencode`) installed for your user; the app finds them through your
login shell's `PATH`. `git` is needed for the git panel and worktrees, `gh` (GitHub CLI) only for the PR review.

Download the installer for your system from the [Releases](https://github.com/kawashita86/agent-studio/releases) page and check it
against `SHA256SUMS`:

| OS | File |
|----|------|
| Linux x86_64 | `.AppImage` (`chmod +x`, run) or `.deb` (`sudo apt install ./agent-studio-*.deb`) |
| macOS (Apple Silicon / Intel) | `.dmg` |
| Windows x86_64 | `.msi` |

The installers are **not signed**: macOS Gatekeeper and Windows SmartScreen warn on first launch (macOS: right-click > Open).

**From source** (Rust 1.88 or newer, Node 22; Linux also needs the WebKit libraries `libwebkit2gtk-4.1-dev`, `libgtk-3-dev`,
`libayatana-appindicator3-dev`, `librsvg2-dev`, `libsoup-3.0-dev`):

```bash
git clone https://github.com/kawashita86/agent-studio
cd agent-studio/gui && npm ci && npm run build && cd ../crates/as-gui
../../gui/node_modules/.bin/tauri build --no-bundle     # ../../target/release/agent-studio
```

Agent Studio never updates itself. It checks GitHub Releases now and then and tells you where to download the new version
(see [Updates](#updates)).

## Quick start

Start **Agent Studio**. The first launch starts the daemon and shows an empty sidebar. Then create an agent (directory, provider,
model, YOLO = skip permission prompts, optional first prompt) from the sidebar or the command palette; its terminal opens. The palette
(`Ctrl/Cmd+K`) lists every command and key binding.

## Configuration

Everything lives in the data directory:

| OS | Data directory |
|----|----------------|
| Linux | `~/.config/agent-studio` (`$XDG_CONFIG_HOME/agent-studio`) |
| macOS | `~/Library/Application Support/agent-studio` |
| Windows | `%APPDATA%\agent-studio` |

`AGENT_STUDIO_HOME=<dir>` (or `agent-studio --home <dir>`) uses another directory, which also isolates the daemon: handy to try things
without touching your real setup.

| File | Content |
|------|---------|
| `config.json` | settings (below); edit it by hand (picked up within a second) or use the Settings screen |
| `gui-keybindings.json` | key overrides (Settings > Keybindings creates it) |
| `workspace.json` | agents (managed by the app) |
| `logs/` | JSONL logs of the daemon and the app, rotated daily |

### `config.json`

Values with a wrong type are ignored one by one (the rest still applies); a corrupt file is saved as `config.json.corrupt` and
replaced by the defaults.

| Key | Default | Meaning |
|-----|---------|---------|
| `defaultProvider` | `"claude"` | provider preselected for new agents (`claude`, `codex`, `opencode`) |
| `defaultModel` | `"sonnet"` | model preselected for new agents |
| `globalYolo` | `false` | skip permission prompts for every new agent |
| `maxAgents` | `8` | agents with a running process at once (hard limit 10) |
| `theme` | `"midnight"` | `light`, `dark` or `midnight` |
| `defaultViewMode` | `"chat"` | view at start: `chat`, `grid`, `overview` |
| `gridColumns` | `"auto"` | `auto`, `2` or `3` |
| `defaultProjectDir` | `""` | preset of the *Directory* field |
| `defaultEditor` | `"vscode"` | editor preselected in *Open in* |
| `importSessionsOnStartup` | `true` | add Claude/Codex sessions as idle agents when the daemon starts |
| `sessionImportLimit`, `sessionMaxAgeDays` | `500`, `7` | how many / how old (0 = no limit) |
| `sessionImportProjectPrefix` | `""` | only import sessions below this folder |
| `hiddenSessionIds` | `[]` | sessions hidden in the resume picker |
| `enableSoundEffects` | `true` | terminal bell when an agent needs you |
| `terminalShellMode`, `terminalShellPath`, `terminalShellArgs` | `"auto"`, `""`, `""` | how agent CLIs are launched (`auto`, `direct`, `login`, `custom`) |
| `terminalCursorStyle`, `terminalCursorBlink` | `"bar"`, `false` | cursor of the terminals |
| `terminalFontFamily`, `terminalFontSize`, `terminalEnableWebgl` | `"SF Mono", "Menlo", "Monaco", monospace`, `12`, `false` | terminal font and WebGL renderer of the terminals; the size is kept within 8-32 |
| `freeTerminalLifecyclePolicy` | `"explicit"` | `explicit`, `lru` or `idle` (when shells are closed) |
| `freeTerminalMaxCount`, `freeTerminalIdleTimeoutMinutes`, `freeTerminalScrollbackLines` | `6`, `60`, `5000` | free terminal limits |
| `gitPanelDisplayMode`, `editorPanelDisplayMode` | `"overlay"` | `overlay` (centered) or `split` (side column) |
| `diffLayout` | `"unified"` | Git panel diff: `unified` or `split` (side by side) |
| `claudeProfileByProject` | `{}` | project folder -> Claude config dir (longest path wins; `""` = default profile) |
| `usage` | see below | usage chip, cost and alerts |
| `checkForUpdates` | `true` | ask GitHub Releases for a newer version at start and every 6 h |
| `includeSensitiveDiagnostics` | `false` | keep sensitive fields in the diagnostics export |
| `enableRemoteErrorReporting`, `errorReportingEndpoint`, `remoteControlEnabled`, `remoteSessionTimeoutMinutes` | `false`, `""`, `false`, `480` | kept for compatibility with Hydra; not used |

`usage`: `alertsEnabled` (`false`), `sessionAlertPercent` (`80`), `weeklyAlertPercent` (`90`), `chipProvider` (`auto`, `claude`,
`codex`), `showCost` (`true`).

## The daemon

The app starts the daemon (the same binary, `agent-studio daemon --home <dir>`, hidden) when it opens; it owns every agent process, so
closing the window does not stop running agents. One daemon per data directory (`daemon.lock`). On close with no agent running the app
stops the daemon; an idle daemon of another version is replaced, one with running agents is kept and the UI shows a banner. The daemon
restores the agents of `workspace.json` when it starts (they resume their sessions on the first input), watches `config.json`, and sends
OS notifications.

### MCP manager agent

The daemon runs a local MCP server (loopback, bearer token) with the tools `studio_list_agents`, `studio_create_agent`,
`studio_send_prompt`, `studio_get_output`, `studio_broadcast`, `studio_kill_agent`, `studio_restart_agent`. *New Manager Agent* in the
palette starts a Claude agent in the manager workspace that can use them to orchestrate your other agents.

### Import from Hydra

Settings > Import from Hydra previews (settings that would change, agents that would be added; Hydra's data folder can be chosen) and, after you
confirm, merges Hydra's config and agents into Agent Studio's. Hydra's files are only read.

## Updates

Every 6 hours (and 10 seconds after the daemon starts) the daemon asks `api.github.com` for the latest release. If a newer stable version
exists a banner links the installer for your OS and CPU. Turn the automatic check off with `"checkForUpdates": false`. No request carries
any credential or data about you.

## Troubleshooting

- **Logs**: `<data directory>/logs/` (JSONL). `AGENT_STUDIO_LOG=debug agent-studio` for more detail. The diagnostics export (command
  palette) writes logs and a snapshot without credentials to `<data directory>/diagnostics/<time>/`, ready to attach to an issue.
- **A CLI is "not found"**: the app repairs its `PATH` from your login shell at start, and the daemon starts agents through your login
  shell (`terminalShellMode: auto`); if you set `PATH` somewhere login shells do not read, set `terminalShellMode` to `custom`.
- **The daemon does not start**: run it in the foreground to see why: `agent-studio daemon --foreground` (use `--home <dir>` for the
  same data directory). A stale lock is cleaned up by itself.
- **Windows**: the daemon uses a named pipe (`\\.\pipe\agent-studio-...`), agent CLIs installed as `.cmd` shims are started through
  `cmd.exe`. Mention it in an issue with the diagnostics export.

## Development

```bash
cd gui && npm ci                          # Node 22
npm run dev                               # front end only, in a browser: http://localhost:1420 (mock data)
cd ../crates/as-gui && ../../gui/node_modules/.bin/tauri dev    # native window (starts the Vite server itself)
```

## Documentation

See [docs/README.md](docs/README.md) for the index (architecture, porting map and decisions). Contributions are welcome:
[CONTRIBUTING.md](CONTRIBUTING.md).

## Credits

- [Hydra](https://github.com/jpdlr/hydra) by jpdlr (MIT): the desktop app this project ports; behavior, defaults and many texts come
  from it.
- The front end under `gui/` is Hydra's renderer (MIT, jpdlr), adapted: see [gui/NOTICE.md](gui/NOTICE.md).
- [agent-lookout](https://github.com/Olanetsoft/agent-lookout) by Idris Olubisi (MIT): the model for the Overview ("which agents need me?").
- Built with [Tauri](https://tauri.app), [portable-pty](https://crates.io/crates/portable-pty), [tokio](https://tokio.rs),
  [interprocess](https://crates.io/crates/interprocess) and [rmcp](https://crates.io/crates/rmcp).

## License

[MIT](LICENSE).
