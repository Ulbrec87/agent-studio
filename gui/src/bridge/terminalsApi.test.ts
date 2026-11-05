// @vitest-environment jsdom
import { afterEach, describe, expect, it } from 'vitest'
import type { FreeTerminalLayout } from '@shared/types'
import { mapTerminalList } from './terminalsApi'
import { base64ToBytes } from './protocol'
import { makeBridge, tick, b64 } from './testKit'
import type { DaemonTransport } from './daemonApi'

const layout = (ids: string[], projectDir = '/proj'): FreeTerminalLayout & { groups: Array<{ tree?: unknown }> } => ({
  projectDir,
  groups: ids.length === 0 ? [] : [{ id: 'g1', paneIds: ids, activePaneId: ids[0], tree: { kind: 'row' } }],
  activeGroupId: ids.length === 0 ? null : 'g1',
  panes: ids.map((id, i) => ({ id, label: `Terminal ${i + 1}`, exited: false }))
})

let transport: DaemonTransport | null = null
afterEach(() => transport?.dispose())
async function setup(configure?: Parameters<typeof makeBridge>[0]) {
  const made = await makeBridge((r) => {
    r.handlers.set('subscribe_terminal', () => ({ type: 'buffer', payload: { data: b64('$ ') } }))
    r.handlers.set('terminal_input', () => ({ type: 'flag', payload: true }))
    r.handlers.set('terminal_resize', () => ({ type: 'ok' }))
    r.handlers.set('kill_terminal', () => ({ type: 'flag', payload: true }))
    configure?.(r)
  })
  transport = made.transport
  return made
}

describe('free terminals', () => {
  it('the layout passes through (split tree included) and every live pane is subscribed', async () => {
    const { raw, api } = await setup((r) => r.handlers.set('get_terminal_layout', () => ({ type: 'terminal_layout', payload: layout(['t1', 't2']) })))
    const got = await api.getFreeTerminalLayout('/proj')
    expect(got.groups[0].paneIds).toEqual(['t1', 't2'])
    expect((got.groups[0] as unknown as { tree: unknown }).tree).toEqual({ kind: 'row' })
    await tick()
    expect(raw.of('subscribe_terminal').map((r) => r.payload)).toEqual([{ terminal_id: 't1' }, { terminal_id: 't2' }])
  })

  it('spawn sends the project, cwd and group, then returns the daemon answer', async () => {
    const { raw, api } = await setup((r) =>
      r.handlers.set('spawn_terminal', () => ({ type: 'terminal_spawned', payload: { terminalId: 't9', groupId: 'g1', layout: layout(['t9']) } }))
    )
    const spawned = await api.spawnFreeTerminal('/proj', { cwd: '/proj/src', groupId: 'g1' })
    expect(spawned.terminalId).toBe('t9')
    expect(raw.of('spawn_terminal')[0].payload).toEqual({ project_dir: '/proj', cwd: '/proj/src', group_id: 'g1', cols: 80, rows: 24 })
    await api.spawnFreeTerminal('/proj')
    expect(raw.of('spawn_terminal')[1].payload).toEqual({ project_dir: '/proj', cols: 80, rows: 24 })
  })

  it('input is UTF-8 base64 and stays in order; resize, kill and activate map to the protocol', async () => {
    const order: string[] = []
    const { raw, api } = await setup((r) => {
      r.handlers.set('terminal_input', async (p) => {
        await tick(order.length === 0 ? 15 : 0)
        order.push(new TextDecoder().decode(base64ToBytes((p as { data: string }).data)))
        return { type: 'flag', payload: true }
      })
      r.handlers.set('activate_terminal', () => ({ type: 'terminal_layout', payload: layout(['t1']) }))
    })
    api.sendFreeTerminalInput('t1', 'é€')
    api.sendFreeTerminalInput('t1', 'x')
    api.resizeFreeTerminal('t1', 120, 40)
    api.killFreeTerminal('t1')
    api.activateFreeTerminal('/proj', 'g1', 't1')
    await tick(60)
    expect(order).toEqual(['é€', 'x'])
    // Per terminal the daemon sees input, resize and kill in the order they were issued (activate is independent).
    const types = raw.requests.map((r) => r.type)
    expect(types.filter((t) => ['terminal_input', 'terminal_resize', 'kill_terminal'].includes(t))).toEqual(['terminal_input', 'terminal_input', 'terminal_resize', 'kill_terminal'])
    expect(types).toContain('activate_terminal')
    expect(raw.of('terminal_resize')[0].payload).toEqual({ terminal_id: 't1', cols: 120, rows: 40 })
    expect(raw.of('activate_terminal')[0].payload).toEqual({ project_dir: '/proj', group_id: 'g1', pane_id: 't1' })
  })

  it('output events become onFreeTerminalOutput(id, projectDir, text) with a streaming decoder', async () => {
    const { raw, api } = await setup((r) => r.handlers.set('get_terminal_layout', () => ({ type: 'terminal_layout', payload: layout(['t1']) })))
    await api.getFreeTerminalLayout('/proj')
    await tick()
    const got: Array<[string, string, string]> = []
    api.onFreeTerminalOutput((id, dir, data) => got.push([id, dir, data]))
    raw.push('terminal_output', { terminal_id: 't1', data: b64([0x61, 0xe2, 0x82]) })
    raw.push('terminal_output', { terminal_id: 't1', data: b64([0xac, 0x62]) })
    expect(got.map((g) => g[2]).join('')).toBe('a€b')
    expect(got.every((g) => g[0] === 't1' && g[1] === '/proj')).toBe(true)
  })

  it('getFreeTerminalBuffer returns the scrollback, or exists:false for a terminal the daemon no longer has', async () => {
    const { api } = await setup((r) =>
      r.handlers.set('subscribe_terminal', (p) =>
        (p as { terminal_id: string }).terminal_id === 'gone' ? Promise.reject('terminal gone not found') : { type: 'buffer', payload: { data: b64('hi €\r\n') } }
      )
    )
    expect(await api.getFreeTerminalBuffer('t1')).toEqual({ exists: true, data: 'hi €\r\n' })
    expect(await api.getFreeTerminalBuffer('gone')).toEqual({ exists: false, data: '' })
  })

  it('exit and layout events reach the renderer; the exited terminal is forgotten', async () => {
    const { raw, api } = await setup()
    const exits: unknown[] = []
    const layouts: unknown[] = []
    api.onFreeTerminalExit((id, dir, code) => exits.push([id, dir, code]))
    api.onFreeTerminalLayoutChanged((dir, l) => layouts.push([dir, l.panes.map((p) => p.id)]))
    raw.push('terminal_layout_changed', { project_dir: '/proj', layout: layout(['t1', 't2']) })
    raw.push('terminal_exit', { terminal_id: 't2', project_dir: '/proj', exit_code: 130 })
    expect(layouts).toEqual([['/proj', ['t1', 't2']]])
    expect(exits).toEqual([['t2', '/proj', 130]])
  })

  it('lists terminals with an exited flag', async () => {
    const info = { terminalId: 't1', projectDir: '/proj', label: 'Terminal 1', lastActivityAt: 1, lastInputAt: 2 }
    expect(mapTerminalList([info])).toEqual([{ ...info, exited: false }])
    const { api } = await setup((r) => r.handlers.set('list_terminals', () => ({ type: 'terminals', payload: [info] })))
    expect(await api.listFreeTerminals()).toEqual([{ ...info, exited: false }])
  })

  it('after a reconnect the layouts of projects with terminals are fetched again (a restarted daemon respawns them) and announced', async () => {
    let call = 0
    const { raw, api } = await setup((r) =>
      r.handlers.set('get_terminal_layout', () => ({ type: 'terminal_layout', payload: layout(call++ === 0 ? ['t1'] : ['n1']) }))
    )
    await api.getFreeTerminalLayout('/proj')
    await tick()
    const layouts: Array<[string, string[]]> = []
    api.onFreeTerminalLayoutChanged((dir, l) => layouts.push([dir, l.panes.map((p) => p.id)]))
    raw.setStatus({ connected: false, error: 'lost' })
    raw.setStatus({ connected: true })
    await tick(10)
    expect(layouts).toEqual([['/proj', ['n1']]])
    // The new shell is subscribed on the new connection.
    expect(raw.of('subscribe_terminal').map((r) => (r.payload as { terminal_id: string }).terminal_id)).toEqual(['t1', 'n1'])
  })
})

