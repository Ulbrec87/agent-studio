import type { Transport, HydraRequest, HydraEvent } from './transport'

type Listener = (...args: never[]) => void

/**
 * What `HydraAPI` is built on: request/response calls, fire-and-forget sends (Electron `ipcRenderer.send`) and event
 * subscriptions (`ipcRenderer.on`), all multiplexed over one `Transport`.
 */
export interface Bridge {
  call<T>(method: string, ...args: unknown[]): Promise<T>
  send(method: string, ...args: unknown[]): void
  subscribe<A extends unknown[]>(channel: string, callback: (...args: A) => void): () => void
}

export function createBridge(transport: Transport): Bridge {
  const listeners = new Map<string, Set<Listener>>()

  // One transport subscription for the whole bridge, fanned out by channel.
  transport.onEvent((raw) => {
    const event = raw as Partial<HydraEvent> | null
    if (!event || typeof event.channel !== 'string' || !Array.isArray(event.args)) return
    for (const listener of [...(listeners.get(event.channel) ?? [])]) {
      try {
        ;(listener as (...a: unknown[]) => void)(...event.args)
      } catch (err) {
        console.error(`[bridge] listener for ${event.channel} threw`, err)
      }
    }
  })

  return {
    call<T>(method: string, ...args: unknown[]): Promise<T> {
      const req: HydraRequest = { method, args }
      return transport.request(req) as Promise<T>
    },
    send(method, ...args) {
      const req: HydraRequest = { method, args }
      transport.request(req).catch((err) => console.warn(`[bridge] ${method} failed`, err))
    },
    subscribe(channel, callback) {
      let set = listeners.get(channel)
      if (!set) {
        set = new Set()
        listeners.set(channel, set)
      }
      const listener = callback as unknown as Listener
      set.add(listener)
      return () => {
        set?.delete(listener)
      }
    }
  }
}
