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
* Frames: `tokio_util::codec::LengthDelimitedCodec` (u32 BE length, max frame 16 MB) carrying one JSON value each.
* Messages (`as-proto`, adjacently tagged `{"type": "...", "payload": ...}`, snake_case names; `Vec<u8>` fields are base64):
  * client -> daemon: `ClientMsg::Request { id: u64, req: Request }`
  * daemon -> client: `ServerMsg::Response { id, result: Result<Response, String> }` and
    `ServerMsg::Event(Event)` (pushed, no id). A frame that does not parse gets a `Response` with the request's id
    (0 when unknown) and an error string; the connection stays open (an oversized/broken frame closes it).
  * `Request` (P2): `Hello`, `Ping`, `ListAgents`, `GetAgent`, `CreateAgent`, `KillAgent`, `RemoveAgent`,
    `RestartAgent`, `SendInput` (submit queue), `SendRaw` (bytes), `Resize`, `Broadcast`, `RenameAgent`, `SetModel`,
    `ToggleYolo`, `SetGlobalYolo`, `Subscribe`, `Unsubscribe`, `GetConfig`, `UpdateConfig` (JSON patch), `Preflight`,
    `Status`, `Shutdown { stop_agents }`, `ClientFocus(bool)` (P3),
    `ListSessions(ListSessionsOptions)`, `HideSession { session_id, hidden }`, `ListClaudeProfiles`,
    `SetProjectProfile { project_dir, config_dir }`, `ListProviderModels { provider }` (P4, see *Sessions* below).
  * `Response`: `Ok`, `Pong`, `Flag(bool)`, `Hello(HelloInfo)`, `Agents`, `Agent(Option)`, `Ids`, `Buffer { data }`,
    `Config`, `Preflight`, `Status`, `Sessions`, `Profiles`, `Models` (P4).
  * `Event`: `AgentOutput { agent_id, data }`, `AgentStatus { agent_id, status, session_id, model }`, `AgentWaiting`,
    `AgentsChanged`, `ConfigChanged`, `DaemonShuttingDown`, `Notification(StudioNotification)` (P3). New phases add variants only.
* **Sessions (P4)**: `ListSessions` returns `SessionSummary`s (camelCase like Hydra's `ClaudeSessionSummary`, plus
  `provider` and `hidden`) newest first; the daemon scans off the async runtime (`spawn_blocking`) and adds the config's
  `hiddenSessionIds` (hidden sessions are left out unless `includeHidden`). Filters: provider, `limit`, `maxAgeDays`,
  `projectPathPrefix`, `query` (all words, case-insensitive), `includeSidechains`, `forceRefresh`. OpenCode has no
  catalog (empty list). `HideSession` and `SetProjectProfile` edit the config under its lock and answer `Config`
  (the change is also pushed as `ConfigChanged`). `SetProjectProfile.config_dir`: a dir, `""` = default profile, `None`
  removes the mapping. `CreateAgent` with `resume_session_id` resumes that session; for Claude the profile is the one
  that owns the session file, otherwise the project mapping (`claudeProfileByProject`, longest prefix wins).
  `ListProviderModels` merges nothing: discovered models (`codex app-server`, `opencode models --verbose`) replace the
  built-in list, which is the fallback (Claude: built-in aliases only); cached 5 min (failures 30 s).
* **Startup import**: after the daemon is up, when `importSessionsOnStartup` is set, every catalog's sessions
  (`sessionImportLimit`, `sessionMaxAgeDays`, `sessionImportProjectPrefix`, minus `hiddenSessionIds`, minus sessions
  without prompt and messages) become idle `sess-<id8>` agents without process, each with the provider that saved it; they
  are not persisted in `workspace.json` and the first input resumes them.
* **Session-id discovery**: after every spawn the manager asks the `SessionLocator` (catalog based) at once and then after
  2.5 s x 1.6^n (max 30 s) until it settles, the process exits or the daemon stops; a match is by project dir (prefix),
  latest user prompt hint, or "modified since start - 5 min"; ids of other agents are skipped.
* Requests of one connection are answered **in order** (so `SendRaw` keystrokes never reorder); use several
  connections or pipeline by id for parallelism.
* Handshake: first request is `Hello { client_version }`; the reply carries daemon version, `PROTOCOL_VERSION` and pid.
  Version policy (`as_proto::lifecycle::ensure_daemon`): on a `major.minor` or protocol mismatch an **idle** daemon
  (no running agent) is shut down (`Shutdown { stop_agents: false }`) and replaced; a daemon with running agents is
  never restarted automatically: the client keeps using it (warning) when the protocol matches and fails with an
  actionable error (`agent-studio stop --force`) when it does not.
