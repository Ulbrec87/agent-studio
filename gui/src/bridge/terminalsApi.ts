// Free terminals (and the preflight "test terminal" built on one): Hydra's `free-terminal:*` channels on top of the daemon's
// `SpawnTerminal` / `TerminalInput` / `SubscribeTerminal` / `GetTerminalLayout` requests and `Terminal*` events.
//
// The daemon's layout is Hydra's layout plus, per group, an optional split `tree` (the renderer ignores unknown fields and
// shows `paneIds`, which the daemon keeps in reading order), so layouts pass through unchanged and keep the split info.
import type { FreeTerminalLayout } from '@shared/types'
import type { BridgeContext, MethodGroup } from './context'
import { bytesToBase64, utf8Bytes, type ProtoRequest, type ProtoResponse } from './protocol'
import { compact } from './mappers'
import type { TerminalInfo } from './generated'
import { createAgentStreams, terminalSubscribeRequest } from './streams'

/** Size a shell starts with; the renderer sends its real size as soon as the pane is mounted. */
const INITIAL_COLS = 80
const INITIAL_ROWS = 24

/** `listFreeTerminals` rows: the daemon drops exited terminals from its list, Hydra has an `exited` flag. */
export function mapTerminalList(list: TerminalInfo[]): Array<TerminalInfo & { exited: boolean }> {
  return list.map((t) => ({ ...t, exited: false }))
}

