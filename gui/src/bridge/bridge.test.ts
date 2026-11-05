import { describe, expect, it, vi } from 'vitest'
import { DEFAULT_CONFIG } from '@shared/types'
import { createBridge } from './bridge'
import { createHydraApi } from './hydraApi'
import { createMockTransport } from './mockTransport'
import { APP_INFO_REQUEST } from './transport'

function setup() {
  const transport = createMockTransport({
    appInfo: { name: 'Agent Studio', version: '9.9.9', dataDir: '/data', platform: 'linux' }
  })
  const api = createHydraApi(createBridge(transport))
  return { transport, api }
}

describe('mock transport', () => {
  it('answers app_info with the configured shape', async () => {
    const { transport } = setup()
    expect(await transport.request(APP_INFO_REQUEST)).toEqual({
      name: 'Agent Studio',
      version: '9.9.9',
      dataDir: '/data',
      platform: 'linux'
    })
  })

  it('rejects unknown requests and methods', async () => {
    const { transport } = setup()
    await expect(transport.request('nope')).rejects.toThrow(/unsupported/)
    await expect(transport.request({ method: 'doesNotExist', args: [] })).rejects.toThrow(/no handler/)
  })
})

describe('window.hydra stub (G0)', () => {
  it('returns the default config and merges updates', async () => {
    const { api } = setup()
    expect(await api.getConfig()).toEqual(DEFAULT_CONFIG)
    const next = await api.setConfig({ theme: 'light' })
    expect(next.theme).toBe('light')
    expect((await api.getConfig()).theme).toBe('light')
  })

  it('serves fixture agents in different states', async () => {
    const { api } = setup()
    const agents = await api.listAgents()
    expect(new Set(agents.map((a) => a.status)).size).toBeGreaterThanOrEqual(3)
    expect(await api.listClaudeSessions()).toEqual([])
  })

  it('keeps agent mutations in memory', async () => {
    const { api } = setup()
    const [first] = await api.listAgents()
    await api.renameAgent(first.id, 'renamed')
    expect((await api.listAgents())[0].name).toBe('renamed')
    expect(await api.removeAgent(first.id)).toBe(true)
    expect((await api.listAgents()).some((a) => a.id === first.id)).toBe(false)
  })

  it('reports empty-but-valid overview, usage and update states', async () => {
    const { api } = setup()
    const overview = await api.getOverview()
    expect(overview.counts.total).toBe(overview.sessions.length)
    const usage = await api.getUsageState()
    expect(usage.claude).toEqual([])
    expect((await api.getUsageSummary('today')).totalTokens).toBe(0)
    const update = await api.getUpdateState()
    expect(update.available).toBe(false)
    expect(update.currentVersion).toBe('9.9.9')
  })

  it('delivers events to subscribers until they unsubscribe', async () => {
    const { api, transport } = setup()
    const seen = vi.fn()
    const unsubscribe = api.onAgentStatus(seen)
    transport.emit('onAgentStatus', { agentId: 'a', status: 'idle' })
    expect(seen).toHaveBeenCalledWith({ agentId: 'a', status: 'idle' })
    unsubscribe()
    transport.emit('onAgentStatus', { agentId: 'a', status: 'running' })
    expect(seen).toHaveBeenCalledTimes(1)
  })

  it('pushes config changes to onConfigChange', async () => {
    const { api } = setup()
    const seen = vi.fn()
    api.onConfigChange(seen)
    await api.setConfig({ enableSoundEffects: false })
    expect(seen).toHaveBeenCalledTimes(1)
    expect(seen.mock.calls[0][0].enableSoundEffects).toBe(false)
  })

  it('has a handler for every method of the API', async () => {
    const { api } = setup()
    const calls: Promise<unknown>[] = []
    for (const [name, fn] of Object.entries(api)) {
      if (name.startsWith('on')) {
        expect(typeof (fn as (cb: () => void) => unknown)(() => undefined)).toBe('function')
        continue
      }
      // Missing handlers throw "no handler"; every other outcome (value or deliberate rejection) is fine.
      calls.push(
        Promise.resolve()
          .then(() => (fn as () => unknown)())
          .catch((err: Error) => {
            expect(err.message).not.toMatch(/no handler/)
          })
      )
    }
    await Promise.all(calls)
  })
})
