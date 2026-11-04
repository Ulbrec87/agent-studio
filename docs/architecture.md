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
    `SetProjectProfile { project_dir, config_dir }`, `ListProviderModels { provider }` (P4, see *Sessions* below),
    `GetOverview`, `GetOverviewHistory { since_ms }`, `GetOverviewWaits`, `GetOverviewEvents { since_ms, limit }`,
    `GetAgentDetails { agent_id }` (P6, see *Overview* below),
    `GetUsageState`, `GetUsageSummary { period, project_key }`, `GetUsageReport { provider, days }` (7-90), `RefreshUsage`,
    `UsageVisible(bool)` (P7, see *Usage* below).
  * `Response`: `Ok`, `Pong`, `Flag(bool)`, `Hello(HelloInfo)`, `Agents`, `Agent(Option)`, `Ids`, `Buffer { data }`,
    `Config`, `Preflight`, `Status`, `Sessions`, `Profiles`, `Models` (P4), `Overview`, `OverviewHistory`,
    `OverviewWaits`, `OverviewEvents`, `AgentDetails(Option)` (P6), `UsageState`, `UsageSummary`, `UsageReport` (P7).
  * `Event`: `AgentOutput { agent_id, data }`, `AgentStatus { agent_id, status, session_id, model }`, `AgentWaiting`,
    `AgentsChanged`, `ConfigChanged`, `DaemonShuttingDown`, `Notification(StudioNotification)` (P3),
    `OverviewSnapshot`, `OverviewEvents` (P6), `UsageChanged(UsageState)` (P7). New phases add variants only. A client that talks to an older daemon
    gets an error for the Overview and Usage requests; the TUI then shows "The overview is not available" and keeps working.
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
* **Overview (P6, `as_core::overview` + `as-daemon::services`)**: the daemon runs one `OverviewTracker` (poll every 2 s and
  100 ms after an agent status event, in `spawn_blocking`). Per poll: running Claude agents are matched to the live
  registry entries `<configDir>/sessions/<pid>.json` of every Claude profile (files > 64 KB, symlinks, `.key` files and
  dead pids are ignored; an entry that fails to parse once is reused for one poll) in three tiers, each across all
  agents so an entry is never taken twice: (1) `sessionId`, (2) the entry pid is the agent's PTY pid **or one of its
  descendants** (`sysinfo` process table, at most every 10 s, only when tiers 1 and the agent pid did not match: the
  PTY child may be the login shell wrapping `claude`), (3) same cwd and start time within 10 s, when unambiguous.
  Status = registry (`busy|shell` working, `waiting` needs-you with reason `permission prompt|input needed|other`, `idle`)
  or, without an entry and for Codex/OpenCode, output within 5 s = working else idle; process states give
  starting/finished (exit seen in this run)/failed/not-running (restored or imported, never ran). `quietFor` (working,
  silent >= 5 min) and stale (idle >= 24 h) are flags. A needs-you Claude session also gets `waitingText` from the last
  256 KB of its transcript ("Run: npm test"; cached per agent on file size/mtime/inode). `OverviewSnapshot` is pushed
  only when something meaningful changed (never for timers; clients tick their own clock), `OverviewEvents` with every
  poll's new events (appeared / status with `waitedMs` when leaving needs-you / ended / resumed after a gap > 15 s).
  `GetAgentDetails` reads the last message and the token counts of the newest reply from the transcript on demand.
  **Notifications**: a Claude agent entering needs-you raises "Needs you" / `<name> needs permission: Run: ls`; the 5 s
  silence notification (`AgentWaiting`) is suppressed for a Claude agent the tracker matched to a registry entry and kept
  for Codex, OpenCode and unmatched Claude agents (the `AgentWaiting` event itself still goes to the clients for the
  sidebar flag).
