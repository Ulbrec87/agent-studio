import { PROVIDER_MODELS, type AgentState, type AppConfig, type ProviderId } from '@shared/types'
import { DEFAULT_KEYBINDINGS } from '@shared/keybindings'
import { summarize } from '@shared/usage/summary'
import { buildUsageReport, type UsageReportProvider } from '@shared/usage/report'
import type { UsagePeriod } from '@shared/usage/types'
import {
  MOCK_APP_INFO,
  defaultConfig,
  emptyTerminalLayout,
  emptyUsageState,
  fixtureAgents,
  overviewSnapshot,
  sampleTerminalBuffer,
  upToDateState
} from './fixtures'
import type { AppInfo, HydraEvent, HydraRequest, Transport } from './transport'

export interface MockTransport extends Transport {
  /** Test/dev hook: deliver an event to subscribers as the daemon would. */
  emit(channel: string, ...args: unknown[]): void
}

type Handler = (...args: never[]) => unknown

const noop = (): undefined => undefined
const resolveFalse = (): boolean => false

export function createMockTransport(options: { appInfo?: AppInfo } = {}): MockTransport {
  const info = options.appInfo ?? MOCK_APP_INFO
  const subscribers = new Set<(e: unknown) => void>()
  let config: AppConfig = defaultConfig()
  let agents: AgentState[] = fixtureAgents()
  let nextId = 1

  const emit = (channel: string, ...args: unknown[]): void => {
    const event: HydraEvent = { channel, args }
    for (const cb of [...subscribers]) cb(event)
  }

  const updateAgent = (id: string, patch: Partial<AgentState>): AgentState | null => {
    const found = agents.find((a) => a.id === id)
    if (!found) return null
    const updated = { ...found, ...patch }
    agents = agents.map((a) => (a.id === id ? updated : a))
    return updated
  }

  // One entry per `window.hydra` method (see ./README.md). Everything is a documented stub in G0.
  const handlers: Record<string, Handler> = {
    // Preflight
    preflight: () => ({ ok: true, claudePath: '/usr/local/bin/claude', version: 'stub', error: null }),

    // Agent lifecycle
    createAgent: ((payload: Partial<AgentState> & { name: string; projectDir: string }) => {
      const created: AgentState = {
        ...fixtureAgents()[0],
        ...payload,
        id: `mock${(nextId++).toString(16).padStart(4, '0')}`,
        status: 'running',
        createdAt: new Date().toISOString(),
        lastActivityAt: new Date().toISOString(),
        startedAt: new Date().toISOString(),
        endedAt: null
      }
      agents = [...agents, created]
      return created
    }) as Handler,
    killAgent: ((id: string) => updateAgent(id, { status: 'idle', pid: null }) !== null) as Handler,
    removeAgent: ((id: string) => {
      const before = agents.length
      agents = agents.filter((a) => a.id !== id)
      return agents.length < before
    }) as Handler,
    restartAgent: ((id: string) => updateAgent(id, { status: 'running', pid: 5000 })) as Handler,
    listAgents: () => agents.map((a) => ({ ...a })),
    toggleYolo: ((id: string, yolo: boolean) => updateAgent(id, { yolo })) as Handler,
    renameAgent: ((id: string, name: string) => updateAgent(id, { name })) as Handler,
    setAgentModel: ((id: string, model: string) => updateAgent(id, { model })) as Handler,
    getAgentBuffer: ((id: string) => sampleTerminalBuffer(agents.find((a) => a.id === id)?.name ?? id)) as Handler,

    // Agent I/O (sends)
    sendInput: noop,
    sendRawInput: noop,
    resizeAgent: noop,
    broadcast: (() => []) as Handler,

    // Connection state (always connected: there is no daemon)
    getDaemonStatus: () => ({ connected: true }),
    previewHydraImport: () => ({
      dir: '~/.config/Hydra',
      configChanges: [{ key: 'theme', current: 'dark', incoming: 'midnight' }],
      agentsTotal: 3,
      agentsNew: 2,
      warnings: ['1 agent(s) already exist or could not be read and will be skipped']
    }),
    applyHydraImport: () => ({ configKeysApplied: 1, agentsAdded: [], warnings: [] }),

    // Overview
    getOverview: () => overviewSnapshot(agents),
    getOverviewAgentDetails: () => null,
    getOverviewHistory: ((sinceMs: number) => ({
      since: sinceMs,
      startedAt: Date.now(),
      starts: [],
      points: [],
      events: [],
      agents: {}
    })) as Handler,
    getOverviewWaits: () => ({ ledger: { transitions: [], spans: [], starts: [] } }),

    // Usage & limits
    getUsageState: () => emptyUsageState(),
    getUsageSummary: ((period: UsagePeriod, projectKey?: string) =>
      summarize([], period, new Date(), { projectKey })) as Handler,
    getUsageReport: ((days: number, provider: UsageReportProvider) =>
      buildUsageReport([], { days, provider, now: new Date() })) as Handler,
    refreshUsage: () => emptyUsageState(),
    setUsageVisible: noop,

    // Config
    getConfig: () => structuredClone(config),
    setConfig: ((partial: Partial<AppConfig>) => {
      config = { ...config, ...partial }
      const next = structuredClone(config)
      emit('onConfigChange', next)
      return next
    }) as Handler,
    getKeybindings: () => DEFAULT_KEYBINDINGS.map((r) => ({ ...r })),
    getKeybindingsPath: () => `${info.dataDir}/keybindings.json`,
    getConfigPath: () => `${info.dataDir}/config.json`,
    toggleGlobalYolo: ((enabled: boolean) => {
      config = { ...config, globalYolo: enabled }
      agents = agents.map((a) => ({ ...a, yolo: enabled }))
      return agents.map((a) => a.id)
    }) as Handler,

    // Clipboard, dialogs, shell (native; G2)
    writeClipboardImage: resolveFalse,
    selectDirectory: () => null,
    listDirectories: () => [],
    listClaudeProfiles: () => [],
    openInEditor: resolveFalse,
    openInApp: resolveFalse,
    openPath: resolveFalse,
    openExternal: ((url: string) => {
      // A browser can at least open links; the desktop shell opens them natively in G2.
      if (typeof window !== 'undefined' && /^https?:\/\//.test(url)) window.open(url, '_blank', 'noopener')
      return true
    }) as Handler,
    getInstalledEditors: () => [],

    // Session catalog
    listClaudeSessions: () => [],
    listProviderModels: ((provider: ProviderId) => PROVIDER_MODELS[provider].map((m) => ({ ...m }))) as Handler,

    // App quit flow: Electron-only dialog plumbing, a no-op until the Tauri quit flow lands (G3)
    confirmQuit: resolveFalse,
    quitBackground: resolveFalse,

    // Headless runs
    startHeadlessRun: () => Promise.reject(new Error('Headless runs are not wired yet (G2)')),
    listHeadlessRuns: () => [],
    getHeadlessRun: () => null,
    cancelHeadlessRun: resolveFalse,
    getHeadlessRunLog: () => null,

    // Observability
    logEvent: noop,
    exportDiagnostics: () => ({ path: null, error: 'Diagnostics export is not wired yet (G2)' }),

    // App updates: always "up to date" (the desktop app has no self-updater; plan §4)
    getUpdateState: () => upToDateState(info),
    checkForUpdates: () => upToDateState(info),
    downloadUpdate: () => upToDateState(info),
    installUpdateAndRestart: resolveFalse,
    runBrewUpgrade: () => ({ ok: false, error: 'Not available in Agent Studio' }),
    openUpdateDownload: () => ({ ok: false, error: 'Not available yet' }),

    // MCP
    getMcpServerStatus: () => ({ running: false, port: null, error: null, managerWorkspace: null }),

    // Notifications
    dismissNotification: noop,

    // File system (editor panel)
    readDir: () => [],
    readFile: () => Promise.reject(new Error('File access is not wired yet (G2)')),
    writeFile: resolveFalse,
    watchDir: noop,
    unwatchDir: noop,
    searchFiles: () => [],

    // Git
    getGitStatus: () => ({ branch: 'main', ahead: 0, behind: 0, modified: [], staged: [], untracked: [] }),
    getGitLog: () => [],
    getGitDiff: () => '',
    getGitDiffStats: () => ({ additions: 0, deletions: 0, files: 0 }),
    gitCommit: () => Promise.reject(new Error('Git is not wired yet (G2)')),
    gitPush: () => Promise.reject(new Error('Git is not wired yet (G2)')),
    gitListBranches: () => [],
    gitCheckout: () => Promise.reject(new Error('Git is not wired yet (G2)')),
    gitCreateBranch: () => Promise.reject(new Error('Git is not wired yet (G2)')),
    gitWorktreeCreate: () => Promise.reject(new Error('Git is not wired yet (G2)')),
    gitWorktreeRemove: () => Promise.reject(new Error('Git is not wired yet (G2)')),
    gitFileContents: () => ({ original: '', modified: '', language: 'plaintext' }),
    gitFetchPr: () => Promise.reject(new Error('Pull requests are not wired yet (G2)')),
    gitPrFileDiff: () => '',

    // Remote control: deferred (P11). Kept so the API shape matches Hydra's; the UI for it is removed.
    enableRemoteControl: () => remoteState(),
    disableRemoteControl: () => remoteState(),
    getRemoteControlState: () => remoteState(),

    // Skills
    scanSkills: () => ({ claude: [], codex: [], opencode: [], scannedAt: new Date().toISOString() }),
    toggleSkill: () => ({ success: false }),

    // Test terminal (preflight modal)
    spawnTestTerminal: noop,
    sendTestTerminalInput: noop,
    resizeTestTerminal: noop,
    killTestTerminal: noop,

    // Free terminals
    spawnFreeTerminal: () => Promise.reject(new Error('Free terminals are not wired yet (G2)')),
    sendFreeTerminalInput: noop,
    resizeFreeTerminal: noop,
    killFreeTerminal: noop,
    activateFreeTerminal: noop,
    getFreeTerminalBuffer: () => ({ exists: false, data: '' }),
    getFreeTerminalLayout: ((projectDir: string) => emptyTerminalLayout(projectDir)) as Handler,
    listFreeTerminals: () => []
  }

  return {
    async request(req) {
      if (typeof req === 'object' && req !== null) {
        if ((req as { kind?: unknown }).kind === 'app_info') return { ...info }
        const { method, args } = req as Partial<HydraRequest>
        if (typeof method === 'string') {
          const handler = handlers[method]
          if (!handler) throw new Error(`mock transport: no handler for ${method}`)
          return (handler as (...a: unknown[]) => unknown)(...(Array.isArray(args) ? args : []))
        }
      }
      throw new Error('mock transport: unsupported request')
    },
    onEvent(cb) {
      subscribers.add(cb)
      return () => {
        subscribers.delete(cb)
      }
    },
    emit
  }
}

function remoteState() {
  return {
    enabled: false,
    status: 'creating' as const,
    sessionId: null,
    qrPayload: null,
    connectedAt: null,
    expiresAt: null,
    mobileConnected: false,
    error: 'Remote control is not available in Agent Studio yet'
  }
}
