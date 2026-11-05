// The mapping layer: Hydra's `window.hydra` methods -> `as-proto` requests, and daemon events -> Hydra's `on*` channels.
// `createDaemonTransport` is a Hydra-level `Transport` (`{ method, args }` in, `{ channel, args }` events out) on top of a
// raw protocol transport (Tauri commands or the dev-bridge WebSocket). Methods that are not wired yet (G2) fall through to
// the fallback (the in-memory mock), so the UI never hits a missing method.
import { DEFAULT_KEYBINDINGS } from '@shared/keybindings'
import type { AgentState, AppConfig, CreateAgentPayload, ListClaudeSessionsOptions, ProviderId } from '@shared/types'
import type { UsagePeriod } from '@shared/usage/types'
import type { UsageReportProvider } from '@shared/usage/report'
import { APP_INFO_REQUEST, type AppInfo, type HydraEvent, type HydraRequest, type RawTransport, type Transport } from './transport'
import { bytesToBase64, toError, utf8Bytes, type DaemonStatus, type ProtoEvent, type ProtoRequest, type ProtoResponse } from './protocol'
import { compact, expectPayload, mapMcpStatus, mapNotification, mapPreflight, splitConfigPatch, toHydraConfig } from './mappers'
import { createAgentStreams } from './streams'

export interface DaemonTransportOptions {
  raw: RawTransport
  /** Answers every method that is not wired to the daemon yet. */
  fallback: Transport
  appInfo?: AppInfo
  /** Where GUI-only settings live (default: `window.localStorage`, guarded). */
  storage?: Pick<Storage, 'getItem' | 'setItem'> | null
}

export interface DaemonTransport extends Transport {
  dispose(): void
}

const GUI_CONFIG_KEY = 'agent-studio.gui-config'

/** The Hydra methods this layer answers itself (everything else goes to the fallback). Used by tests and the README check. */
export const WIRED_METHODS = [
  'preflight', 'createAgent', 'killAgent', 'removeAgent', 'restartAgent', 'listAgents', 'toggleYolo', 'renameAgent',
  'setAgentModel', 'getAgentBuffer', 'sendInput', 'sendRawInput', 'resizeAgent', 'broadcast', 'getOverview',
  'getOverviewAgentDetails', 'getOverviewHistory', 'getOverviewWaits', 'getUsageState', 'getUsageSummary', 'getUsageReport',
  'refreshUsage', 'setUsageVisible', 'getConfig', 'setConfig', 'getKeybindings', 'getKeybindingsPath', 'getConfigPath',
  'toggleGlobalYolo', 'listClaudeProfiles', 'listClaudeSessions', 'listProviderModels', 'getMcpServerStatus',
  'dismissNotification', 'getDaemonStatus'
] as const

/** The `on*` channels this layer emits from daemon events (the other `on*` channels are G2 and never fire yet). */
export const WIRED_CHANNELS = [
  'onAgentOutput', 'onAgentStatus', 'onOverviewEvents', 'onOverviewSnapshot', 'onUsageChanged', 'onConfigChange', 'onNotification'
] as const

function defaultStorage(): Pick<Storage, 'getItem' | 'setItem'> | null {
  try {
    return typeof window !== 'undefined' ? window.localStorage : null
  } catch {
    return null
  }
}

