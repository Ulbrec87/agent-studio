// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from 'vitest'
import { DEFAULT_CONFIG } from '@shared/types'
import { createBridge } from './bridge'
import { createDaemonTransport, type DaemonTransport } from './daemonApi'
import { createHydraApi, type HydraAPI } from './hydraApi'
import { createMockTransport } from './mockTransport'
import { base64ToBytes, type ProtoResponse } from './protocol'

// JSON recorded from a real daemon (agent-studio --dev-bridge, 2026-11-05).
const PREFLIGHT = [
  { provider: 'claude', result: { error: null, ok: true, path: '/home/u/.local/bin/claude', version: '2.1.318 (Claude Code)' } },
  { provider: 'codex', result: { error: 'Codex CLI not found.', ok: false, path: null, version: null } }
]
const NOTIFICATION = {
  agent_id: 'ddbc7683', body: 'shape is now running', id: 'c7f8128a2c19', kind: 'agent_started',
  timestamp: '2026-11-05T17:54:11.098996089+00:00', title: 'Agent Started'
}

import { AGENT, FakeRaw, IDLE, b64, tick } from './testKit'

let transport: DaemonTransport | null = null
afterEach(() => {
  transport?.dispose()
  transport = null
})

async function setup(configure?: (raw: FakeRaw) => void) {
  const raw = new FakeRaw()
  configure?.(raw)
  const store = new Map<string, string>()
  transport = createDaemonTransport({
    raw,
    fallback: createMockTransport(),
    appInfo: { name: 'Agent Studio', version: '1', dataDir: '/data', platform: 'linux' },
    storage: { getItem: (k) => store.get(k) ?? null, removeItem: (k) => void store.delete(k) },
    keybindingsPollMs: 0
  })
  const api: HydraAPI = createHydraApi(createBridge(transport))
  await tick() // let the initial status/resync settle
  raw.requests.length = 0
  return { raw, api, store }
}