export function createTerminalMethods(ctx: BridgeContext): MethodGroup & { testTerminalId(): string | null } {
  const streams = createAgentStreams({
    send: (req: ProtoRequest) => ctx.send(req.type, req.payload) as Promise<ProtoResponse>,
    emit: (terminalId, data) => emitOutput(terminalId, data),
    subscribeRequest: terminalSubscribeRequest
  })
  /** Terminal id -> project (the daemon's output event does not say). */
  const projectOf = new Map<string, string>()
  /** Project -> its terminal ids, and whether it had any (those are re-requested after a reconnect, which respawns them). */
  const idsByProject = new Map<string, Set<string>>()
  let testId: string | null = null

  /** While the test terminal's first buffer is being fetched, its live output waits behind it (the buffer comes first). */
  let testQueue: string[] | null = null

  function emitOutput(terminalId: string, data: string): void {
    if (terminalId === testId) {
      if (testQueue) testQueue.push(data)
      else ctx.emit('onTestTerminalOutput', data)
    }
    else ctx.emit('onFreeTerminalOutput', terminalId, projectOf.get(terminalId) ?? '', data)
  }

  /** The test terminal lives in the data folder as a scratch project; its layout is nobody's business. */
  const isScratch = (projectDir: string): boolean => projectDir !== '' && projectDir === ctx.appInfo()?.dataDir

  /** Remember a layout (ids per project) and stream every live pane of it. */
  function observe(layout: FreeTerminalLayout): void {
    if (isScratch(layout.projectDir)) return
    const ids = new Set(layout.panes.map((p) => p.id))
    for (const old of idsByProject.get(layout.projectDir) ?? []) {
      if (!ids.has(old)) {
        projectOf.delete(old)
        streams.drop(old)
      }
    }
    idsByProject.set(layout.projectDir, ids)
    for (const pane of layout.panes) {
      projectOf.set(pane.id, layout.projectDir)
      if (!pane.exited && !streams.has(pane.id)) void streams.ensure(pane.id).catch((err) => console.warn(`[bridge] subscribe terminal ${pane.id} failed`, err))
    }
  }

  const seen = async (layout: Promise<FreeTerminalLayout>): Promise<FreeTerminalLayout> => {
    const value = await layout
    observe(value)
    return value
  }

  /** Keystrokes and resizes of one terminal must arrive in order (concurrent invokes carry no ordering guarantee). */
  const queues = new Map<string, Promise<unknown>>()
  const serial = (id: string, job: () => Promise<unknown>): Promise<unknown> => {
    const run = (queues.get(id) ?? Promise.resolve()).catch(() => undefined).then(job)
    queues.set(id, run)
    const clear = (): void => {
      if (queues.get(id) === run) queues.delete(id)
    }
    void run.then(clear, clear)
    return run
  }

  const spawn = async (projectDir: string, options?: { cwd?: string; groupId?: string }) => {
    const spawned = await ctx.call<{ terminalId: string; groupId: string; layout: FreeTerminalLayout }>(
      'spawn_terminal',
      compact({ project_dir: projectDir, cwd: options?.cwd, group_id: options?.groupId, cols: INITIAL_COLS, rows: INITIAL_ROWS }),
      'terminal_spawned'
    )
    observe(spawned.layout)
    return spawned
  }

  const input = (id: string, data: string) =>
    serial(id, () => ctx.send('terminal_input', { terminal_id: id, data: bytesToBase64(utf8Bytes(data)) }))
  const resize = (id: string, cols: number, rows: number) => serial(id, () => ctx.send('terminal_resize', { terminal_id: id, cols, rows }))
  const kill = (id: string) => serial(id, () => ctx.send('kill_terminal', { terminal_id: id }))

  const methods: MethodGroup['methods'] = {
    spawnFreeTerminal: spawn as never,
    sendFreeTerminalInput: input as never,
    resizeFreeTerminal: resize as never,
    killFreeTerminal: kill as never,
    activateFreeTerminal: (async (projectDir: string, groupId: string, paneId?: string) => {
      await seen(ctx.call('activate_terminal', compact({ project_dir: projectDir, group_id: groupId, pane_id: paneId }), 'terminal_layout'))
    }) as never,
    getFreeTerminalBuffer: (async (id: string) => {
      try {
        const chunks = await streams.snapshot(id)
        return { exists: true, data: chunks.join('') }
      } catch {
        return { exists: false, data: '' }
      }
    }) as never,
    getFreeTerminalLayout: ((projectDir: string) =>
      seen(ctx.call<FreeTerminalLayout>('get_terminal_layout', { project_dir: projectDir }, 'terminal_layout'))) as never,
    listFreeTerminals: (async () => mapTerminalList(await ctx.call<TerminalInfo[]>('list_terminals', undefined, 'terminals'))) as never,

    // The preflight "test terminal" (a shell to try `claude` in): a free terminal of a scratch project (the data folder) with its
    // own output/exit channels. It is killed when the modal closes, so it never stays in a layout.
    spawnTestTerminal: (async () => {
      const dir = ctx.appInfo()?.dataDir
      if (!dir) throw new Error('The data folder is unknown, cannot start a test terminal')
      if (testId) await kill(testId).catch(() => undefined)
      testId = null
      const spawned = await ctx.call<{ terminalId: string }>('spawn_terminal', { project_dir: dir, cols: INITIAL_COLS, rows: INITIAL_ROWS }, 'terminal_spawned')
      testId = spawned.terminalId
      testQueue = []
      try {
        // The shell may have printed its prompt already: the daemon's buffer first, then what arrived meanwhile.
        const first = (await streams.snapshot(spawned.terminalId)).join('')
        if (first) ctx.emit('onTestTerminalOutput', first)
      } finally {
        const queued = testQueue ?? []
        testQueue = null
        for (const chunk of queued) ctx.emit('onTestTerminalOutput', chunk)
      }
    }) as never,
    sendTestTerminalInput: ((data: string) => {
      if (testId) void input(testId, data)
    }) as never,
    resizeTestTerminal: ((cols: number, rows: number) => {
      if (testId) void resize(testId, cols, rows)
    }) as never,
    killTestTerminal: (() => {
      const id = testId
      testId = null
      if (id) void kill(id)
    }) as never
  }

  return {
    methods,
    testTerminalId: () => testId,
    onEvent(type, p) {
      switch (type) {
        case 'terminal_output':
          streams.handleOutput(String(p.terminal_id), String(p.data ?? ''))
          return true
        case 'terminal_exit': {
          const id = String(p.terminal_id)
          const projectDir = String(p.project_dir ?? projectOf.get(id) ?? '')
          const exitCode = Number(p.exit_code ?? 0)
          if (id === testId) {
            testId = null
            ctx.emit('onTestTerminalExit', exitCode)
          } else {
            ctx.emit('onFreeTerminalExit', id, projectDir, exitCode)
          }
          streams.drop(id)
          projectOf.delete(id)
          idsByProject.get(projectDir)?.delete(id)
          return true
        }
        case 'terminal_layout_changed': {
          const layout = p.layout as FreeTerminalLayout
          if (isScratch(layout.projectDir)) return true
          observe(layout)
          ctx.emit('onFreeTerminalLayoutChanged', String(p.project_dir ?? layout.projectDir), layout)
          return true
        }
        default:
          return false
      }
    },
    onDisconnect() {
      // The connection owned the subscriptions; the layout memory stays, to be asked for again on reconnect.
      streams.reset()
      queues.clear()
    },
    onConnect() {
      // A restarted daemon respawns a project's saved shells on the first `GetTerminalLayout`, with new ids: ask for every
      // project that had terminals and tell the renderer, which only reads layouts when a panel mounts.
      for (const [projectDir, ids] of idsByProject) {
        if (ids.size === 0) continue
        void ctx
          .call<FreeTerminalLayout>('get_terminal_layout', { project_dir: projectDir }, 'terminal_layout')
          .then((layout) => {
            observe(layout)
            ctx.emit('onFreeTerminalLayoutChanged', projectDir, layout)
          })
          .catch((err) => console.warn('[bridge] terminal layout refresh failed', err))
      }
    }
  }
}

export const TERMINAL_METHODS = [
  'spawnFreeTerminal', 'sendFreeTerminalInput', 'resizeFreeTerminal', 'killFreeTerminal', 'activateFreeTerminal',
  'getFreeTerminalBuffer', 'getFreeTerminalLayout', 'listFreeTerminals', 'spawnTestTerminal', 'sendTestTerminalInput',
  'resizeTestTerminal', 'killTestTerminal'
] as const
