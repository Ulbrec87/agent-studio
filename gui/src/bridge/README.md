# Bridge: `window.hydra` for Agent Studio

`window.hydra` has exactly the shape of Hydra's `electron/preload.ts` (`HydraAPI` in `hydraApi.ts`), so the copied renderer runs
unchanged. Layers:

| File | Role |
|------|------|
| `transport.ts` | `Transport { request, onEvent }` (Hydra level) and `RawTransport` (protocol level + `onStatus`/`status`), `selectRawTransport()` / `selectTransport()` |
| `tauriTransport.ts` | desktop raw transport: `invoke('app_info')`, `invoke('daemon_request', { request })`, `daemon-event`, `daemon-status`, `daemon_status` |
| `wsTransport.ts` | browser dev raw transport: `?bridge=ws://127.0.0.1:PORT&token=T` -> the `agent-studio-gui --dev-bridge` WebSocket |
| `daemonApi.ts` | **the mapping layer**: Hydra method -> `as-proto` request, daemon event -> Hydra `on*` channel; unwired methods fall through to the mock |
| `mappers.ts` | pure shape adapters (preflight, notification, MCP status, config defaults + GUI-only keys) |
| `streams.ts` | per-agent output streams: `Subscribe`, held live chunks, de-duplication, streaming `TextDecoder` |
| `protocol.ts` | protocol envelope types, base64 helpers, `toError` |
| `mockTransport.ts` + `fixtures.ts` | in-memory answers for plain browsers, tests and every method that is not wired yet; `emit()` for tests |
| `bridge.ts` | `call` / `send` / `subscribe` over a Hydra-level transport; fans events out by channel |
| `hydraApi.ts` | generated method table: every method calls the bridge with its own name (+ 3 Agent Studio extensions, below) |
| `index.ts` | `installHydra()`: loads `app_info`, waits up to 4 s for the daemon, builds and installs `window.hydra` before the first render |

Two wire formats:

* **Hydra level** (`bridge.ts` <-> `daemonApi.ts`/mock): a request is `{ method: '<hydra method>', args: [...] }`, an event is
  `{ channel: '<onXxx name>', args: [...] }`; `{ kind: 'app_info' }` asks for the shell's `AppInfo`.
* **Protocol level** (`daemonApi.ts` <-> shell): the JSON of `as_proto::Request` / `Response` / `Event` (`{ type, payload }`, snake_case
  type names, snake_case message fields, camelCase domain structs, base64 bytes). The Rust shell is generic and has no per-method code:
  `daemon_request(request) -> response` (a daemon error rejects with its message), events on `daemon-event`, connection state on
  `daemon-status` (`{ connected, error? }`; `connected: true` again after a drop means "subscriptions are gone, start over").
  The dev bridge frames the same thing over a WebSocket (`crates/as-gui/src/devbridge.rs`).

### How the layer behaves

* **Output is bytes.** `AgentOutput` carries base64; each agent has its own streaming `TextDecoder` (`{ stream: true }`), so a UTF-8
  character split over two PTY chunks is intact. Typed input becomes UTF-8 bytes for `SendRaw`.
