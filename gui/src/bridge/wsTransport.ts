import { toError, type DaemonStatus, type ProtoEvent } from './protocol'
import type { RawTransport, ShellBridge } from './transport'

/**
 * Browser dev mode: `?bridge=ws://127.0.0.1:PORT&token=T` selects this transport. The WebSocket is served by
 * `agent-studio --dev-bridge` (a dev/test tool, see `crates/as-gui/src/devbridge.rs`): one JSON text frame per message,
 * `{ id, request }` -> `{ id, result | error }`, pushes `{ event }` and `{ status }`.
 */
export function wsBridgeParams(search: string = typeof window !== 'undefined' ? window.location.search : ''): { url: string; token: string } | null {
  const params = new URLSearchParams(search)
  const url = params.get('bridge')
  const token = params.get('token')
  if (!url || !token || !/^wss?:\/\/(127\.0\.0\.1|localhost|\[::1\])(:\d+)?\/?$/.test(url)) return null
  return { url, token }
}

type Pending = { resolve: (v: unknown) => void; reject: (e: Error) => void }

export function createWsTransport(url: string, token: string, WS: typeof WebSocket = WebSocket): RawTransport {
  const base = url.replace(/\/$/, '')
  const eventListeners = new Set<(e: ProtoEvent) => void>()
  const statusListeners = new Set<(s: DaemonStatus) => void>()
  const pending = new Map<number, Pending>()
  let nextId = 1
  let current: WebSocket | null = null
  const waiters = new Set<(ws: WebSocket) => void>()
  let lastStatus: DaemonStatus = { connected: false, error: 'connecting to the dev bridge' }
  let retry = 500

  const setStatus = (s: DaemonStatus): void => {
    lastStatus = s
    for (const cb of [...statusListeners]) cb(s)
  }

  const connect = (): void => {
    const ws = new WS(`${base}/?token=${encodeURIComponent(token)}`)
    ws.onopen = () => {
      retry = 500
      current = ws
      for (const waiter of [...waiters]) waiter(ws)
      waiters.clear()
    }
    ws.onmessage = (message) => {
      let data: { id?: number; result?: unknown; error?: string; event?: ProtoEvent; status?: DaemonStatus }
      try {
        data = JSON.parse(String(message.data))
      } catch {
        return
      }
      if (typeof data.id === 'number') {
        const waiter = pending.get(data.id)
        if (!waiter) return
        pending.delete(data.id)
        if (data.error !== undefined) waiter.reject(new Error(data.error))
        else waiter.resolve(data.result)
      } else if (data.event) {
        for (const cb of [...eventListeners]) cb(data.event)
      } else if (data.status) {
        setStatus(data.status)
      }
    }
    ws.onclose = () => {
      if (current === ws) current = null
      for (const waiter of pending.values()) waiter.reject(new Error('dev bridge connection closed'))
      pending.clear()
      setStatus({ connected: false, error: 'dev bridge connection closed' })
      setTimeout(connect, retry)
      retry = Math.min(retry * 2, 5000)
    }
    ws.onerror = () => {
      // `close` follows and schedules the retry.
    }
  }
  connect()

  /** The open socket; waits (up to 10 s) for the first connection or a reconnect. */
  const ready = (): Promise<WebSocket> => {
    if (current) return Promise.resolve(current)
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        waiters.delete(onOpen)
        reject(new Error('dev bridge not reachable'))
      }, 10_000)
      const onOpen = (ws: WebSocket): void => {
        clearTimeout(timer)
        resolve(ws)
      }
      waiters.add(onOpen)
    })
  }

  const request = async (req: unknown): Promise<unknown> => {
    const ws = await ready()
    return new Promise((resolve, reject) => {
      const id = nextId++
      pending.set(id, { resolve, reject: (e) => reject(toError(e)) })
      try {
        ws.send(JSON.stringify({ id, request: req }))
      } catch (err) {
        pending.delete(id)
        reject(toError(err))
      }
    })
  }

  // The dev bridge has no window: the only shell command a browser may use is the renderer log.
  const shell: ShellBridge = {
    supports: (command) => command === 'log_event',
    invoke: (command, args) => request({ kind: 'shell', command, args }) as Promise<never>,
    on: () => () => undefined
  }

  return {
    shell,
    request,
    onEvent(cb) {
      eventListeners.add(cb)
      return () => eventListeners.delete(cb)
    },
    onStatus(cb) {
      statusListeners.add(cb)
      return () => statusListeners.delete(cb)
    },
    async status() {
      return lastStatus
    }
  }
}
