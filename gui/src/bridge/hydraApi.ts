// The `window.hydra` API surface, kept identical to Hydra's `electron/preload.ts` `hydraApi` (MIT, jpdlr) so the
// copied renderer runs unchanged. Every method forwards to a `Bridge` (request/response over a `Transport`, plus an event
// hub for the `on*` subscriptions). The method table below is generated from the preload file; do not reorder or rename.
//
// Status of each method (stub / wired): see ./README.md.
import type { KeybindingRule } from '@shared/keybindings'
import type { UsageReport, UsageReportProvider } from '@shared/usage/report'
import type { UsagePeriod, UsageState, UsageSummary } from '@shared/usage/types'
import type { AgentDetails, HistoryResponse, OverviewEvent, OverviewSnapshot, WaitsResponse } from '@shared/overview/types'
import type {
  CreateAgentPayload,
  AgentState,
  AgentOutputPayload,
  AgentStatusPayload,
  AppConfig,
  PreflightResult,
  ProviderId,
  ProviderModelOption,
  ClaudeSessionSummary,
  ClaudeProfile,
  ListClaudeSessionsOptions,
  HeadlessRun,
  ListHeadlessRunsOptions,
  HeadlessRunLogOptions,
  HeadlessRunLogPayload,
  StartHeadlessRunPayload,
  HeadlessRunEventPayload,
  ObservabilityLogEventPayload,
  ExportDiagnosticsResult,
  McpServerStatus,
  HydraNotification,
  AppUpdateState,
  FsDirEntry,
  FsReadFileResult,
  FsSearchResult,
  FsWatchEventPayload,
  DirSuggestion,
  GitStatus,
  GitCommit,
  GitBranch,
  GitFileContents,
  GitPrDiff,
  GitDiffStats,
  EditorId,
  RemoteControlState,
  SkillScanResult,
  SkillTogglePayload,
  FreeTerminalLayout
} from '@shared/types'
import type { Bridge } from './bridge'