export function createDaemonTransport(options: DaemonTransportOptions): DaemonTransport {
  const { raw, fallback } = options
  const storage = options.storage === undefined ? defaultStorage() : options.storage
  const listeners = new Set<(e: unknown) => void>()
  let status: DaemonStatus = { connected: false, error: 'connecting to the daemon' }
  let knownIds = new Set<string>()
  let appInfo = options.appInfo

  const emit = (channel: string, ...args: unknown[]): void => {
    const event: HydraEvent = { channel, args }
    for (const cb of [...listeners]) cb(event)
  }

  const send = (type: string, payload?: unknown): Promise<ProtoResponse> =>
    raw.request(payload === undefined ? { type } : { type, payload }) as Promise<ProtoResponse>

  const call = async <T>(type: string, payload: unknown, ...expected: string[]): Promise<T> =>
    expectPayload<T>(await send(type, payload), ...expected)

  const streams = createAgentStreams({
    send: (req: ProtoRequest) => raw.request(req) as Promise<ProtoResponse>,
    emit: (agentId, data) => emit('onAgentOutput', { agentId, data })
  })

  // ── GUI-only settings overlay ────────────────────────────────────────────
  let overlayMemory: Record<string, unknown> = {}
  const readOverlay = (): Record<string, unknown> => {
    try {
      const text = storage?.getItem(GUI_CONFIG_KEY)
      const parsed: unknown = text ? JSON.parse(text) : {}
      return parsed && typeof parsed === 'object' ? (parsed as Record<string, unknown>) : {}
    } catch {
      return {}
    }
  }
  const writeOverlay = (patch: Record<string, unknown>): void => {
    try {
      storage?.setItem(GUI_CONFIG_KEY, JSON.stringify({ ...readOverlay(), ...patch }))
    } catch {
      // Storage unavailable (private window): the setting lasts for this session only.
      overlayMemory = { ...overlayMemory, ...patch }
    }
  }
  const overlay = (): Record<string, unknown> => ({ ...readOverlay(), ...overlayMemory })

  // ── agents ───────────────────────────────────────────────────────────────
  const isLive = (a: AgentState): boolean => a.status !== 'idle'

  /** Start streaming every live agent; stop streaming (and forget) the ones that disappeared. */
  const syncAgents = async (agents: AgentState[]): Promise<void> => {
    const ids = new Set(agents.map((a) => a.id))
    for (const id of knownIds) if (!ids.has(id)) streams.drop(id)
    knownIds = ids
    await Promise.all(
      agents.filter(isLive).map((a) =>
        streams.ensure(a.id).catch((err) => console.warn(`[bridge] subscribe ${a.id} failed`, err))
      )
    )
  }

  const listAgents = async (): Promise<AgentState[]> => {
    const agents = await call<AgentState[]>('list_agents', undefined, 'agents')
    void syncAgents(agents)
    return agents
  }

  const withAgent = async (type: string, payload: unknown): Promise<AgentState | null> => {
    const agent = await call<AgentState | null>(type, payload, 'agent')
    if (agent) {
      knownIds.add(agent.id)
      if (isLive(agent)) void streams.ensure(agent.id).catch(() => undefined)
    }
    return agent
  }

  /** Keystrokes and resizes of one agent must reach the daemon in order, but concurrent invokes carry no ordering guarantee. */
  const queues = new Map<string, Promise<unknown>>()
  const serial = <T>(agentId: string, job: () => Promise<T>): Promise<T> => {
    const run = (queues.get(agentId) ?? Promise.resolve()).catch(() => undefined).then(job)
    queues.set(agentId, run)
    void run.then(
      () => queues.get(agentId) === run && queues.delete(agentId),
      () => queues.get(agentId) === run && queues.delete(agentId)
    )
    return run
  }

  const methods: Record<string, (...args: never[]) => unknown> = {
    // Preflight
    preflight: (async (provider?: ProviderId) =>
      mapPreflight(await call('preflight', { provider: provider ?? null }, 'preflight'), provider)) as never,

    // Agent lifecycle
    createAgent: (async (payload: CreateAgentPayload) => {
      let body = { ...payload }
      if (payload.isManager) {
        const mcp = mapMcpStatus(await call('get_mcp_status', undefined, 'mcp_status'))
        if (!mcp.running || !mcp.managerWorkspace) throw new Error('MCP server is not running — cannot create manager agent')
        body = { ...body, projectDir: mcp.managerWorkspace }
      }
      const agent = await call<AgentState>('create_agent', compact(body as unknown as Record<string, unknown>), 'agent')
      knownIds.add(agent.id)
      await streams.ensure(agent.id).catch((err) => console.warn('[bridge] subscribe failed', err))
      return agent
    }) as never,
    killAgent: (async (agentId: string) => call<boolean>('kill_agent', { agent_id: agentId }, 'flag')) as never,
    removeAgent: (async (agentId: string) => {
      const removed = await call<boolean>('remove_agent', { agent_id: agentId }, 'flag')
      if (removed) {
        streams.drop(agentId)
        knownIds.delete(agentId)
      }
      return removed
    }) as never,
    restartAgent: (async (agentId: string) => {
      streams.resetDecoder(agentId)
      return withAgent('restart_agent', { agent_id: agentId })
    }) as never,
    listAgents: listAgents as never,
    toggleYolo: ((agentId: string, yolo: boolean) => withAgent('toggle_yolo', { agent_id: agentId, yolo })) as never,
    renameAgent: ((agentId: string, name: string) => withAgent('rename_agent', { agent_id: agentId, name })) as never,
    setAgentModel: ((agentId: string, model: string) => withAgent('set_model', { agent_id: agentId, model })) as never,
    getAgentBuffer: ((agentId: string) => streams.snapshot(agentId)) as never,
    broadcast: ((projectDir: string, input: string) =>
      call<string[]>('broadcast', { project_dir: projectDir, input }, 'ids')) as never,
    toggleGlobalYolo: (async (enabled: boolean) => {
      const before = await call<AgentState[]>('list_agents', undefined, 'agents')
      // Hydra stores the flag in the config and then toggles every agent; the daemon only does the latter.
      await call('update_config', { globalYolo: enabled }, 'config')
      await call('set_global_yolo', enabled, 'agents', 'ok')
      return before.filter((a) => a.yolo !== enabled).map((a) => a.id)
    }) as never,

    // Agent I/O: input strings become UTF-8 bytes for SendRaw
    sendInput: ((agentId: string, input: string) => serial(agentId, () => send('send_input', { agent_id: agentId, input }))) as never,
    sendRawInput: ((agentId: string, data: string) =>
      serial(agentId, () => send('send_raw', { agent_id: agentId, data: bytesToBase64(utf8Bytes(data)) }))) as never,
    resizeAgent: ((agentId: string, cols: number, rows: number) =>
      serial(agentId, () => send('resize', { agent_id: agentId, cols, rows }))) as never,

    // Overview
    getOverview: (() => call('get_overview', undefined, 'overview')) as never,
    getOverviewAgentDetails: ((agentId: string) => call('get_agent_details', { agent_id: agentId }, 'agent_details')) as never,
    getOverviewHistory: ((sinceMs: number) => call('get_overview_history', { since_ms: sinceMs }, 'overview_history')) as never,
    getOverviewWaits: (async () => ({ ledger: await call('get_overview_waits', undefined, 'overview_waits') })) as never,

    // Usage
    getUsageState: (() => call('get_usage_state', undefined, 'usage_state')) as never,
    getUsageSummary: ((period: UsagePeriod, projectKey?: string) =>
      call('get_usage_summary', compact({ period, project_key: projectKey }), 'usage_summary')) as never,
    getUsageReport: ((days: number, provider: UsageReportProvider) =>
      call('get_usage_report', { days, provider }, 'usage_report')) as never,
    refreshUsage: (() => call('refresh_usage', undefined, 'usage_state')) as never,
    setUsageVisible: (async () => {
      await send('usage_visible', true)
    }) as never,

    // Config
    getConfig: (async () => toHydraConfig(await call<Partial<AppConfig>>('get_config', undefined, 'config'), overlay())) as never,
    setConfig: (async (patch: Partial<AppConfig>) => {
      const { daemon, gui } = splitConfigPatch(patch)
      if (Object.keys(gui).length > 0) writeOverlay(gui)
      if (Object.keys(daemon).length === 0) {
        const current = toHydraConfig(await call<Partial<AppConfig>>('get_config', undefined, 'config'), overlay())
        emit('onConfigChange', current)
        return current
      }
      // The daemon answers with the new config and also pushes `config_changed` (-> onConfigChange).
      return toHydraConfig(await call<Partial<AppConfig>>('update_config', daemon, 'config'), overlay())
    }) as never,
    getKeybindings: (() => DEFAULT_KEYBINDINGS.map((r) => ({ ...r }))) as never,
    getKeybindingsPath: (() => dataPath('gui-keybindings.json')) as never,
    getConfigPath: (() => dataPath('config.json')) as never,

    // Sessions, profiles, models, MCP
    listClaudeProfiles: (() => call('list_claude_profiles', undefined, 'profiles')) as never,
    listClaudeSessions: ((opts?: ListClaudeSessionsOptions) =>
      call('list_sessions', compact({ provider: opts?.provider ?? 'claude', limit: opts?.limit, maxAgeDays: opts?.maxAgeDays, projectPathPrefix: opts?.projectPathPrefix, includeHidden: opts?.includeHidden }), 'sessions')) as never,
    listProviderModels: ((provider: ProviderId) => call('list_provider_models', { provider }, 'models')) as never,
    getMcpServerStatus: (async () => mapMcpStatus(await call('get_mcp_status', undefined, 'mcp_status'))) as never,

    // Notifications: the daemon dismisses nothing (toasts are renderer state)
    dismissNotification: (() => undefined) as never,

    // Agent Studio extensions
    getDaemonStatus: (() => ({ ...status })) as never
  }

  function dataPath(file: string): string {
    const root = appInfo?.dataDir ?? ''
    const sep = appInfo?.platform === 'win32' ? '\\' : '/'
    return root ? `${root.replace(/[\\/]+$/, '')}${sep}${file}` : file
  }

  // ── events ───────────────────────────────────────────────────────────────
  const onProtoEvent = (event: ProtoEvent): void => {
    const p = (event.payload ?? {}) as Record<string, unknown>
    switch (event.type) {
      case 'agent_output':
        streams.handleOutput(String(p.agent_id), String(p.data ?? ''))
        break
      case 'agent_status': {
        const agentId = String(p.agent_id)
        emit('onAgentStatus', { agentId, status: p.status, sessionId: p.session_id ?? null, model: p.model })
        if (p.status === 'idle' || p.status === 'errored') streams.resetDecoder(agentId)
        else if (!streams.has(agentId)) void streams.ensure(agentId).catch(() => undefined)
        break
      }
      case 'agents_changed':
        void call<AgentState[]>('list_agents', undefined, 'agents')
          .then(syncAgents)
          .then(() => emit('onAgentsChanged'))
          .catch((err) => console.warn('[bridge] agents refresh failed', err))
        break
      case 'config_changed':
        emit('onConfigChange', toHydraConfig(event.payload as Partial<AppConfig>, overlay()))
        break
      case 'notification':
        emit('onNotification', mapNotification(event.payload as Parameters<typeof mapNotification>[0]))
        break
      case 'overview_snapshot':
        emit('onOverviewSnapshot', event.payload)
        break
      case 'overview_events':
        emit('onOverviewEvents', event.payload)
        break
      case 'usage_changed':
        emit('onUsageChanged', event.payload)
        break
      default:
        // agent_waiting (covered by overview + notifications), headless/terminal/update events: G2.
        break
    }
  }

  const reportFocus = (focused: boolean): void => {
    send('client_focus', focused).catch(() => undefined)
  }

  /** After (re)connecting, subscriptions are gone: start over and tell the renderer to reload its agent list. */
  const resync = (): void => {
    streams.reset()
    knownIds = new Set()
    if (typeof document !== 'undefined') reportFocus(document.hasFocus())
    void call<AgentState[]>('list_agents', undefined, 'agents')
      .then(syncAgents)
      .then(() => emit('onAgentsChanged'))
      .catch((err) => console.warn('[bridge] resync failed', err))
  }

  const onStatus = (next: DaemonStatus): void => {
    const was = status.connected
    status = next
    if (!next.connected) streams.reset()
    emit('onDaemonStatus', { ...next })
    if (next.connected && !was) resync()
  }

  const offEvent = raw.onEvent(onProtoEvent)
  const offStatus = raw.onStatus(onStatus)
  void raw.status().then((s) => {
    if (s.connected !== status.connected || s.error !== status.error) onStatus(s)
  })
  const onFocus = (): void => {
    if (status.connected) reportFocus(true)
  }
  const onBlur = (): void => {
    if (status.connected) reportFocus(false)
  }
  if (typeof window !== 'undefined') {
    window.addEventListener('focus', onFocus)
    window.addEventListener('blur', onBlur)
  }
  const offFallback = fallback.onEvent((e) => {
    for (const cb of [...listeners]) cb(e)
  })

  return {
    async request(req: unknown): Promise<unknown> {
      if (typeof req === 'object' && req !== null && (req as { kind?: unknown }).kind === APP_INFO_REQUEST.kind) {
        appInfo = (await raw.request(req)) as AppInfo
        return appInfo
      }
      const { method, args } = req as HydraRequest
      const handler = methods[method]
      if (!handler) return fallback.request(req)
      try {
        return await (handler as (...a: unknown[]) => unknown)(...(args ?? []))
      } catch (err) {
        throw toError(err)
      }
    },
    onEvent(cb) {
      listeners.add(cb)
      return () => listeners.delete(cb)
    },
    dispose() {
      offEvent()
      offStatus()
      offFallback()
      if (typeof window !== 'undefined') {
        window.removeEventListener('focus', onFocus)
        window.removeEventListener('blur', onBlur)
      }
      listeners.clear()
    }
  }
}