describe('test terminal (preflight)', () => {
  it('runs in the data folder, routes output/exit to its own channels and is killed on close', async () => {
    const { raw, api } = await setup((r) => {
      r.handlers.set('spawn_terminal', () => ({ type: 'terminal_spawned', payload: { terminalId: 'tt', groupId: 'g', layout: layout(['tt'], '/data') } }))
      r.handlers.set('subscribe_terminal', () => ({ type: 'buffer', payload: { data: b64('$ ') } }))
    })
    const out: string[] = []
    const free: string[] = []
    const exits: number[] = []
    api.onTestTerminalOutput((d) => out.push(d))
    api.onFreeTerminalOutput((_id, _dir, d) => free.push(d))
    api.onTestTerminalExit((c) => exits.push(c))
    await api.spawnTestTerminal()
    expect(raw.of('spawn_terminal')[0].payload).toEqual({ project_dir: '/data', cols: 80, rows: 24 })
    raw.push('terminal_output', { terminal_id: 'tt', data: b64('claude --version\r\n') })
    expect(out).toEqual(['$ ', 'claude --version\r\n'])
    expect(free).toEqual([])
    api.sendTestTerminalInput('ls\r')
    api.resizeTestTerminal(100, 30)
    await tick(5)
    expect(raw.of('terminal_input')[0].payload).toMatchObject({ terminal_id: 'tt' })
    expect(raw.of('terminal_resize')[0].payload).toEqual({ terminal_id: 'tt', cols: 100, rows: 30 })
    // The scratch project's layout is nobody's business.
    const layouts: unknown[] = []
    api.onFreeTerminalLayoutChanged((d) => layouts.push(d))
    raw.push('terminal_layout_changed', { project_dir: '/data', layout: layout(['tt'], '/data') })
    expect(layouts).toEqual([])
    raw.push('terminal_exit', { terminal_id: 'tt', project_dir: '/data', exit_code: 0 })
    expect(exits).toEqual([0])
    api.sendTestTerminalInput('ignored')
    await tick(5)
    expect(raw.of('terminal_input')).toHaveLength(1)
  })

  it('killTestTerminal kills the shell', async () => {
    const { raw, api } = await setup((r) =>
      r.handlers.set('spawn_terminal', () => ({ type: 'terminal_spawned', payload: { terminalId: 'tt', groupId: 'g', layout: layout(['tt'], '/data') } }))
    )
    await api.spawnTestTerminal()
    api.killTestTerminal()
    await tick(5)
    expect(raw.of('kill_terminal')[0].payload).toEqual({ terminal_id: 'tt' })
  })
})