* **Subscriptions.** The daemon pushes output only to subscribed connections. The layer subscribes every non-idle agent on connect and on
  `agents_changed` / status events, forgets agents that disappeared and subscribes again after a reconnect. `getAgentBuffer` re-sends
  `Subscribe` and serves the returned buffer (the renderer merges it with live chunks, like Hydra's own reconnect). The first
  subscription of an agent holds live chunks until the `Subscribe` answer arrived and drops the daemon's documented repeat of the last
  chunk (only when the chunk is at least 4 bytes, so a repeated spinner frame is never eaten).
* **Ordering.** Concurrent Tauri `invoke`s carry no ordering guarantee, so `sendInput` / `sendRawInput` / `resizeAgent` of one agent are
  chained (the next is sent when the previous was answered).
* **Resize.** Hydra's grid tiles call `resizeAgent` too, so (like Hydra, unlike the TUI) a tile resizes the PTY to the tile size. Keeping the
  PTY at the chat pane size needs the renderer to stop resizing from tiles; not done in G1 (limitation).
* **Config.** The daemon's config lacks three Hydra keys (`terminalFontFamily`, `terminalFontSize`, `terminalEnableWebgl`): they are
  filled from Hydra's defaults and a patch of them is kept in the webview's `localStorage` (`agent-studio.gui-config`). Everything else is
  `GetConfig` / `UpdateConfig`; `config_changed` events become `onConfigChange`.
* **Keybindings** are Hydra's `mod+` defaults (`getKeybindings`); the path getters return `<data dir>/gui-keybindings.json` and
  `<data dir>/config.json`. Real editing is G2.
* **Window focus** is reported with `ClientFocus`, so the daemon only shows OS notifications while the GUI is not focused.
* **Errors**: a rejected request becomes an `Error` with the daemon's message; a response of an unexpected variant is an error.
* **Shape fixes** (everything else already matches `gui/shared/types.ts`): `Preflight` (list per provider, `path`) -> one result with
  `claudePath`; `StudioNotification` (`kind`, `agent_id`) -> `HydraNotification` (`type`, `agentId`); MCP status `manager_workspace` ->
  `managerWorkspace`; the wait ledger is wrapped as `{ ledger }`; global YOLO also writes `globalYolo` into the config (Hydra does).

### Agent Studio extensions of `window.hydra`

| Method | Meaning |
|--------|---------|
| `getDaemonStatus()` / `onDaemonStatus(cb)` | `{ connected, error? }`; drives the "Connecting to the daemon..." / "Daemon not reachable" banner (`components/Daemon`) |
| `onAgentsChanged(cb)` | the agent list changed elsewhere (CLI/TUI/MCP) or the connection came back: `useAgents` reloads the list |

### Running the front end against a real daemon in a browser

```bash
cd crates/as-gui && cargo run -- --dev-bridge --home /tmp/as-dev     # prints: DEV_BRIDGE ws://127.0.0.1:PORT token=TOKEN
cd gui && npm run dev                                                # http://localhost:1420/?bridge=ws://127.0.0.1:PORT&token=TOKEN
```

The dev bridge is a dev/test tool (browser automation, screenshots): loopback only, random port and token, never started without the flag.

## Methods

Status: `stub` = answered by the mock (fixtures / empty states / documented no-op), `wired` = real daemon or native call.
Tick them off here as G1/G2 land (`wired.test.ts` keeps this table in step with `WIRED_METHODS` / `WIRED_CHANNELS` in `daemonApi.ts`). `send` = fire-and-forget (`ipcRenderer.send`), `on` = subscription returning an unsubscribe.

| Method | Kind | Phase | Status |
|--------|------|-------|--------|
| `preflight` | invoke | G1 | wired |
| `createAgent` | invoke | G1 | wired |
| `killAgent` | invoke | G1 | wired |
| `removeAgent` | invoke | G1 | wired |
| `restartAgent` | invoke | G1 | wired |
| `listAgents` | invoke | G1 | wired |
| `toggleYolo` | invoke | G1 | wired |
| `renameAgent` | invoke | G1 | wired |
| `setAgentModel` | invoke | G1 | wired |
| `getAgentBuffer` | invoke | G1 | wired |
| `sendInput` | send | G1 | wired |
| `sendRawInput` | send | G1 | wired |
| `resizeAgent` | send | G1 | wired |
| `broadcast` | invoke | G1 | wired |
| `onAgentOutput` | on | G1 | wired |
| `onAgentStatus` | on | G1 | wired |
| `getOverview` | invoke | G1 | wired |
| `getOverviewAgentDetails` | invoke | G1 | wired |
| `getOverviewHistory` | invoke | G1 | wired |
| `getOverviewWaits` | invoke | G1 | wired |
| `onOverviewEvents` | on | G1 | wired |
| `onOverviewSnapshot` | on | G1 | wired |
| `getUsageState` | invoke | G1 | wired |
| `getUsageSummary` | invoke | G1 | wired |
| `getUsageReport` | invoke | G1 | wired |
| `refreshUsage` | invoke | G1 | wired |
| `setUsageVisible` | invoke | G1 | wired |
| `onUsageChanged` | on | G1 | wired |
| `getConfig` | invoke | G1 | wired |
| `setConfig` | invoke | G1 | wired |
| `onConfigChange` | on | G1 | wired |
| `getKeybindings` | invoke | G1 | wired |
| `getKeybindingsPath` | invoke | G1 | wired |
| `getConfigPath` | invoke | G1 | wired |
| `onKeybindingsChange` | on | G1 | stub |
| `toggleGlobalYolo` | invoke | G1 | wired |
| `writeClipboardImage` | invoke | G2 | stub |
| `selectDirectory` | invoke | G2 | stub |
| `listDirectories` | invoke | G2 | stub |
| `listClaudeProfiles` | invoke | G1 | wired |
| `openInEditor` | invoke | G2 | stub |
| `openInApp` | invoke | G2 | stub |
| `openPath` | invoke | G2 | stub |
| `openExternal` | invoke | G2 | stub |
| `getInstalledEditors` | invoke | G2 | stub |
| `listClaudeSessions` | invoke | G1 | wired |
| `listProviderModels` | invoke | G1 | wired |
| `onConfirmQuit` | on | G1 | stub |
| `confirmQuit` | invoke | G1 | stub |
| `quitBackground` | invoke | G1 | stub |
| `startHeadlessRun` | invoke | G2 | stub |
| `listHeadlessRuns` | invoke | G2 | stub |
| `getHeadlessRun` | invoke | G2 | stub |
| `cancelHeadlessRun` | invoke | G2 | stub |
| `getHeadlessRunLog` | invoke | G2 | stub |
| `onHeadlessRunEvent` | on | G2 | stub |
| `logEvent` | send | G1 | stub |
| `exportDiagnostics` | invoke | G2 | stub |
| `getUpdateState` | invoke | G2 | stub |
| `checkForUpdates` | invoke | G2 | stub |
| `downloadUpdate` | invoke | G2 | stub |
| `installUpdateAndRestart` | invoke | G2 | stub |
| `runBrewUpgrade` | invoke | G2 | stub |
| `openUpdateDownload` | invoke | G2 | stub |
| `onUpdateStateChange` | on | G2 | stub |
| `getMcpServerStatus` | invoke | G1 | wired |
| `onNotification` | on | G1 | wired |
| `dismissNotification` | send | G1 | wired |
| `readDir` | invoke | G2 | stub |
| `readFile` | invoke | G2 | stub |
| `writeFile` | invoke | G2 | stub |
| `watchDir` | send | G2 | stub |
| `unwatchDir` | send | G2 | stub |
| `searchFiles` | invoke | G2 | stub |
| `onFsWatchEvent` | on | G2 | stub |
| `getGitStatus` | invoke | G2 | stub |
| `getGitLog` | invoke | G2 | stub |
| `getGitDiff` | invoke | G2 | stub |
| `getGitDiffStats` | invoke | G2 | stub |
| `gitCommit` | invoke | G2 | stub |
| `gitPush` | invoke | G2 | stub |
| `gitListBranches` | invoke | G2 | stub |
| `gitCheckout` | invoke | G2 | stub |
| `gitCreateBranch` | invoke | G2 | stub |
| `gitWorktreeCreate` | invoke | G2 | stub |
| `gitWorktreeRemove` | invoke | G2 | stub |
| `gitFileContents` | invoke | G2 | stub |
| `gitFetchPr` | invoke | G2 | stub |
| `gitPrFileDiff` | invoke | G2 | stub |
| `enableRemoteControl` | invoke | G2 | stub |
| `disableRemoteControl` | invoke | G2 | stub |
| `getRemoteControlState` | invoke | G2 | stub |
| `onRemoteStateChange` | on | G2 | stub |
| `scanSkills` | invoke | G2 | stub |
| `toggleSkill` | invoke | G2 | stub |
| `spawnTestTerminal` | invoke | G2 | stub |
| `sendTestTerminalInput` | send | G2 | stub |
| `resizeTestTerminal` | send | G2 | stub |
| `killTestTerminal` | send | G2 | stub |
| `onTestTerminalOutput` | on | G2 | stub |
| `onTestTerminalExit` | on | G2 | stub |
| `spawnFreeTerminal` | invoke | G2 | stub |
| `sendFreeTerminalInput` | send | G2 | stub |
| `resizeFreeTerminal` | send | G2 | stub |
| `killFreeTerminal` | send | G2 | stub |
| `activateFreeTerminal` | send | G2 | stub |
| `getFreeTerminalBuffer` | invoke | G2 | stub |
| `getFreeTerminalLayout` | invoke | G2 | stub |
| `listFreeTerminals` | invoke | G2 | stub |
| `onFreeTerminalOutput` | on | G2 | stub |
| `onFreeTerminalExit` | on | G2 | stub |
| `onFreeTerminalLayoutChanged` | on | G2 | stub |
G1 rows still `stub` on purpose: `logEvent` (no daemon request for renderer logs yet), `onKeybindingsChange` (no keybinding editing before G2),
`confirmQuit` / `quitBackground` / `onConfirmQuit` (Tauri quit flow, G3).

Notes: `enableRemoteControl`, `disableRemoteControl`, `getRemoteControlState`, `onRemoteStateChange` are deferred to P11 (kept
so the API shape matches; the UI is removed). `confirmQuit`, `quitBackground`, `onConfirmQuit` are the Electron quit flow: no-ops
until the Tauri quit flow (G3). Hydra's preload has no window-control methods, so there is nothing else Electron-only to stub.