* Notifications (P3, `as-daemon::notifications`, rules of Hydra's `DaemonNotificationService`): `agent_waiting` ("Agent Finished" / "<name> is waiting for input"), `agent_idle` ("Agent Completed"), `agent_errored` ("Agent Error"), `agent_started` ("Agent Started"); status notifications are de-duplicated per agent. Every notification is broadcast as `Event::Notification`; an OS notification (`notify-rust`, cargo feature `desktop-notifications`, default on) is shown only while no client reported `ClientFocus(true)`.
* Output streaming: `Subscribe { agent_id }` returns the buffered output (<= 2 MB raw bytes) and from then on
  `Event::AgentOutput` chunks are pushed to **that connection only**. Subscriptions belong to the connection: after a
  reconnect, subscribe again. Status/waiting/agents-changed/config events go to every client. A client whose outgoing
  queue overflows (8192 messages) is disconnected instead of silently losing PTY bytes.
* PTY output is **bytes** (`Vec<u8>`), not `String`: a chunk can split a UTF-8 sequence.
* Shutdown: `Shutdown { stop_agents: true }` kills all agents (waits up to 5 s), saves the workspace, broadcasts
  `DaemonShuttingDown`, exits. `stop_agents: false` is refused while any agent is active (the TUI's "Detach" simply
  disconnects). SIGTERM/Ctrl+C behave like `stop_agents: true`; SIGHUP is ignored.
* Lock file `daemon.lock`: `{ pid, version, socket, started_at }`, written after the socket is bound. A daemon counts as
  alive only if the pid exists **and** the socket accepts a connection; otherwise lock + socket file are removed.
* Spawning (`spawn_daemon`): `current_exe daemon --home <root>`, stdio null; Unix: own process group; Windows:
  `CREATE_NEW_PROCESS_GROUP | DETACHED_PROCESS | CREATE_NO_WINDOW`. Clients retry connecting with backoff for 5 s.

## Data root (`as_core::paths::Paths`)

`AGENT_STUDIO_HOME` or the OS config dir (`~/.config/agent-studio`, `~/Library/Application Support/agent-studio`,
`%APPDATA%\agent-studio`):

| File | Content |
|------|---------|
| `config.json` | `AppConfig` (Hydra `shared/types.ts` `AppConfig`, camelCase keys, defaults filled in) |
| `keybindings.json` | user overrides `[{ "command": "...", "keys": "..." }]`, keys in TUI syntax (see `docs/tui-keys.md`); polled by the TUI every second |
| `tui-state.json` | UI state of the TUI: last view, selected agent, sidebar width/sort/status chip, grid tab, expanded tile, tile order, running-only toggle (written by the TUI only, tolerant load) |
| `workspace.json` | persisted agents, restored at daemon start |
| `daemon.lock`, `daemon.sock` | daemon single instance + socket |
| `logs/` | `tracing` JSONL logs, daily rotation |
| `headless-runs/` | one JSONL log per headless run + index |
| `overview-history/` | overview events and samples (8 days, ≤ 20 MB) |
| `manager-workspace/` | cwd of the MCP manager agent (`.mcp.json`, `CLAUDE.md`) |

## External files read (never written, except skill toggles)

| Path | Used for |
|------|----------|
| `~/.claude*/projects/<slug>/<session>.jsonl` (+ `$CLAUDE_CONFIG_DIR`, optional `sessions-index.json`) | session catalog (P4), resume, usage, transcript history |
| `~/.claude*/sessions/<pid>.json` | live status / "needs you" (undocumented) |
| `~/.claude*/.credentials.json` / macOS keychain | OAuth token for plan-limits endpoint |
| `~/.codex/sessions/**/rollout-*.jsonl` (`$CODEX_HOME`) | Codex session catalog (P4) + usage |
| `~/.claude/settings.json`, `~/.claude/plugins`, `~/.codex/skills` | skills tab |

## Crates

| Crate | Responsibility |
|-------|---------------|
| `as-core` | all domain services, synchronous or tokio-based, no UI, no socket (`agents::{manager, startup, model_catalog}`, `sessions::{claude, codex, transcript, profiles, locator}`) |
| `as-proto` | `Request`/`Response`/`Event` enums and frame codec helpers |
| `as-daemon` | socket server, client registry, event fan-out, lock/lifecycle, MCP server, OS notifications |
| `as-tui` | daemon client, app state, views, widgets, keymap, theme |
| bin `agent-studio` | clap entry point, logging init, dispatch |
