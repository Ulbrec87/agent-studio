// @vitest-environment jsdom

import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import App from './App'
import type { AgentState, AppConfig, PreflightResult, ViewMode } from '@shared/types'
import { DEFAULT_KEYBINDINGS } from '@shared/keybindings'
import { RUNNING_PROJECT_ID } from '@shared/types'

const mockUpdateConfig = vi.fn()
const mockUseAgents = vi.fn()
const mockUseViewMode = vi.fn()

vi.mock('./hooks/useConfig', () => ({
  useConfig: () => ({
    config: currentConfig,
    updateConfig: mockUpdateConfig
  })
}))

vi.mock('./hooks/useKeybindings', () => ({
  useKeybindings: () => ({
    keybindings: DEFAULT_KEYBINDINGS,
    keybindingsPath: '/tmp/keybindings.json'
  })
}))

vi.mock('./hooks/useAgents', () => ({
  useAgents: () => mockUseAgents()
}))

vi.mock('./hooks/useViewMode', () => ({
  useViewMode: () => mockUseViewMode()
}))

vi.mock('./hooks/useUpdates', () => ({
  useUpdates: () => ({
    updateState: {
      supported: false,
      platform: 'darwin',
      checking: false,
      available: false,
      downloaded: false,
      downloading: false,
      currentVersion: '0.1.0',
      latestVersion: null,
      releaseDate: null,
      releaseNotes: null,
      error: null,
      canAutoInstall: false,
      installMethod: null,
      downloadUrl: null,
      releaseUrl: null
    },
    check: vi.fn(),
    download: vi.fn(),
    install: vi.fn(),
    runBrewUpgrade: vi.fn(),
    openDownload: vi.fn()
  })
}))

vi.mock('./components/Header/Header', () => ({
  Header: (props: {
    onOpenSettings: () => void
    onOpenHeadless: () => void
    onOpenUsage: () => void
    onSelectViewMode: (mode: ViewMode) => void
  }) => (
    <div>
      <button onClick={() => props.onSelectViewMode('overview')}>show-overview</button>
      <button onClick={() => props.onSelectViewMode('chat')}>show-chat</button>
      <button onClick={() => props.onSelectViewMode('grid')}>show-grid</button>
      <button onClick={props.onOpenSettings}>settings</button>
      <button onClick={props.onOpenHeadless}>headless</button>
      <button onClick={props.onOpenUsage}>usage</button>
    </div>
  )
}))

vi.mock('./components/Sidebar/Sidebar', () => ({
  Sidebar: (props: { onNewAgent: () => void; onSelectAgent: (id: string) => void }) => (
    <aside aria-label="Projects and agents">
      <button onClick={props.onNewAgent}>new-agent</button>
      <button onClick={() => props.onSelectAgent('agent-jump')}>sidebar-open-agent</button>
    </aside>
  )
}))

vi.mock('./components/ChatView/ChatView', () => ({
  ChatView: () => <div>chat-view</div>
}))

vi.mock('./components/GridView/GridView', () => ({
  GridView: (props: {
    selectedProject: string | null
    onSelectProject: (projectDir: string) => void
    onRemoveAgent: (id: string) => void
    onOpenInChat: (id: string) => void
  }) => (
    <div>
      <span>grid-project:{props.selectedProject ?? 'none'}</span>
      <button onClick={() => props.onSelectProject('__running__')}>grid-select-running</button>
      <button onClick={() => props.onOpenInChat('agent-from-grid')}>grid-open-chat</button>
      <button onClick={() => props.onRemoveAgent('sess-abc12345')}>remove-imported</button>
    </div>
  )
}))

vi.mock('./hooks/useOverview', () => ({
  useOverviewSnapshot: () => ({ snapshot: null, loading: false, error: null }),
  needsYouCount: () => 0
}))

vi.mock('./hooks/useUsage', () => ({
  useUsageState: () => null
}))

vi.mock('./components/Overview/OverviewView', () => ({
  OverviewView: (props: { onJumpToAgent: (agentId: string) => void }) => (
    <div>
      overview-view
      <button onClick={() => props.onJumpToAgent('agent-jump')}>jump-agent</button>
    </div>
  )
}))

vi.mock('./components/Settings/SettingsPanel', () => ({
  SettingsPanel: () => <div>settings-panel</div>
}))

vi.mock('./components/NewAgent/NewAgentDialog', () => ({
  NewAgentDialog: () => <div>new-agent-dialog</div>
}))

vi.mock('./components/Headless/HeadlessPanel', () => ({
  HeadlessPanel: () => <div>headless-panel</div>
}))

vi.mock('./components/Updates/UpdatePanel', () => ({
  UpdatePanel: () => <div>update-panel</div>
}))