describe('agents', () => {
  it('createAgent sends the camelCase payload without undefined keys, subscribes, and returns the daemon state', async () => {
    const NEW = { ...AGENT, id: 'newagent1' }
    const { raw, api } = await setup((r) => r.handlers.set('create_agent', () => ({ type: 'agent', payload: NEW })))
    const created = await api.createAgent({ name: 'shape', projectDir: '/tmp/asg1-proj', provider: 'claude', model: 'sonnet', yolo: false, initialPrompt: '', reasoningEffort: undefined })
    expect(created).toEqual(NEW)
    expect(raw.of('create_agent')[0].payload).toEqual({ name: 'shape', projectDir: '/tmp/asg1-proj', provider: 'claude', model: 'sonnet', yolo: false, initialPrompt: '' })
    expect(raw.of('subscribe')).toEqual([{ type: 'subscribe', payload: { agent_id: 'newagent1' } }])
  })

  it('createAgent forwards claudeConfigDir, including the empty default-profile value', async () => {
    const { raw, api } = await setup((r) => r.handlers.set('create_agent', () => ({ type: 'agent', payload: AGENT })))
    await api.createAgent({ name: 'p', projectDir: '/p', provider: 'claude', model: 'sonnet', yolo: false, initialPrompt: '', claudeConfigDir: '/home/me/.claude-work' })
    await api.createAgent({ name: 'd', projectDir: '/p', provider: 'claude', model: 'sonnet', yolo: false, initialPrompt: '', claudeConfigDir: '' })
    expect((raw.of('create_agent')[0].payload as { claudeConfigDir?: string }).claudeConfigDir).toBe('/home/me/.claude-work')
    expect((raw.of('create_agent')[1].payload as { claudeConfigDir?: string }).claudeConfigDir).toBe('')
  })

  it('manager agents are created in the MCP manager workspace and fail when MCP is down', async () => {
    const { raw, api } = await setup((r) => {
      r.handlers.set('get_mcp_status', () => ({ type: 'mcp_status', payload: { running: true, port: 1, error: null, manager_workspace: '/m' } }))
      r.handlers.set('create_agent', (p) => ({ type: 'agent', payload: { ...AGENT, projectDir: (p as { projectDir: string }).projectDir } }))
    })
    await api.createAgent({ name: 'm', projectDir: '', provider: 'claude', model: 'sonnet', yolo: false, initialPrompt: '', isManager: true })
    expect((raw.of('create_agent')[0].payload as { projectDir: string }).projectDir).toBe('/m')
    raw.handlers.set('get_mcp_status', () => ({ type: 'mcp_status', payload: { running: false, port: null, error: null, manager_workspace: null } }))
    await expect(api.createAgent({ name: 'm', projectDir: '', provider: 'claude', model: 'sonnet', yolo: false, initialPrompt: '', isManager: true })).rejects.toThrow(/MCP server is not running/)
  })

  it('kill/remove/restart/rename/model/yolo/broadcast map to the protocol', async () => {
    const { raw, api } = await setup((r) => {
      r.handlers.set('kill_agent', () => ({ type: 'flag', payload: true }))
      r.handlers.set('remove_agent', () => ({ type: 'flag', payload: true }))
      r.handlers.set('restart_agent', () => ({ type: 'agent', payload: AGENT }))
      r.handlers.set('rename_agent', () => ({ type: 'agent', payload: { ...AGENT, name: 'x' } }))
      r.handlers.set('set_model', () => ({ type: 'agent', payload: { ...AGENT, model: 'opus' } }))
      r.handlers.set('toggle_yolo', () => ({ type: 'agent', payload: { ...AGENT, yolo: true } }))
      r.handlers.set('broadcast', () => ({ type: 'ids', payload: ['ddbc7683'] }))
    })
    expect(await api.killAgent('a')).toBe(true)
    expect(await api.removeAgent('a')).toBe(true)
    expect((await api.restartAgent('a'))?.id).toBe('ddbc7683')
    expect((await api.renameAgent('a', 'x'))?.name).toBe('x')
    expect((await api.setAgentModel('a', 'opus'))?.model).toBe('opus')
    expect((await api.toggleYolo('a', true))?.yolo).toBe(true)
    expect(await api.broadcast('/p', 'hi')).toEqual(['ddbc7683'])
    expect(raw.of('rename_agent')[0].payload).toEqual({ agent_id: 'a', name: 'x' })
    expect(raw.of('set_model')[0].payload).toEqual({ agent_id: 'a', model: 'opus' })
    expect(raw.of('toggle_yolo')[0].payload).toEqual({ agent_id: 'a', yolo: true })
    expect(raw.of('broadcast')[0].payload).toEqual({ project_dir: '/p', input: 'hi' })
  })

  it('toggleGlobalYolo returns the agents that changed', async () => {
    const { raw, api } = await setup((r) => {
      r.handlers.set('set_global_yolo', () => ({ type: 'agents', payload: [] }))
      r.handlers.set('update_config', () => ({ type: 'config', payload: {} }))
    })
    expect(await api.toggleGlobalYolo(true)).toEqual(['ddbc7683', 'idle0001'])
    expect(raw.of('set_global_yolo')[0].payload).toBe(true)
    expect(raw.of('update_config')[0].payload).toEqual({ globalYolo: true })
  })

  it('preflight picks the provider and renames path to claudePath', async () => {
    const { raw, api } = await setup((r) => r.handlers.set('preflight', () => ({ type: 'preflight', payload: PREFLIGHT })))
    expect(await api.preflight()).toEqual({ ok: true, claudePath: '/home/u/.local/bin/claude', version: '2.1.318 (Claude Code)', error: null })
    expect(await api.preflight('codex')).toEqual({ ok: false, claudePath: null, version: null, error: 'Codex CLI not found.' })
    expect(raw.of('preflight')[0].payload).toEqual({ provider: null })
  })
})