* **Usage (P7, `as_core::usage` + `as-daemon::services`)**: one `UsageService` in the daemon, all of it synchronous and run from
  `spawn_blocking` (`usage_loop` calls `tick()` every 5 s, the service decides what is due).
  * *Tokens*: `UsageStore` scans `~/.claude*/projects/**/*.jsonl` (also `subagents/`) and `$CODEX_HOME|~/.codex/{sessions,archived_sessions}`
    incrementally: per file it remembers size, mtime, inode and the byte offset of the last complete line (a partial last line waits, a
    file that shrank or changed inode is forgotten and re-read, a deleted file keeps what it added). Claude lines count once per
    `message.id:requestId`, Codex lines per session id + the `(total, last)` usage signature (resumed/forked files replay history),
    uncarried Codex compactions are billed once the file is quiet for 10 min. Buckets are per (UTC hour, provider, model, project)
    and kept 400 days; cost is the "API-equivalent" estimate of `pricing::SPECS` (unknown models are tokens only, the total says `>=`).
    Day/week (from Monday)/month windows use the local zone (`chrono::Local`). Cache: `usage-cache.json` (mode 600, atomic, at most
    every 30 s and on shutdown; a version or pricing-version change means one full rescan); it holds counts, models, project paths and
    timestamps, never message text or credentials. Rescan every 60 s (cheap: only appended bytes), first full index of ~1 GB of
    logs takes ~20 s in a debug build and is shown as "Indexing usage... 40 %".
  * *Plan limits*: Claude per profile through `GET https://api.anthropic.com/api/oauth/usage` (`Authorization: Bearer`,
    `anthropic-beta: oauth-2025-04-20`, 15 s timeout, blocking `reqwest` + rustls behind the `HttpClient` trait). The token comes from
    `<configDir>/.credentials.json` or, on macOS, the Keychain item `Claude Code-credentials[-<sha256(configDir)[..8]]` through
    `security find-generic-password -w`; it is **never refreshed** (that rotates the refresh token), never logged, never cached, never
    sent to a client: it lives in a `Secret` (redacted `Debug`) from the credential reader to the request and `UsageState` only has
    percentages, reset times and labels. 401/403 = "sign-in expired" (last values kept, the rejected token is not sent again), 429/5xx/
    network errors back off 60 s, 2 min, 5 min, 10 min (a larger `Retry-After` wins), a denied Keychain is not asked again for 24 h,
    two profiles signed in to one account are shown once. Codex limits are passive: the newest `rate_limits` of any rollout line
    (`ESTIMATED`, with its age). Cadence: every 5 min, every 60 s while any client reported `UsageVisible(true)` (a per-connection flag,
    cleared when the client disconnects), and once more (throttled to 60 s) after a Claude agent goes working -> idle.
  * *Alerts* (`usage.alertsEnabled`, default off): once per (provider, account, window, resetsAt, threshold) when a window passes the
    session/weekly alert level (default 80 % / 90 %, other windows 85 %); windows without a reset time, already reset, from a Claude
    account that is not `ok` or from a Codex line older than 30 min never alert. The config's soft daily budgets
    (`usageDailyTokenBudget`, `usageDailyCostBudgetUsd`, `usageBudgetWarningThresholdPct`, default 80 %) alert once at the warning
    level and once at 100 % per local day (the cost budget is skipped while `usage.showCost` is false). Sent keys are kept in
    `usage-alerts.json` (pruned when the window resets / the day ends) so a restart repeats nothing. Delivery is the normal
    notification path: `Event::Notification` with `NotificationKind::UsageLimit` (toast + OS notification when no client is focused).
    Alerts are re-evaluated on every usage change and when the config changes.
  * *Client*: the TUI holds `state` (pushed by `UsageChanged`), summaries per (period, project) and reports per provider, and asks
    again only when `data_version` moved; old values stay on screen meanwhile.
* **Overview history (`<root>/overview-history/v1-YYYY-MM-DD.jsonl`, mode 600, folder 700)**: one writer (the daemon).
  Lines: `{"start":<ms>}` (each daemon start; time between the last point and the next one is *unmeasured*),
  `{"point":[at,needsYou,working,idle,total,{agentId:status}]}` every 5 s (a trailing `1` marks a delta line: only the
  changed statuses, `null` for a gone agent; the first point of a run and of each file is full; agents ended for over an
  hour and not-running ones are not written), `{"event":{...}}` (written field by field). Flushed every 5 s and on
  shutdown; files of days that ended more than 8 days ago are deleted, then the oldest until the total is <= 20 MB; a
  day over 2 MB goes on in `-2`, `-3`. Memory: 1 000 events, 6 h of points; the wait ledger (moves in/out of needs-you,
  measured stretches, starts; 9 days) is rebuilt from every file at start. Unknown formats, links, other file types and
  files > 4 MB are never read; malformed lines and records dated > 1 min ahead are skipped.
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
| `usage-cache.json` | usage store: file offsets, hourly buckets, dedupe keys, newest Codex limits (mode 600) |
| `usage-alerts.json` | usage alerts already sent (key + when it stops mattering) |
| `manager-workspace/` | cwd of the MCP manager agent (`.mcp.json`, `CLAUDE.md`) |

## External files read (never written, except skill toggles)

| Path | Used for |
|------|----------|
| `~/.claude*/projects/<slug>/<session>.jsonl` (+ `$CLAUDE_CONFIG_DIR`, optional `sessions-index.json`) | session catalog (P4), resume, usage, transcript history |
| `~/.claude*/sessions/<pid>.json` | live status / "needs you" (undocumented) |
| `~/.claude*/.credentials.json` / macOS keychain (`security`) | OAuth token for the plan-limits endpoint (read-only, never refreshed, never logged) |
| `~/.codex/{sessions,archived_sessions}/**/rollout-*.jsonl` (`$CODEX_HOME`) | Codex session catalog (P4) + usage (tokens and passive limits) |
| `~/.claude/settings.json`, `~/.claude/plugins`, `~/.codex/skills` | skills tab |

## Crates

| Crate | Responsibility |
|-------|---------------|
| `as-core` | all domain services, synchronous or tokio-based, no UI, no socket (`agents::{manager, startup, model_catalog}`, `sessions::{claude, codex, transcript, profiles, locator}`) |
| `as-proto` | `Request`/`Response`/`Event` enums and frame codec helpers |
| `as-daemon` | socket server, client registry, event fan-out, lock/lifecycle, MCP server, OS notifications |
| `as-tui` | daemon client, app state, views, widgets, keymap, theme |
| bin `agent-studio` | clap entry point, logging init, dispatch |