vi.mock('./components/Notifications/NotificationToast', () => ({
  NotificationToast: () => null
}))

vi.mock('./hooks/useEditorPanel', () => ({
  useEditorPanel: () => ({
    isOpen: false,
    toggle: vi.fn(),
    tabs: [],
    activeTabPath: null,
    fileContents: new Map(),
    openFile: vi.fn(),
    closeTab: vi.fn(),
    selectTab: vi.fn(),
    updateContent: vi.fn(),
    saveFile: vi.fn()
  })
}))

const baseConfig: AppConfig = {
  schemaVersion: 1,
  defaultProvider: 'claude',
  defaultModel: 'sonnet',
  globalYolo: false,
  maxAgents: 8,
  theme: 'dark',
  defaultViewMode: 'chat',
  defaultProjectDir: '/tmp',
  defaultEditor: 'vscode',
  importSessionsOnStartup: true,
  sessionImportLimit: 500,
  sessionImportProjectPrefix: '',
  hiddenSessionIds: [],
  enableSoundEffects: true,
  enableRemoteErrorReporting: false,
  errorReportingEndpoint: '',
  includeSensitiveDiagnostics: false,
  sessionMaxAgeDays: 7,
  remoteControlEnabled: false,
  remoteSessionTimeoutMinutes: 480,
  gridColumns: 'auto',
  terminalShellMode: 'auto',
  terminalShellPath: '',
  terminalShellArgs: '',
  terminalFontFamily: '"SF Mono", "Menlo", monospace',
  terminalFontSize: 12,
  terminalCursorStyle: 'bar',
  terminalCursorBlink: false,
  terminalEnableWebgl: false,
  freeTerminalLifecyclePolicy: 'explicit',
  freeTerminalMaxCount: 8,
  freeTerminalIdleTimeoutMinutes: 30,
  freeTerminalScrollbackLines: 5000,
  gitPanelDisplayMode: 'overlay',
  editorPanelDisplayMode: 'overlay',
  claudeProfileByProject: {},
  usage: { alertsEnabled: false, sessionAlertPercent: 80, weeklyAlertPercent: 90, chipProvider: 'auto', showCost: true }
}

let currentConfig: AppConfig = { ...baseConfig }
let currentViewMode: ViewMode = 'chat'

function createAgent(id: string, overrides: Partial<AgentState> = {}): AgentState {
  return {
    id,
    name: 'Agent',
    projectDir: '/tmp/project',
    provider: 'claude',
    model: 'sonnet',
    yolo: false,
    isManager: false,
    sessionId: null,
    initialPrompt: '',
    createdAt: '2026-01-01T00:00:00.000Z',
    status: 'idle',
    pid: null,
    restartCount: 0,
    startedAt: null,
    endedAt: null,
    lastActivityAt: '2026-01-01T00:00:00.000Z',
    workMode: 'local',
    worktreePath: null,
    worktreeBranch: null,
    ...overrides
  }
}

function buildAgentsState(importedSessionId: string | null = null) {
  const imported = createAgent('sess-abc12345', {
    sessionId: importedSessionId,
    projectDir: '/tmp/project'
  })

  const map = new Map([
    [
      imported.id,
      {
        state: imported,
        rawOutput: ''
      }
    ]
  ])

  return {
    agents: map,
    agentList: [imported],
    projectGroups: [
      {
        projectDir: '/tmp/project',
        projectName: 'project',
        agents: [imported]
      }
    ],
    selectedAgentId: imported.id,
    selectedAgent: map.get(imported.id),
    setSelectedAgentId: vi.fn(),
    createAgent: vi.fn(),
    killAgent: vi.fn().mockResolvedValue(true),
    removeAgent: vi.fn().mockResolvedValue(true),
    restartAgent: vi.fn().mockResolvedValue(imported),
    toggleYolo: vi.fn().mockResolvedValue(imported),
    renameAgent: vi.fn().mockResolvedValue(null),
    setAgentModel: vi.fn().mockResolvedValue(imported),
    sendInput: vi.fn(),
    sendTerminalInput: vi.fn(),
    resizeTerminal: vi.fn(),
    broadcastInput: vi.fn().mockResolvedValue([])
  }
}

function setupViewModeMock() {
  const setViewMode = vi.fn((mode: ViewMode) => {
    currentViewMode = mode
  })
  const toggleViewMode = vi.fn(() => {
    currentViewMode = currentViewMode === 'chat' ? 'grid' : 'chat'
  })
  mockUseViewMode.mockImplementation(() => ({
    viewMode: currentViewMode,
    setViewMode,
    toggleViewMode
  }))
}