describe('input, resize and output', () => {
  it('sends UTF-8 bytes as base64 and keeps one agent’s input in order', async () => {
    const order: string[] = []
    const { raw, api } = await setup((r) => {
      r.handlers.set('send_raw', async (p) => {
        const data = (p as { data: string }).data
        await tick(order.length === 0 ? 15 : 0) // the first one is slow: the second must still wait for it
        order.push(new TextDecoder().decode(base64ToBytes(data)))
        return { type: 'flag', payload: true }
      })
      r.handlers.set('resize', () => ({ type: 'ok' }))
      r.handlers.set('send_input', () => ({ type: 'flag', payload: true }))
    })
    api.sendRawInput('a', 'é€')
    api.sendRawInput('a', 'x')
    api.resizeAgent('a', 100, 30)
    api.sendInput('a', 'hello')
    await tick(60)
    expect(order).toEqual(['é€', 'x'])
    expect(raw.requests.map((r) => r.type)).toEqual(['send_raw', 'send_raw', 'resize', 'send_input'])
    expect(raw.of('resize')[0].payload).toEqual({ agent_id: 'a', cols: 100, rows: 30 })
    expect(raw.of('send_raw')[0].payload).toEqual({ agent_id: 'a', data: b64('é€') })
  })

  it('decodes output with a streaming decoder per agent (a character split across chunks survives)', async () => {
    const { raw, api } = await setup()
    const got: Array<{ agentId: string; data: string }> = []
    api.onAgentOutput((p) => got.push(p))
    await tick() // subscriptions of the live agent
    const euro = [0xe2, 0x82, 0xac]
    raw.push('agent_output', { agent_id: 'ddbc7683', data: b64([0x61, euro[0], euro[1]]) })
    raw.push('agent_output', { agent_id: 'other', data: b64([0xe2]) })
    raw.push('agent_output', { agent_id: 'ddbc7683', data: b64([euro[2], 0x62]) })
    raw.push('agent_output', { agent_id: 'other', data: b64([0x82, 0xac]) })
    expect(got.filter((g) => g.agentId === 'ddbc7683').map((g) => g.data).join('')).toBe('a€b')
    expect(got.filter((g) => g.agentId === 'other').map((g) => g.data).join('')).toBe('€')
  })

  it('getAgentBuffer returns the decoded daemon buffer', async () => {
    const { raw, api } = await setup((r) => r.handlers.set('subscribe', () => ({ type: 'buffer', payload: { data: b64('hello €') } })))
    expect(await api.getAgentBuffer('ddbc7683')).toEqual(['hello €'])
    expect(raw.of('subscribe')).toHaveLength(1)
    raw.handlers.set('subscribe', () => ({ type: 'buffer', payload: { data: '' } }))
    expect(await api.getAgentBuffer('ddbc7683')).toEqual([])
  })
})

