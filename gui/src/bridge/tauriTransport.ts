import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import type { Transport } from './transport'

/** True inside a Tauri webview (the runtime injects `__TAURI_INTERNALS__`). */
export function isTauri(): boolean {
  return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window
}

/**
 * Transport backed by the Rust shell (`crates/as-gui`).
 * - `{ kind: 'app_info' }` -> `app_info` command (no daemon needed).
 * - anything else -> `daemon_request` (G0: the shell answers "not wired (G1)").
 * Events arrive on the `daemon-event` channel (nothing is emitted before G1).
 */
export function createTauriTransport(): Transport {
  return {
    request(req) {
      if (typeof req === 'object' && req !== null && (req as { kind?: unknown }).kind === 'app_info') {
        return invoke('app_info')
      }
      return invoke('daemon_request', { request: req })
    },
    onEvent(cb) {
      let unlisten: (() => void) | null = null
      let cancelled = false
      void listen('daemon-event', (event) => cb(event.payload)).then((fn) => {
        if (cancelled) fn()
        else unlisten = fn
      })
      return () => {
        cancelled = true
        unlisten?.()
      }
    }
  }
}
