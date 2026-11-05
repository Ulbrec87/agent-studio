# Bridge: `window.hydra` for Agent Studio

`window.hydra` has exactly the shape of Hydra's `electron/preload.ts` (`HydraAPI` in `hydraApi.ts`), so the copied renderer runs
unchanged. Layers:

| File | Role |
|------|------|
| `transport.ts` | `Transport { request, onEvent }`, shared types (`HydraRequest`, `HydraEvent`, `AppInfo`), `selectTransport()` |
| `tauriTransport.ts` | desktop: `invoke('app_info')`, `invoke('daemon_request', { request })`, `listen('daemon-event')` |
| `mockTransport.ts` + `fixtures.ts` | in-memory answers for browsers, tests and (G0) the desktop app; `emit()` for tests |
| `bridge.ts` | `call` / `send` / `subscribe` over a transport; fans events out by channel |
| `hydraApi.ts` | generated method table: every method calls the bridge with its own name |
| `index.ts` | `installHydra()`: loads `app_info`, builds and installs `window.hydra` before the first render |

Wire format (G0): a request is `{ method: '<hydra method>', args: [...] }`, an event is `{ channel: '<onXxx name>', args: [...] }`.
`{ kind: 'app_info' }` is answered by the shell. In G1 `installHydra` passes the selected transport to `createBridge` and the
Tauri transport maps these to `as-proto` requests/events (add a mapping layer in TypeScript, keep the Rust side generic).

## Methods

Status: `stub` = answered by the mock (fixtures / empty states / documented no-op), `wired` = real daemon or native call.
Tick them off here as G1/G2 land. `send` = fire-and-forget (`ipcRenderer.send`), `on` = subscription returning an unsubscribe.

| Method | Kind | Phase | Status |
|--------|------|-------|--------|
| `preflight` | invoke | G1 | stub |
| `createAgent` | invoke | G1 | stub |
| `killAgent` | invoke | G1 | stub |
| `removeAgent` | invoke | G1 | stub |
| `restartAgent` | invoke | G1 | stub |
| `listAgents` | invoke | G1 | stub |
| `toggleYolo` | invoke | G1 | stub |
| `renameAgent` | invoke | G1 | stub |
| `setAgentModel` | invoke | G1 | stub |
| `getAgentBuffer` | invoke | G1 | stub |
| `sendInput` | send | G1 | stub |
| `sendRawInput` | send | G1 | stub |
| `resizeAgent` | send | G1 | stub |
| `broadcast` | invoke | G1 | stub |
| `onAgentOutput` | on | G1 | stub |
| `onAgentStatus` | on | G1 | stub |
| `getOverview` | invoke | G1 | stub |
| `getOverviewAgentDetails` | invoke | G1 | stub |
| `getOverviewHistory` | invoke | G1 | stub |
| `getOverviewWaits` | invoke | G1 | stub |
| `onOverviewEvents` | on | G1 | stub |
| `onOverviewSnapshot` | on | G1 | stub |
| `getUsageState` | invoke | G1 | stub |
| `getUsageSummary` | invoke | G1 | stub |
| `getUsageReport` | invoke | G1 | stub |
| `refreshUsage` | invoke | G1 | stub |
| `setUsageVisible` | invoke | G1 | stub |
| `onUsageChanged` | on | G1 | stub |
| `getConfig` | invoke | G1 | stub |
| `setConfig` | invoke | G1 | stub |
| `onConfigChange` | on | G1 | stub |
| `getKeybindings` | invoke | G1 | stub |
| `getKeybindingsPath` | invoke | G1 | stub |
| `getConfigPath` | invoke | G1 | stub |
| `onKeybindingsChange` | on | G1 | stub |
| `toggleGlobalYolo` | invoke | G1 | stub |
| `writeClipboardImage` | invoke | G2 | stub |
| `selectDirectory` | invoke | G2 | stub |
| `listDirectories` | invoke | G2 | stub |
| `listClaudeProfiles` | invoke | G1 | stub |
| `openInEditor` | invoke | G2 | stub |
| `openInApp` | invoke | G2 | stub |
| `openPath` | invoke | G2 | stub |
| `openExternal` | invoke | G2 | stub |
| `getInstalledEditors` | invoke | G2 | stub |
| `listClaudeSessions` | invoke | G1 | stub |
| `listProviderModels` | invoke | G1 | stub |
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
| `getMcpServerStatus` | invoke | G1 | stub |
| `onNotification` | on | G1 | stub |
| `dismissNotification` | send | G1 | stub |
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
Notes: `enableRemoteControl`, `disableRemoteControl`, `getRemoteControlState`, `onRemoteStateChange` are deferred to P11 (kept
so the API shape matches; the UI is removed). `confirmQuit`, `quitBackground`, `onConfirmQuit` are the Electron quit flow: no-ops
until the Tauri quit flow (G3). Hydra's preload has no window-control methods, so there is nothing else Electron-only to stub.