describe('subscriptions', () => {
  it('subscribes only live agents on listAgents and stops tracking removed ones', async () => {
    const { raw, api } = await setup((r) => r.handlers.set('list_agents', () => ({ type: 'agents', payload: [] })))
    raw.handlers.set('list_agents', () => ({ type: 'agents', payload: [AGENT, IDLE] }))
    await api.listAgents()
    await tick()
    expect(raw.of('subscribe').map((r) => (r.payload as { agent_id: string }).agent_id)).toEqual(['ddbc7683'])
    raw.handlers.set('list_agents', () => ({ type: 'agents', payload: [IDLE] }))
    const changed = vi.fn()
    api.onAgentsChanged(changed)
    raw.push('agents_changed')
    await tick(5)
    expect(changed).toHaveBeenCalled()
    // The removed agent is forgotten: a new agent with the same id would subscribe again.
    raw.handlers.set('list_agents', () => ({ type: 'agents', payload: [AGENT] }))
    raw.push('agents_changed')
    await tick(5)
    expect(raw.of('subscribe')).toHaveLength(2)
  })

  it('subscribes when an agent starts running and keeps a single subscription', async () => {
    const { raw } = await setup()
    raw.push('agent_status', { agent_id: 'idle0001', status: 'running', session_id: null, model: 'sonnet' })
    raw.push('agent_status', { agent_id: 'idle0001', status: 'running', session_id: 's', model: 'sonnet' })
    await tick(5)
    expect(raw.of('subscribe')).toHaveLength(1)
  })

  it('subscribes again after a reconnect and asks the renderer to reload its agents', async () => {
    const { raw, api } = await setup()
    const status = vi.fn()
    const changed = vi.fn()
    api.onDaemonStatus(status)
    api.onAgentsChanged(changed)
    await api.listAgents()
    await tick()
    expect(raw.of('subscribe')).toHaveLength(0) // already subscribed by the initial sync
    raw.setStatus({ connected: false, error: 'daemon connection closed' })
    raw.setStatus({ connected: true })
    await tick(5)
    expect(status.mock.calls.map((c) => c[0].connected)).toEqual([false, true])
    expect(raw.of('subscribe').map((r) => (r.payload as { agent_id: string }).agent_id)).toEqual(['ddbc7683'])
    expect(changed).toHaveBeenCalled()
    expect(await api.getDaemonStatus()).toEqual({ connected: true })
  })

  it('holds live chunks until the first subscribe answered and drops the re-delivered last chunk', async () => {
    let answer: (r: ProtoResponse) => void = () => undefined
    const { raw, api } = await setup()
    raw.handlers.set('subscribe', () => new Promise<ProtoResponse>((res) => (answer = res)))
    const got: string[] = []
    api.onAgentOutput((p) => got.push(p.data))
    const buffer = api.getAgentBuffer('idle0001') // idle: not subscribed by the initial sync
    await tick()
    raw.push('agent_output', { agent_id: 'idle0001', data: b64('tail-1') }) // also the last bytes of the buffer: a repeat
    raw.push('agent_output', { agent_id: 'idle0001', data: b64('new') })
    expect(got).toEqual([])
    answer({ type: 'buffer', payload: { data: b64('old tail-1') } })
    expect(await buffer).toEqual(['old tail-1'])
    expect(got).toEqual(['new'])
    raw.push('agent_output', { agent_id: 'idle0001', data: b64('live') })
    expect(got).toEqual(['new', 'live'])
    expect(raw.of('subscribe')).toHaveLength(1)
  })
})

describe('events', () => {
  it('maps status, notification and config events to Hydra shapes', async () => {
    const { raw, api } = await setup()
    const status = vi.fn()
    const notification = vi.fn()
    const config = vi.fn()
    api.onAgentStatus(status)
    api.onNotification(notification)
    api.onConfigChange(config)
    raw.push('agent_status', { agent_id: 'ddbc7683', status: 'idle', session_id: 'sid', model: 'opus' })
    raw.push('notification', NOTIFICATION)
    raw.push('notification', { ...NOTIFICATION, agent_id: null, kind: 'usage_limit' })
    raw.push('config_changed', { theme: 'light' })
    expect(status).toHaveBeenCalledWith({ agentId: 'ddbc7683', status: 'idle', sessionId: 'sid', model: 'opus' })
    expect(notification.mock.calls[0][0]).toEqual({ id: 'c7f8128a2c19', type: 'agent_started', title: 'Agent Started', body: 'shape is now running', timestamp: NOTIFICATION.timestamp, agentId: 'ddbc7683' })
    expect(notification.mock.calls[1][0]).not.toHaveProperty('agentId')
    expect(config.mock.calls[0][0]).toEqual({ ...DEFAULT_CONFIG, theme: 'light' })
  })

  it('passes overview and usage pushes through', async () => {
    const { raw, api } = await setup()
    const snapshot = vi.fn()
    const events = vi.fn()
    const usage = vi.fn()
    api.onOverviewSnapshot(snapshot)
    api.onOverviewEvents(events)
    api.onUsageChanged(usage)
    raw.push('overview_snapshot', { at: 1, counts: {}, sessions: [] })
    raw.push('overview_events', [{ type: 'appeared' }])
    raw.push('usage_changed', { dataVersion: 2 })
    expect(snapshot).toHaveBeenCalledWith({ at: 1, counts: {}, sessions: [] })
    expect(events).toHaveBeenCalledWith([{ type: 'appeared' }])
    expect(usage).toHaveBeenCalledWith({ dataVersion: 2 })
  })

  it('reports the window focus to the daemon (notifications go to the OS only when unfocused)', async () => {
    const { raw } = await setup()
    window.dispatchEvent(new Event('blur'))
    window.dispatchEvent(new Event('focus'))
    await tick()
    expect(raw.of('client_focus').map((r) => r.payload)).toEqual([false, true])
  })
})