export interface HydraAPI {
  preflight: (provider?: ProviderId) => Promise<PreflightResult>
  createAgent: (payload: CreateAgentPayload) => Promise<AgentState>
  killAgent: (agentId: string) => Promise<boolean>
  removeAgent: (agentId: string) => Promise<boolean>
  restartAgent: (agentId: string) => Promise<AgentState | null>
  listAgents: () => Promise<AgentState[]>
  toggleYolo: (agentId: string, yolo: boolean) => Promise<AgentState | null>
  renameAgent: (agentId: string, name: string) => Promise<AgentState | null>
  setAgentModel: (agentId: string, model: string) => Promise<AgentState | null>
  getAgentBuffer: (agentId: string) => Promise<string[]>
  sendInput: (agentId: string, input: string) => void
  sendRawInput: (agentId: string, data: string) => void
  resizeAgent: (agentId: string, cols: number, rows: number) => void
  broadcast: (projectDir: string, input: string) => Promise<string[]>
  onAgentOutput: (callback: (payload: AgentOutputPayload) => void) => () => void
  onAgentStatus: (callback: (payload: AgentStatusPayload) => void) => () => void
  getOverview: () => Promise<OverviewSnapshot>
  getOverviewAgentDetails: (agentId: string) => Promise<AgentDetails | null>
  getOverviewHistory: (sinceMs: number) => Promise<HistoryResponse>
  getOverviewWaits: () => Promise<WaitsResponse>
  onOverviewEvents: (callback: (events: OverviewEvent[]) => void) => () => void
  onOverviewSnapshot: (callback: (snapshot: OverviewSnapshot) => void) => () => void
  getUsageState: () => Promise<UsageState>
  getUsageSummary: (period: UsagePeriod, projectKey?: string) => Promise<UsageSummary>
  getUsageReport: (days: number, provider: UsageReportProvider) => Promise<UsageReport>
  refreshUsage: () => Promise<UsageState>
  setUsageVisible: () => Promise<void>
  onUsageChanged: (callback: (state: UsageState) => void) => () => void
  getConfig: () => Promise<AppConfig>
  setConfig: (partial: Partial<AppConfig>) => Promise<AppConfig>
  onConfigChange: (callback: (config: AppConfig) => void) => () => void
  getKeybindings: () => Promise<KeybindingRule[]>
  getKeybindingsPath: () => Promise<string>
  getConfigPath: () => Promise<string>
  onKeybindingsChange: (callback: (bindings: KeybindingRule[]) => void) => () => void
  toggleGlobalYolo: (enabled: boolean) => Promise<string[]>
  writeClipboardImage: (dataUrl: string) => Promise<boolean>
  selectDirectory: () => Promise<string | null>
  listDirectories: (query: string) => Promise<DirSuggestion[]>
  listClaudeProfiles: () => Promise<ClaudeProfile[]>
  openInEditor: (dir: string) => Promise<boolean>
  openInApp: (editorId: EditorId, dir: string) => Promise<boolean>
  openPath: (targetPath: string) => Promise<boolean>
  openExternal: (url: string) => Promise<boolean>
  getInstalledEditors: () => Promise<EditorId[]>
  listClaudeSessions: (options?: ListClaudeSessionsOptions) => Promise<ClaudeSessionSummary[]>
  listProviderModels: (provider: ProviderId) => Promise<ProviderModelOption[]>
  onConfirmQuit: (callback: (runningCount: number) => void) => () => void
  confirmQuit: () => Promise<boolean>
  quitBackground: () => Promise<boolean>
  startHeadlessRun: (payload: StartHeadlessRunPayload) => Promise<HeadlessRun>
  listHeadlessRuns: (options?: ListHeadlessRunsOptions) => Promise<HeadlessRun[]>
  getHeadlessRun: (runId: string) => Promise<HeadlessRun | null>
  cancelHeadlessRun: (runId: string) => Promise<boolean>
  getHeadlessRunLog: (runId: string, options?: HeadlessRunLogOptions) => Promise<HeadlessRunLogPayload | null>
  onHeadlessRunEvent: (callback: (payload: HeadlessRunEventPayload) => void) => () => void
  logEvent: (payload: ObservabilityLogEventPayload) => void
  exportDiagnostics: () => Promise<ExportDiagnosticsResult>
  getUpdateState: () => Promise<AppUpdateState>
  checkForUpdates: () => Promise<AppUpdateState>
  downloadUpdate: () => Promise<AppUpdateState>
  installUpdateAndRestart: () => Promise<boolean>
  runBrewUpgrade: () => Promise<{ ok: boolean; error?: string }>
  openUpdateDownload: () => Promise<{ ok: boolean; error?: string }>
  onUpdateStateChange: (callback: (state: AppUpdateState) => void) => () => void
  getMcpServerStatus: () => Promise<McpServerStatus>
  onNotification: (callback: (notification: HydraNotification) => void) => () => void
  dismissNotification: (id: string) => void
  readDir: (agentId: string, dirPath: string) => Promise<FsDirEntry[]>
  readFile: (agentId: string, filePath: string) => Promise<FsReadFileResult>
  writeFile: (agentId: string, filePath: string, content: string) => Promise<boolean>
  watchDir: (agentId: string) => void
  unwatchDir: (agentId: string) => void
  searchFiles: (agentId: string, query: string, maxResults?: number) => Promise<FsSearchResult[]>
  onFsWatchEvent: (callback: (payload: FsWatchEventPayload) => void) => () => void
  getGitStatus: (projectDir: string) => Promise<GitStatus>
  getGitLog: (projectDir: string, limit?: number) => Promise<GitCommit[]>
  getGitDiff: (projectDir: string, filePath?: string) => Promise<string>
  getGitDiffStats: (projectDir: string) => Promise<GitDiffStats>
  gitCommit: (projectDir: string, message: string, files?: string[]) => Promise<string>
  gitPush: (projectDir: string) => Promise<void>
  gitListBranches: (projectDir: string) => Promise<GitBranch[]>
  gitCheckout: (projectDir: string, branchName: string) => Promise<void>
  gitCreateBranch: (projectDir: string, branchName: string, startPoint?: string) => Promise<void>
  gitWorktreeCreate: (projectDir: string, branchName: string) => Promise<{ worktreePath: string; branch: string }>
  gitWorktreeRemove: (projectDir: string, worktreePath: string, deleteBranch?: string) => Promise<void>
  gitFileContents: (projectDir: string, filePath: string) => Promise<GitFileContents>
  gitFetchPr: (projectDir: string, prIdentifier: string) => Promise<GitPrDiff>
  gitPrFileDiff: (projectDir: string, prNumber: number, filePath: string) => Promise<string>
  enableRemoteControl: () => Promise<RemoteControlState>
  disableRemoteControl: () => Promise<RemoteControlState>
  getRemoteControlState: () => Promise<RemoteControlState>
  onRemoteStateChange: (callback: (state: RemoteControlState) => void) => () => void
  scanSkills: () => Promise<SkillScanResult>
  toggleSkill: (payload: SkillTogglePayload) => Promise<{ success: boolean }>
  spawnTestTerminal: () => Promise<void>
  sendTestTerminalInput: (data: string) => void
  resizeTestTerminal: (cols: number, rows: number) => void
  killTestTerminal: () => void
  onTestTerminalOutput: (callback: (data: string) => void) => () => void
  onTestTerminalExit: (callback: (exitCode: number) => void) => () => void
  spawnFreeTerminal: (projectDir: string, options?: { cwd?: string; groupId?: string }) => Promise<{ terminalId: string; groupId: string; layout: FreeTerminalLayout }>
  sendFreeTerminalInput: (terminalId: string, data: string) => void
  resizeFreeTerminal: (terminalId: string, cols: number, rows: number) => void
  killFreeTerminal: (terminalId: string) => void
  activateFreeTerminal: (projectDir: string, groupId: string, paneId?: string) => void
  getFreeTerminalBuffer: (terminalId: string) => Promise<{ exists: boolean; data: string }>
  getFreeTerminalLayout: (projectDir: string) => Promise<FreeTerminalLayout>
  listFreeTerminals: () => Promise<Array<{ terminalId: string; projectDir: string; label: string; lastActivityAt: number; lastInputAt: number; exited: boolean }>>
  onFreeTerminalOutput: (callback: (terminalId: string, projectDir: string, data: string) => void) => () => void
  onFreeTerminalExit: (callback: (terminalId: string, projectDir: string, exitCode: number) => void) => () => void
  onFreeTerminalLayoutChanged: (callback: (projectDir: string, layout: FreeTerminalLayout) => void) => () => void
}

