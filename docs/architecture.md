# Architecture

## Processes

```
 ┌──────────── agent-studio (TUI client) ────────────┐   local socket    ┌──────── agent-studio daemon ────────┐
 │ ratatui views · keymap · vt100 emulator per agent │◄────────────────►│ owns every PTY (portable-pty)        │
 │ connects; spawns the daemon if it isn't running   │ framed JSON      │ AgentManager · Config · Workspace    │
 │ quitting = detach, or stop agents + daemon        │ req/resp+events  │ Sessions · Overview · Usage · MCP    │
 └───────────────────────────────────────────────────┘                  │ Headless · Notifications · Git       │
                                                                         └──────────────────────────────────────┘
 agent-studio ls | send | kill …  → one-shot clients of the same socket (scripting)
```

* The daemon is the **only** owner of state. The TUI holds view state only (selection, focus, scroll)
  plus a local terminal emulator per attached agent, fed by output events.
* The daemon keeps running after the TUI quits (detach). It stops when asked (`Shutdown`, or quitting the
  TUI with "stop agents").
* Single instance: `daemon.lock` (pid + version + socket name) in the data root. A client that finds a
  stale lock (dead pid) or a version mismatch restarts the daemon (Hydra: `electron/daemon/lifecycle.ts`).

## Transport

* `interprocess` local sockets (tokio): Unix domain socket `<root>/daemon.sock`; Windows named pipe
  `\\.\pipe\agent-studio-<hash(root)>` (`as_core::paths::Paths::daemon_socket_name`).
* Frames: `tokio_util::codec::LengthDelimitedCodec` (u32 BE length) carrying one JSON value each.
* Messages (`as-proto`):
  * client → daemon: `ClientMsg::Request { id: u64, req: Request }`
  * daemon → client: `ServerMsg::Response { id, result: Result<Response, String> }` and
    `ServerMsg::Event(Event)` (pushed, no id).
* Handshake: first request is `Hello { client_version }`; the daemon answers with its version; mismatched
  major/minor → client asks it to shut down and starts a fresh one.
* Output streaming: `Subscribe { agent_id }` returns the buffered output (≤ 2 MB raw bytes, base64 in
  JSON) and from then on `Event::AgentOutput { agent_id, data }` chunks are pushed to that client.
  Status/overview/usage/notification events are broadcast to every client.
* PTY output is **bytes** (`Vec<u8>`), not `String`: a chunk can split a UTF-8 sequence.

## Data root (`as_core::paths::Paths`)

`AGENT_STUDIO_HOME` or the OS config dir (`~/.config/agent-studio`, `~/Library/Application Support/agent-studio`,
`%APPDATA%\agent-studio`):

| File | Content |
|------|---------|
| `config.json` | `AppConfig` (Hydra `shared/types.ts` `AppConfig`, camelCase keys, defaults filled in) |
| `keybindings.json` | user overrides `[{ "command": "...", "keys": "..." }]` |
| `workspace.json` | persisted agents, restored at daemon start |
| `daemon.lock`, `daemon.sock` | daemon single instance + socket |
| `logs/` | `tracing` JSONL logs, daily rotation |
| `headless-runs/` | one JSONL log per headless run + index |
| `overview-history/` | overview events and samples (8 days, ≤ 20 MB) |
| `manager-workspace/` | cwd of the MCP manager agent (`.mcp.json`, `CLAUDE.md`) |

## External files read (never written, except skill toggles)

| Path | Used for |
|------|----------|
| `~/.claude*/projects/<slug>/<session>.jsonl` (+ `$CLAUDE_CONFIG_DIR`) | session catalog, resume, usage, transcript tail |
| `~/.claude*/sessions/<pid>.json` | live status / "needs you" (undocumented) |
| `~/.claude*/.credentials.json` / macOS keychain | OAuth token for plan-limits endpoint |
| `~/.codex/sessions/**/rollout-*.jsonl` | Codex session catalog + usage |
| `~/.claude/settings.json`, `~/.claude/plugins`, `~/.codex/skills` | skills tab |

## Crates

| Crate | Responsibility |
|-------|---------------|
| `as-core` | all domain services, synchronous or tokio-based, no UI, no socket |
| `as-proto` | `Request`/`Response`/`Event` enums and frame codec helpers |
| `as-daemon` | socket server, client registry, event fan-out, lock/lifecycle, MCP server, OS notifications |
| `as-tui` | daemon client, app state, views, widgets, keymap, theme |
| bin `agent-studio` | clap entry point, logging init, dispatch |