describe('config, sessions, overview, usage', () => {
  it('config: defaults for keys the daemon lacks, everything (terminal keys included) goes to the daemon', async () => {
    const { raw, api } = await setup((r) => {
      r.handlers.set('get_config', () => ({ type: 'config', payload: { theme: 'dark', maxAgents: 4 } }))
      r.handlers.set('update_config', (p) => ({ type: 'config', payload: { theme: 'dark', maxAgents: 4, ...(p as object) } }))
    })
    expect(await api.getConfig()).toEqual({ ...DEFAULT_CONFIG, theme: 'dark', maxAgents: 4 })
    const next = await api.setConfig({ theme: 'light', terminalFontSize: 18, terminalFontFamily: 'Fira Code', terminalEnableWebgl: true })
    expect(raw.of('update_config')[0].payload).toEqual({ theme: 'light', terminalFontSize: 18, terminalFontFamily: 'Fira Code', terminalEnableWebgl: true })
    expect(next).toMatchObject({ theme: 'light', terminalFontSize: 18, terminalFontFamily: 'Fira Code', terminalEnableWebgl: true })
    // The daemon's change event is the only change notification (no local echo).
    const changed = vi.fn()
    api.onConfigChange(changed)
    raw.push('config_changed', { theme: 'dark', terminalFontSize: 20 })
    expect(changed.mock.calls[0][0]).toMatchObject({ theme: 'dark', terminalFontSize: 20, terminalEnableWebgl: false })
  })

  it('migrates the terminal settings the old localStorage overlay kept, once, without overriding the config', async () => {
    const store = new Map([['agent-studio.gui-config', JSON.stringify({ terminalFontSize: 18, terminalFontFamily: 'Fira Code', terminalEnableWebgl: true })]])
    const raw = new FakeRaw()
    // The config already has a custom family: the overlay must not override it. The size and WebGL values are still defaults.
    raw.handlers.set('get_config', () => ({ type: 'config', payload: { terminalFontFamily: 'Mono', terminalFontSize: 12, terminalEnableWebgl: false } }))
    raw.handlers.set('update_config', (p) => ({ type: 'config', payload: p }))
    transport = createDaemonTransport({
      raw,
      fallback: createMockTransport(),
      appInfo: { name: 'Agent Studio', version: '1', dataDir: '/data', platform: 'linux' },
      storage: { getItem: (k) => store.get(k) ?? null, removeItem: (k) => void store.delete(k) },
      keybindingsPollMs: 0
    })
    await tick(20)
    expect(raw.of('update_config').map((r) => r.payload)).toEqual([{ terminalFontSize: 18, terminalEnableWebgl: true }])
    expect(store.has('agent-studio.gui-config')).toBe(false)
    // A reconnect does not run it again.
    raw.setStatus({ connected: false })
    raw.setStatus({ connected: true })
    await tick(20)
    expect(raw.of('update_config')).toHaveLength(1)
  })

  it('paths and keybindings are local', async () => {
    const { api } = await setup()
    expect(await api.getConfigPath()).toBe('/data/config.json')
    expect(await api.getKeybindingsPath()).toBe('/data/gui-keybindings.json')
    // No daemon answer for the keybindings file (older daemon): Hydra's defaults.
    expect((await api.getKeybindings()).length).toBeGreaterThan(5)
  })

  it('sessions, profiles, models and MCP status', async () => {
    const { raw, api } = await setup((r) => {
      r.handlers.set('list_sessions', () => ({ type: 'sessions', payload: [{ sessionId: 's1' }] }))
      r.handlers.set('list_claude_profiles', () => ({ type: 'profiles', payload: [{ configDir: '/h/.claude', isDefault: true, label: 'default' }] }))
      r.handlers.set('list_provider_models', () => ({ type: 'models', payload: [{ id: 'opus', label: 'Opus' }] }))
      r.handlers.set('get_mcp_status', () => ({ type: 'mcp_status', payload: { error: null, manager_workspace: '/m', port: 7, running: true } }))
    })
    expect(await api.listClaudeSessions({ limit: 5, projectPathPrefix: '/p' })).toEqual([{ sessionId: 's1' }])
    expect(raw.of('list_sessions')[0].payload).toEqual({ provider: 'claude', limit: 5, projectPathPrefix: '/p' })
    expect((await api.listClaudeProfiles())[0].label).toBe('default')
    expect((await api.listProviderModels('claude'))[0].id).toBe('opus')
    expect(await api.getMcpServerStatus()).toEqual({ running: true, port: 7, error: null, managerWorkspace: '/m' })
  })

  it('overview and usage requests', async () => {
    const ledger = { transitions: [], spans: [{ start: 1, end: 2 }], starts: [1] }
    const { raw, api } = await setup((r) => {
      r.handlers.set('get_overview', () => ({ type: 'overview', payload: { at: 1, counts: {}, sessions: [] } }))
      r.handlers.set('get_agent_details', () => ({ type: 'agent_details', payload: null }))
      r.handlers.set('get_overview_history', () => ({ type: 'overview_history', payload: { since: 5 } }))
      r.handlers.set('get_overview_waits', () => ({ type: 'overview_waits', payload: ledger }))
      r.handlers.set('get_usage_state', () => ({ type: 'usage_state', payload: { dataVersion: 1 } }))
      r.handlers.set('get_usage_summary', () => ({ type: 'usage_summary', payload: { period: 'today' } }))
      r.handlers.set('get_usage_report', () => ({ type: 'usage_report', payload: { daily: [] } }))
      r.handlers.set('refresh_usage', () => ({ type: 'usage_state', payload: { dataVersion: 2 } }))
      r.handlers.set('usage_visible', () => ({ type: 'ok' }))
    })
    expect((await api.getOverview()).at).toBe(1)
    expect(await api.getOverviewAgentDetails('a')).toBeNull()
    expect(await api.getOverviewHistory(5)).toEqual({ since: 5 })
    expect(raw.of('get_overview_history')[0].payload).toEqual({ since_ms: 5 })
    expect(await api.getOverviewWaits()).toEqual({ ledger })
    expect(await api.getUsageState()).toEqual({ dataVersion: 1 })
    await api.getUsageSummary('today', '/p')
    expect(raw.of('get_usage_summary')[0].payload).toEqual({ period: 'today', project_key: '/p' })
    await api.getUsageReport(7, 'all')
    expect(raw.of('get_usage_report')[0].payload).toEqual({ days: 7, provider: 'all' })
    expect(await api.refreshUsage()).toEqual({ dataVersion: 2 })
    await api.setUsageVisible()
    expect(raw.of('usage_visible')[0].payload).toBe(true)
  })
})

describe('errors and fallback', () => {
  it('a rejected request becomes an Error with the daemon message', async () => {
    const { api } = await setup((r) => r.handlers.set('kill_agent', () => Promise.reject('no such agent')))
    await expect(api.killAgent('x')).rejects.toThrow(new Error('no such agent'))
    await expect(api.killAgent('x')).rejects.toBeInstanceOf(Error)
  })

  it('an unexpected response variant is an error, not silent garbage', async () => {
    const { api } = await setup((r) => r.handlers.set('kill_agent', () => ({ type: 'pong' })))
    await expect(api.killAgent('x')).rejects.toThrow(/unexpected daemon response "pong"/)
  })

  it('the deferred remote-control methods (P11) still fall through to the mock instead of failing', async () => {
    const { raw, api } = await setup()
    expect((await api.getRemoteControlState()).enabled).toBe(false)
    expect(raw.requests).toHaveLength(0)
  })
})