/** Build the complete `window.hydra` object on top of a bridge. */
export function createHydraApi(bridge: Bridge): HydraAPI {
  return {
    preflight: (provider?: ProviderId) => bridge.call<PreflightResult>('preflight', provider),
    createAgent: (payload: CreateAgentPayload) => bridge.call<AgentState>('createAgent', payload),
    killAgent: (agentId: string) => bridge.call<boolean>('killAgent', agentId),
    removeAgent: (agentId: string) => bridge.call<boolean>('removeAgent', agentId),
    restartAgent: (agentId: string) => bridge.call<AgentState | null>('restartAgent', agentId),
    listAgents: () => bridge.call<AgentState[]>('listAgents'),
    toggleYolo: (agentId: string, yolo: boolean) => bridge.call<AgentState | null>('toggleYolo', agentId, yolo),
    renameAgent: (agentId: string, name: string) => bridge.call<AgentState | null>('renameAgent', agentId, name),
    setAgentModel: (agentId: string, model: string) => bridge.call<AgentState | null>('setAgentModel', agentId, model),
    getAgentBuffer: (agentId: string) => bridge.call<string[]>('getAgentBuffer', agentId),
    sendInput: (agentId: string, input: string) => bridge.send('sendInput', agentId, input),
    sendRawInput: (agentId: string, data: string) => bridge.send('sendRawInput', agentId, data),
    resizeAgent: (agentId: string, cols: number, rows: number) => bridge.send('resizeAgent', agentId, cols, rows),
    broadcast: (projectDir: string, input: string) => bridge.call<string[]>('broadcast', projectDir, input),
    onAgentOutput: (callback: (payload: AgentOutputPayload) => void) => bridge.subscribe('onAgentOutput', callback),
    onAgentStatus: (callback: (payload: AgentStatusPayload) => void) => bridge.subscribe('onAgentStatus', callback),
    getOverview: () => bridge.call<OverviewSnapshot>('getOverview'),
    getOverviewAgentDetails: (agentId: string) => bridge.call<AgentDetails | null>('getOverviewAgentDetails', agentId),
    getOverviewHistory: (sinceMs: number) => bridge.call<HistoryResponse>('getOverviewHistory', sinceMs),
    getOverviewWaits: () => bridge.call<WaitsResponse>('getOverviewWaits'),
    onOverviewEvents: (callback: (events: OverviewEvent[]) => void) => bridge.subscribe('onOverviewEvents', callback),
    onOverviewSnapshot: (callback: (snapshot: OverviewSnapshot) => void) => bridge.subscribe('onOverviewSnapshot', callback),
    getUsageState: () => bridge.call<UsageState>('getUsageState'),
    getUsageSummary: (period: UsagePeriod, projectKey?: string) => bridge.call<UsageSummary>('getUsageSummary', period, projectKey),
    getUsageReport: (days: number, provider: UsageReportProvider) => bridge.call<UsageReport>('getUsageReport', days, provider),
    refreshUsage: () => bridge.call<UsageState>('refreshUsage'),
    setUsageVisible: () => bridge.call<void>('setUsageVisible'),
    onUsageChanged: (callback: (state: UsageState) => void) => bridge.subscribe('onUsageChanged', callback),
    getConfig: () => bridge.call<AppConfig>('getConfig'),
    setConfig: (partial: Partial<AppConfig>) => bridge.call<AppConfig>('setConfig', partial),
    onConfigChange: (callback: (config: AppConfig) => void) => bridge.subscribe('onConfigChange', callback),
    getKeybindings: () => bridge.call<KeybindingRule[]>('getKeybindings'),
    getKeybindingsPath: () => bridge.call<string>('getKeybindingsPath'),
    getConfigPath: () => bridge.call<string>('getConfigPath'),
    onKeybindingsChange: (callback: (bindings: KeybindingRule[]) => void) => bridge.subscribe('onKeybindingsChange', callback),
    toggleGlobalYolo: (enabled: boolean) => bridge.call<string[]>('toggleGlobalYolo', enabled),
    writeClipboardImage: (dataUrl: string) => bridge.call<boolean>('writeClipboardImage', dataUrl),
    selectDirectory: () => bridge.call<string | null>('selectDirectory'),
    listDirectories: (query: string) => bridge.call<DirSuggestion[]>('listDirectories', query),
    listClaudeProfiles: () => bridge.call<ClaudeProfile[]>('listClaudeProfiles'),
    openInEditor: (dir: string) => bridge.call<boolean>('openInEditor', dir),
    openInApp: (editorId: EditorId, dir: string) => bridge.call<boolean>('openInApp', editorId, dir),
    openPath: (targetPath: string) => bridge.call<boolean>('openPath', targetPath),
    openExternal: (url: string) => bridge.call<boolean>('openExternal', url),
    getInstalledEditors: () => bridge.call<EditorId[]>('getInstalledEditors'),
    listClaudeSessions: (options?: ListClaudeSessionsOptions) => bridge.call<ClaudeSessionSummary[]>('listClaudeSessions', options),
    listProviderModels: (provider: ProviderId) => bridge.call<ProviderModelOption[]>('listProviderModels', provider),
    onConfirmQuit: (callback: (runningCount: number) => void) => bridge.subscribe('onConfirmQuit', callback),
    confirmQuit: () => bridge.call<boolean>('confirmQuit'),
    quitBackground: () => bridge.call<boolean>('quitBackground'),
    startHeadlessRun: (payload: StartHeadlessRunPayload) => bridge.call<HeadlessRun>('startHeadlessRun', payload),
    listHeadlessRuns: (options?: ListHeadlessRunsOptions) => bridge.call<HeadlessRun[]>('listHeadlessRuns', options),
    getHeadlessRun: (runId: string) => bridge.call<HeadlessRun | null>('getHeadlessRun', runId),
    cancelHeadlessRun: (runId: string) => bridge.call<boolean>('cancelHeadlessRun', runId),
    getHeadlessRunLog: (runId: string, options?: HeadlessRunLogOptions) => bridge.call<HeadlessRunLogPayload | null>('getHeadlessRunLog', runId, options),
    onHeadlessRunEvent: (callback: (payload: HeadlessRunEventPayload) => void) => bridge.subscribe('onHeadlessRunEvent', callback),
    logEvent: (payload: ObservabilityLogEventPayload) => bridge.send('logEvent', payload),
    exportDiagnostics: () => bridge.call<ExportDiagnosticsResult>('exportDiagnostics'),
    getUpdateState: () => bridge.call<AppUpdateState>('getUpdateState'),
    checkForUpdates: () => bridge.call<AppUpdateState>('checkForUpdates'),
    downloadUpdate: () => bridge.call<AppUpdateState>('downloadUpdate'),
    installUpdateAndRestart: () => bridge.call<boolean>('installUpdateAndRestart'),
    runBrewUpgrade: () => bridge.call<{ ok: boolean; error?: string }>('runBrewUpgrade'),
    openUpdateDownload: () => bridge.call<{ ok: boolean; error?: string }>('openUpdateDownload'),
    onUpdateStateChange: (callback: (state: AppUpdateState) => void) => bridge.subscribe('onUpdateStateChange', callback),
    getMcpServerStatus: () => bridge.call<McpServerStatus>('getMcpServerStatus'),
    onNotification: (callback: (notification: HydraNotification) => void) => bridge.subscribe('onNotification', callback),
    dismissNotification: (id: string) => bridge.send('dismissNotification', id),
    readDir: (agentId: string, dirPath: string) => bridge.call<FsDirEntry[]>('readDir', agentId, dirPath),
    readFile: (agentId: string, filePath: string) => bridge.call<FsReadFileResult>('readFile', agentId, filePath),
    writeFile: (agentId: string, filePath: string, content: string) => bridge.call<boolean>('writeFile', agentId, filePath, content),
    watchDir: (agentId: string) => bridge.send('watchDir', agentId),
    unwatchDir: (agentId: string) => bridge.send('unwatchDir', agentId),
    searchFiles: (agentId: string, query: string, maxResults?: number) => bridge.call<FsSearchResult[]>('searchFiles', agentId, query, maxResults),
    onFsWatchEvent: (callback: (payload: FsWatchEventPayload) => void) => bridge.subscribe('onFsWatchEvent', callback),
    getGitStatus: (projectDir: string) => bridge.call<GitStatus>('getGitStatus', projectDir),
    getGitLog: (projectDir: string, limit?: number) => bridge.call<GitCommit[]>('getGitLog', projectDir, limit),
    getGitDiff: (projectDir: string, filePath?: string) => bridge.call<string>('getGitDiff', projectDir, filePath),
    getGitDiffStats: (projectDir: string) => bridge.call<GitDiffStats>('getGitDiffStats', projectDir),
    gitCommit: (projectDir: string, message: string, files?: string[]) => bridge.call<string>('gitCommit', projectDir, message, files),
    gitPush: (projectDir: string) => bridge.call<void>('gitPush', projectDir),
    gitListBranches: (projectDir: string) => bridge.call<GitBranch[]>('gitListBranches', projectDir),
    gitCheckout: (projectDir: string, branchName: string) => bridge.call<void>('gitCheckout', projectDir, branchName),
    gitCreateBranch: (projectDir: string, branchName: string, startPoint?: string) => bridge.call<void>('gitCreateBranch', projectDir, branchName, startPoint),
    gitWorktreeCreate: (projectDir: string, branchName: string) => bridge.call<{ worktreePath: string; branch: string }>('gitWorktreeCreate', projectDir, branchName),
    gitWorktreeRemove: (projectDir: string, worktreePath: string, deleteBranch?: string) => bridge.call<void>('gitWorktreeRemove', projectDir, worktreePath, deleteBranch),
    gitFileContents: (projectDir: string, filePath: string) => bridge.call<GitFileContents>('gitFileContents', projectDir, filePath),
    gitFetchPr: (projectDir: string, prIdentifier: string) => bridge.call<GitPrDiff>('gitFetchPr', projectDir, prIdentifier),
    gitPrFileDiff: (projectDir: string, prNumber: number, filePath: string) => bridge.call<string>('gitPrFileDiff', projectDir, prNumber, filePath),
    enableRemoteControl: () => bridge.call<RemoteControlState>('enableRemoteControl'),
    disableRemoteControl: () => bridge.call<RemoteControlState>('disableRemoteControl'),
    getRemoteControlState: () => bridge.call<RemoteControlState>('getRemoteControlState'),
    onRemoteStateChange: (callback: (state: RemoteControlState) => void) => bridge.subscribe('onRemoteStateChange', callback),
    scanSkills: () => bridge.call<SkillScanResult>('scanSkills'),
    toggleSkill: (payload: SkillTogglePayload) => bridge.call<{ success: boolean }>('toggleSkill', payload),
    spawnTestTerminal: () => bridge.call<void>('spawnTestTerminal'),
    sendTestTerminalInput: (data: string) => bridge.send('sendTestTerminalInput', data),
    resizeTestTerminal: (cols: number, rows: number) => bridge.send('resizeTestTerminal', cols, rows),
    killTestTerminal: () => bridge.send('killTestTerminal'),
    onTestTerminalOutput: (callback: (data: string) => void) => bridge.subscribe('onTestTerminalOutput', callback),
    onTestTerminalExit: (callback: (exitCode: number) => void) => bridge.subscribe('onTestTerminalExit', callback),
    spawnFreeTerminal: (projectDir: string, options?: { cwd?: string; groupId?: string }) => bridge.call<{ terminalId: string; groupId: string; layout: FreeTerminalLayout }>('spawnFreeTerminal', projectDir, options),
    sendFreeTerminalInput: (terminalId: string, data: string) => bridge.send('sendFreeTerminalInput', terminalId, data),
    resizeFreeTerminal: (terminalId: string, cols: number, rows: number) => bridge.send('resizeFreeTerminal', terminalId, cols, rows),
    killFreeTerminal: (terminalId: string) => bridge.send('killFreeTerminal', terminalId),
    activateFreeTerminal: (projectDir: string, groupId: string, paneId?: string) => bridge.send('activateFreeTerminal', projectDir, groupId, paneId),
    getFreeTerminalBuffer: (terminalId: string) => bridge.call<{ exists: boolean; data: string }>('getFreeTerminalBuffer', terminalId),
    getFreeTerminalLayout: (projectDir: string) => bridge.call<FreeTerminalLayout>('getFreeTerminalLayout', projectDir),
    listFreeTerminals: () => bridge.call<Array<{ terminalId: string; projectDir: string; label: string; lastActivityAt: number; lastInputAt: number; exited: boolean }>>('listFreeTerminals'),
    onFreeTerminalOutput: (callback: (terminalId: string, projectDir: string, data: string) => void) => bridge.subscribe('onFreeTerminalOutput', callback),
    onFreeTerminalExit: (callback: (terminalId: string, projectDir: string, exitCode: number) => void) => bridge.subscribe('onFreeTerminalExit', callback),
    onFreeTerminalLayoutChanged: (callback: (projectDir: string, layout: FreeTerminalLayout) => void) => bridge.subscribe('onFreeTerminalLayoutChanged', callback),
  }
}
