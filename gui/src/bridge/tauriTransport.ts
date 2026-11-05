import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { toError, type DaemonStatus, type ProtoEvent } from './protocol'
import type { RawTransport } from './transport'

/** True inside a Tauri webview (the runtime injects `__TAURI_INTERNALS__`). */
export function isTauri(): boolean {
  return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window
}

function subscribe<T>(name: string, cb: (payload: T) => void): () => void {
  let unlisten: (() => void) | null = null
  let cancelled = false
  void listen<T>(name, (event) => cb(event.payload)).then((fn) => {
    if (cancelled) fn()
    else unlisten = fn
  })
  return () => {
    cancelled = true
    unlisten?.()
  }
}

/**
 * Transport backed by the Rust shell (`crates/as-gui`).
 * - `{ kind: 'app_info' }` -> `app_info` command (no daemon needed).
 * - anything else -> `daemon_request` (JSON of `as_proto::Request` in, JSON of the `Response` out; errors reject with the daemon's message).
 * - `daemon-event` carries `as_proto::Event` JSON, `daemon-status` carries `{ connected, error? }`, `daemon_status` answers the initial state.
 */
export function createTauriTransport(): RawTransport {
  return {
    async request(req) {
      try {
        if (typeof req === 'object' && req !== null && (req as { kind?: unknown }).kind === 'app_info') {
          return await invoke('app_info')
        }
        return await invoke('daemon_request', { request: req })
      } catch (err) {
        throw toError(err)
      }
    },
    onEvent: (cb) => subscribe<ProtoEvent>('daemon-event', cb),
    onStatus: (cb) => subscribe<DaemonStatus>('daemon-status', cb),
    async status() {
      try {
        return await invoke<DaemonStatus>('daemon_status')
      } catch (err) {
        return { connected: false, error: toError(err).message }
      }
    }
  }
}