describe('App flow behavior', () => {
  afterEach(cleanup)

  beforeEach(() => {
    window.localStorage.clear()
    currentConfig = { ...baseConfig }
    currentViewMode = currentConfig.defaultViewMode
    mockUpdateConfig.mockReset()
    mockUpdateConfig.mockImplementation(async (partial: Partial<AppConfig>) => {
      currentConfig = { ...currentConfig, ...partial }
      return currentConfig
    })
    setupViewModeMock()
    mockUseAgents.mockReset()
    mockUseAgents.mockReturnValue(buildAgentsState())

    // Ensure FS methods are always available on window.hydra
    window.hydra = {
      ...window.hydra,
      readDir: vi.fn().mockResolvedValue([]),
      readFile: vi.fn().mockResolvedValue({ content: '', path: '' }),
      writeFile: vi.fn().mockResolvedValue(true),
      watchDir: vi.fn(),
      unwatchDir: vi.fn(),
      onFsWatchEvent: vi.fn().mockReturnValue(() => undefined),
      searchFiles: vi.fn().mockResolvedValue([]),
      listProviderModels: vi.fn().mockResolvedValue([]),
      enableRemoteControl: vi.fn().mockResolvedValue({ enabled: false, status: 'creating', sessionId: null, qrPayload: null, connectedAt: null, expiresAt: null, mobileConnected: false, error: null }),
      disableRemoteControl: vi.fn().mockResolvedValue({ enabled: false, status: 'creating', sessionId: null, qrPayload: null, connectedAt: null, expiresAt: null, mobileConnected: false, error: null }),
      getRemoteControlState: vi.fn().mockResolvedValue({ enabled: false, status: 'creating', sessionId: null, qrPayload: null, connectedAt: null, expiresAt: null, mobileConnected: false, error: null }),
      onRemoteStateChange: vi.fn().mockReturnValue(() => undefined),
      getAgentBuffer: vi.fn().mockResolvedValue([]),
      quitBackground: vi.fn().mockResolvedValue(true),
      getKeybindings: vi.fn().mockResolvedValue(DEFAULT_KEYBINDINGS),
      getKeybindingsPath: vi.fn().mockResolvedValue('/tmp/keybindings.json'),
      getConfigPath: vi.fn().mockResolvedValue('/tmp/config.json'),
      onKeybindingsChange: vi.fn().mockReturnValue(() => undefined)
    }
  })

  it('blocks opening new agent when preflight fails', async () => {
    const failedPreflight: PreflightResult = {
      ok: false,
      claudePath: null,
      version: null,
      error: 'Claude CLI missing'
    }

    window.hydra = {
      ...window.hydra,
      preflight: vi.fn().mockResolvedValue(failedPreflight),
      onConfirmQuit: vi.fn().mockReturnValue(() => undefined),
      confirmQuit: vi.fn().mockResolvedValue(true),
      onNotification: vi.fn().mockReturnValue(() => undefined),
      dismissNotification: vi.fn()
    }

    render(<App />)
    fireEvent.click(screen.getByText('new-agent'))

    expect(await screen.findByText('No Supported CLI Found')).toBeTruthy()
    expect(screen.queryByText('new-agent-dialog')).toBeNull()
  })

  it('shows quit confirmation and confirms forced quit', async () => {
    let quitListener: ((count: number) => void) | null = null

    window.hydra = {
      ...window.hydra,
      preflight: vi.fn().mockResolvedValue({
        ok: true,
        claudePath: '/usr/local/bin/claude',
        version: '1.0.0',
        error: null
      } satisfies PreflightResult),
      onConfirmQuit: vi.fn().mockImplementation((cb: (count: number) => void) => {
        quitListener = cb
        return () => undefined
      }),
      confirmQuit: vi.fn().mockResolvedValue(true),
      onNotification: vi.fn().mockReturnValue(() => undefined),
      dismissNotification: vi.fn()
    }

    render(<App />)
    await waitFor(() => expect(quitListener).not.toBeNull())
    if (!quitListener) {
      throw new Error('Quit listener was not registered')
    }
    ;(quitListener as (count: number) => void)(2)
    expect(await screen.findByText('2 agents are still running')).toBeTruthy()

    fireEvent.click(screen.getByText('Quit All'))
    await waitFor(() => {
      expect(window.hydra.confirmQuit).toHaveBeenCalledTimes(1)
    })
  })

  it('adds removed imported session ids to hidden config list', async () => {
    currentConfig = {
      ...baseConfig,
      defaultViewMode: 'grid',
      hiddenSessionIds: []
    }
    currentViewMode = 'grid'
    setupViewModeMock()
    mockUseAgents.mockReturnValue(buildAgentsState('session-hidden-01'))

    window.hydra = {
      ...window.hydra,
      preflight: vi.fn().mockResolvedValue({
        ok: true,
        claudePath: '/usr/local/bin/claude',
        version: '1.0.0',
        error: null
      } satisfies PreflightResult),
      onConfirmQuit: vi.fn().mockReturnValue(() => undefined),
      confirmQuit: vi.fn().mockResolvedValue(true),
      onNotification: vi.fn().mockReturnValue(() => undefined),
      dismissNotification: vi.fn()
    }

    render(<App />)
    fireEvent.click(screen.getByText('remove-imported'))

    await waitFor(() => {
      expect(mockUpdateConfig).toHaveBeenCalledWith(
        expect.objectContaining({
          hiddenSessionIds: ['session-hidden-01']
        })
      )
    })
  })

  it('switches to the Overview and back to Chat', async () => {
    const { rerender } = render(<App />)
    expect(await screen.findByText('chat-view')).toBeTruthy()

    fireEvent.click(screen.getByText('show-overview'))
    expect(currentViewMode).toBe('overview')
    rerender(<App />)
    expect(await screen.findByText('overview-view')).toBeTruthy()
    expect(screen.queryByText('chat-view')).toBeNull()

    fireEvent.click(screen.getByText('show-chat'))
    expect(currentViewMode).toBe('chat')
    rerender(<App />)
    expect(await screen.findByText('chat-view')).toBeTruthy()
  })

  it.each(['grid', 'overview'] as const)('keeps sidebar navigation available in %s', async (mode) => {
    window.localStorage.setItem('agentStudio.workspace-ui:v1', JSON.stringify({ viewMode: mode }))
    currentViewMode = mode
    setupViewModeMock()
    const state = buildAgentsState()
    mockUseAgents.mockReturnValue(state)
    render(<App />)
    expect(screen.getByRole('complementary', { name: 'Projects and agents' })).toBeTruthy()
    fireEvent.click(await screen.findByText('sidebar-open-agent'))
    expect(state.setSelectedAgentId).toHaveBeenCalledWith('agent-jump')
    expect(currentViewMode).toBe('chat')
  })

  it('restores a persisted Overview view mode', async () => {
    window.localStorage.setItem(
      'agentStudio.workspace-ui:v1',
      JSON.stringify({ viewMode: 'overview' })
    )
    currentViewMode = 'overview'
    setupViewModeMock()
    render(<App />)
    expect(await screen.findByText('overview-view')).toBeTruthy()
  })

  it('opens the Overview with its keyboard shortcut', async () => {
    render(<App />)
    await screen.findByText('chat-view')
    fireEvent.keyDown(window, { key: 'O', ctrlKey: true, shiftKey: true })
    expect(currentViewMode).toBe('overview')
  })

  it('jumps from the Overview to an agent in the Chat view', async () => {
    window.localStorage.setItem('agentStudio.workspace-ui:v1', JSON.stringify({ viewMode: 'overview' }))
    currentViewMode = 'overview'
    setupViewModeMock()
    const state = buildAgentsState()
    mockUseAgents.mockReturnValue(state)
    render(<App />)

    fireEvent.click(await screen.findByText('jump-agent'))

    expect(state.setSelectedAgentId).toHaveBeenCalledWith('agent-jump')
    expect(currentViewMode).toBe('chat')
  })

  it('opens a grid tile in the Chat view when its header is clicked', async () => {
    window.localStorage.setItem('agentStudio.workspace-ui:v1', JSON.stringify({ viewMode: 'grid' }))
    currentViewMode = 'grid'
    setupViewModeMock()
    const state = buildAgentsState()
    mockUseAgents.mockReturnValue(state)
    render(<App />)

    fireEvent.click(await screen.findByText('grid-open-chat'))

    expect(state.setSelectedAgentId).toHaveBeenCalledWith('agent-from-grid')
    expect(currentViewMode).toBe('chat')
  })

  it('keeps the grid Running tab selected after visiting the Chat view', async () => {
    window.localStorage.setItem('agentStudio.workspace-ui:v1', JSON.stringify({ viewMode: 'grid' }))
    currentViewMode = 'grid'
    setupViewModeMock()
    const { rerender } = render(<App />)

    expect(await screen.findByText('grid-project:/tmp/project')).toBeTruthy()
    fireEvent.click(screen.getByText('grid-select-running'))
    expect(await screen.findByText(`grid-project:${RUNNING_PROJECT_ID}`)).toBeTruthy()

    fireEvent.click(screen.getByText('show-chat'))
    rerender(<App />)
    expect(await screen.findByText('chat-view')).toBeTruthy()

    fireEvent.click(screen.getByText('show-grid'))
    rerender(<App />)
    expect(await screen.findByText(`grid-project:${RUNNING_PROJECT_ID}`)).toBeTruthy()
  })
})
