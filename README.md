# Agent Studio

**Run Claude Code, OpenAI Codex and OpenCode agents in parallel, from your terminal.**

Agent Studio is a terminal UI (Rust, [ratatui](https://ratatui.rs)) that runs several AI coding CLI agents side by side, each in its
own pseudo-terminal and grouped by project. A background daemon owns the agents, so they keep working when you close the UI. Reattach any
time, like tmux. It is a port of the desktop app [Hydra](https://github.com/jpdlr/hydra).

## Features

- **Three views**: *Chat* (one agent, full terminal), *Grid* (tiles per project, with a broadcast bar that sends one message to every
  running agent) and *Overview* ("which agents need me?": counts, needs-you list with wait timers, last hour, sessions, events,
  timeline, waits).
- **Agents that outlive the UI**: quit with *Detach* and the agents keep running in the daemon; start the UI again and you are back.
- **Resume sessions** of Claude and Codex, import existing sessions as idle agents, several **Claude profiles**
  (`CLAUDE_CONFIG_DIR`) mapped per project, optional **git worktree per agent**.
- **Notifications** when an agent needs you, finishes or fails (toast, terminal bell, OS notification while the terminal is unfocused).
- **Usage and plan limits**: tokens and API-equivalent cost per day/week/month from the local logs, Claude plan windows (5 h / weekly)
  from your own sign-in, optional alerts.
- **Manager agent + MCP server**: a Claude agent that creates, lists, messages and stops the other agents through `studio_*` tools,
  and **headless runs** (a provider CLI run in the background, with a rendered log).
- **Git panel** (changes with a unified or side-by-side diff, stage, commit, push, branches, read-only PR review through `gh`),
  **files browser** with search and a syntax-highlighted preview, **free terminals** (shells per project, split right / down, the
  layout survives a daemon restart), **Open in** (VS Code, Cursor, Zed, file manager, ...), **skills** toggles.
- **Command palette**, fully rebindable keys (`keybindings.json`), three themes, optional mouse support.
- **Settings screen**, diagnostics export (logs + snapshot without secrets), update check, import from Hydra.

## Install

Requirements: the agent CLIs you want to use on your `PATH` (`claude`, `codex`, `opencode`). `git` is needed for the git panel and
worktrees, `gh` (GitHub CLI) only for the PR review. A terminal with truecolor support is recommended (the theme falls back to 256 and
16 colors otherwise). Linux, macOS and Windows (Windows Terminal / ConPTY) are supported.

**Release binaries** (Linux x86_64 / arm64, macOS Intel / Apple Silicon, Windows x86_64): download the archive for your system from the
[Releases](https://github.com/kawashita86/agent-studio/releases) page, check it against `SHA256SUMS`, unpack it and put `agent-studio` on
your `PATH`.

**With cargo** (Rust 1.88 or newer; needs a C compiler):

```bash
cargo install --git https://github.com/kawashita86/agent-studio --locked
```

**From source**:

```bash
git clone https://github.com/kawashita86/agent-studio
cd agent-studio
cargo build --release          # ./target/release/agent-studio
```

Syntax highlighting (Files preview, diffs) is the cargo feature `highlight`, on by default and pure Rust (about 0.7 MB). Build with
`--no-default-features` to leave it out; everything then shows plain text.

Agent Studio never updates itself. It checks GitHub Releases now and then and tells you how to upgrade for the way you installed it
(see [Updates](#updates)).

## Quick start

```bash
agent-studio
```

The first launch starts the daemon and shows an empty sidebar. Then:

1. `Ctrl+A n` opens the **New agent** dialog: directory, provider (Claude / Codex / OpenCode), model, YOLO (skip permission prompts),
   optional first prompt. `Enter` creates the agent and focuses its terminal.
2. While a terminal is focused **every key goes to the agent**; app commands are `prefix key`, with `Ctrl+A` as the prefix (like tmux).
   `Ctrl+A ?` shows every binding, `Ctrl+A p` opens the command palette.
3. `Ctrl+A g` toggles Chat and Grid, `Ctrl+A O` opens the Overview, `Ctrl+A s` moves to the sidebar (`j`/`k`, `Enter`), `Ctrl+A 1`-`9`
   jump to the n-th agent, `Ctrl+A o` resumes an earlier session.
4. `Ctrl+A q` quits. With agents running it asks: **Stop all & quit**, **Detach** (leave, agents keep running) or **Cancel**.

Key bindings are listed in [docs/tui-keys.md](docs/tui-keys.md); the footer always shows the ones that apply right now.

## Configuration

Everything lives in the data directory:

| OS | Data directory |
|----|----------------|
| Linux | `~/.config/agent-studio` (`$XDG_CONFIG_HOME/agent-studio`) |
| macOS | `~/Library/Application Support/agent-studio` |
| Windows | `%APPDATA%\agent-studio` |

`AGENT_STUDIO_HOME=<dir>` (or `--home <dir>`) uses another directory, which also isolates the daemon: handy to try things without
touching your real setup.

| File | Content |
|------|---------|
| `config.json` | settings (below); edit it by hand (picked up within a second) or use the Settings screen `Ctrl+A ,` |
| `keybindings.json` | key overrides, see [docs/tui-keys.md](docs/tui-keys.md#keybindings-file); palette: *Open Keybindings File* |
| `workspace.json`, `tui-state.json` | agents and UI state (managed by the app) |
| `logs/` | JSONL logs of the daemon and the UI, rotated daily |

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
| `freeTerminalLifecyclePolicy` | `"explicit"` | `explicit`, `lru` or `idle` (when shells are closed) |
| `freeTerminalMaxCount`, `freeTerminalIdleTimeoutMinutes`, `freeTerminalScrollbackLines` | `6`, `60`, `5000` | free terminal limits |
| `gitPanelDisplayMode`, `editorPanelDisplayMode` | `"overlay"` | `overlay` (centered) or `split` (side column) |
| `diffLayout` | `"unified"` | Git panel diff: `unified` or `split` (side by side, needs a 100+ column panel; `s` toggles it) |
| `claudeProfileByProject` | `{}` | project folder -> Claude config dir (longest path wins; `""` = default profile) |
| `usage` | see below | usage chip, cost and alerts |
| `prefixKey` | `"ctrl+a"` | the prefix key, e.g. `"ctrl+b"` |
| `checkForUpdates` | `true` | ask GitHub Releases for a newer version at start and every 6 h |
| `mouse` | `false` | capture the mouse (read at start) |
| `includeSensitiveDiagnostics` | `false` | keep sensitive fields in the diagnostics export |
| `enableRemoteErrorReporting`, `errorReportingEndpoint`, `remoteControlEnabled`, `remoteSessionTimeoutMinutes` | `false`, `""`, `false`, `480` | kept for compatibility with Hydra; not used |

`usage`: `alertsEnabled` (`false`), `sessionAlertPercent` (`80`), `weeklyAlertPercent` (`90`), `chipProvider` (`auto`, `claude`,
`codex`), `showCost` (`true`).

### Mouse

Off by default: capturing the mouse takes the terminal's native text selection away. Hold `Shift` (or `Option` in some macOS terminals)
while dragging to select text. Enable it with `"mouse": true`, `agent-studio --mouse` or `AGENT_STUDIO_MOUSE=1`: clicks on the header
tabs, sidebar rows and grid tiles, and the wheel scrolls the Chat scrollback.

## Command line

```
agent-studio [OPTIONS] [COMMAND]

Commands:
  daemon        Run the background daemon
  ls            List agents
  new           Create and start an agent
  import-hydra  Import config, agents and keybindings from Hydra (never modifies Hydra's files)
  send          Send text to an agent and submit it
  kill          Stop an agent's process (it stays in the list)
  rm            Remove an agent
  restart       Restart an agent
  status        Show the daemon status (starts it when needed)
  stop          Stop the daemon
  update-check  Check GitHub Releases for a newer version (does not need the daemon)
  version       Print the version

Options:
      --home <DIR>  Data root (same as AGENT_STUDIO_HOME)
  -V, --version     Print the version
      --mouse       Capture the mouse in the TUI (wheel scrolls the scrollback, clicks pick agents)
  -h, --help        Print help
```

Without a command the TUI starts. The one-shot commands talk to the same daemon, so they are scriptable:
`agent-studio new -p ~/code/api --provider codex --prompt "fix the failing tests"`, `agent-studio ls`, `agent-studio send <id> "continue"`.

## The daemon

The first `agent-studio` (or any one-shot command) starts the daemon in the background; it owns every agent process, so closing the UI
(or the terminal) does not stop them. Details: one daemon per data directory (`daemon.lock`), a UI of another version restarts an idle
daemon and refuses to touch one with running agents (`agent-studio stop --force` stops it and the agents). The daemon restores the
agents of `workspace.json` when it starts (they resume their sessions on the first input), watches `config.json`, and sends OS
notifications while no UI is focused.

### MCP manager agent

The daemon runs a local MCP server (loopback, bearer token) with the tools `studio_list_agents`, `studio_create_agent`,
`studio_send_prompt`, `studio_get_output`, `studio_broadcast`, `studio_kill_agent`, `studio_restart_agent`. *New Manager Agent* in the palette starts a Claude agent in the
manager workspace that can use them to orchestrate your other agents. The help overlay shows the server status.

### Import from Hydra

`agent-studio import-hydra` (or Settings > Data) previews and merges Hydra's config, agents and keybindings into Agent Studio's. Hydra's
files are only read.

## Updates

Every 6 hours (and 10 seconds after the daemon starts) the daemon asks `api.github.com` for the latest release. If a newer stable version
exists a banner shows how to upgrade for your installation (`brew upgrade agent-studio`, `cargo install --git ... --locked --force`, or the
archive URL for your OS and CPU). `Ctrl+A U` checks right now, `agent-studio update-check` does the same from the shell. Turn the
automatic check off with `"checkForUpdates": false`. No request carries any credential or data about you.

## Troubleshooting

- **"daemon did not start within 5s"**: run it in the foreground to see why: `agent-studio daemon --foreground`. A stale lock is
  cleaned up by itself; if something is really stuck, `agent-studio stop --force` and delete `daemon.lock` / `daemon.sock` in the data
  directory.
- **Stop everything**: `agent-studio stop --force` (stops the agents too). Close the UI first, otherwise it reconnects and starts a new
  daemon.
- **Logs**: `<data directory>/logs/` (JSONL). `AGENT_STUDIO_LOG=debug agent-studio` (also for the daemon) for more detail. `Ctrl+A p` > *Export Diagnostics* writes
  logs and a snapshot without credentials to `<data directory>/diagnostics/<time>/`, ready to attach to an issue.
- **A CLI is "not found"**: the daemon starts agents through your login shell (`terminalShellMode: auto`) so your `PATH` applies; if you
  set it somewhere that login shells do not read, set `terminalShellMode` to `custom` with your shell.
- **Keys do nothing / go to the agent**: while a terminal is focused only `Ctrl+A` is special. Watch the footer (it names the mode).
  Inside Git/Files/Settings panels `Ctrl+A` followed by `t`, `p`, `?` or `Esc` still works.
- **Colors look wrong**: set `COLORTERM=truecolor` if your terminal supports it, or pick another `theme`.
- **Windows**: use Windows Terminal (ConPTY). The daemon uses a named pipe (`\\.\pipe\agent-studio-...`), agent CLIs installed as
  `.cmd` shims are started through `cmd.exe`. If a first start hangs, `agent-studio stop --force` and retry; mention it in an issue with
  the diagnostics export.

## Documentation

See [docs/README.md](docs/README.md) for the index (architecture, key bindings, porting map and decisions). Contributions are welcome:
[CONTRIBUTING.md](CONTRIBUTING.md).

## Credits

- [Hydra](https://github.com/jpdlr/hydra) by jpdlr (MIT): the desktop app this project ports; behavior, defaults and many texts come
  from it.
- [agent-lookout](https://github.com/Olanetsoft/agent-lookout) by Idris Olubisi (MIT): the model for the Overview ("which agents need me?").
- Built with [ratatui](https://ratatui.rs), [portable-pty](https://crates.io/crates/portable-pty), [vt100](https://crates.io/crates/vt100),
  [tokio](https://tokio.rs), [interprocess](https://crates.io/crates/interprocess) and [rmcp](https://crates.io/crates/rmcp).

## License

[MIT](LICENSE).
