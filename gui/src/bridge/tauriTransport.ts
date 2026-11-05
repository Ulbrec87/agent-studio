import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { toError, type DaemonStatus, type ProtoEvent } from './protocol'
import type { RawTransport, ShellBridge } from './transport'

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

/** The Tauri commands of the shell besides `daemon_request` (see `crates/as-gui/src/shell.rs`). */
export const SHELL_COMMANDS = ['log_event', 'open_in', 'list_open_targets', 'quit_app'] as const

function createShell(): ShellBridge {
  return {
    supports: (command) => (SHELL_COMMANDS as readonly string[]).includes(command),
    async invoke<T>(command: string, args?: Record<string, unknown>): Promise<T> {
      try {
        return await invoke<T>(command, args)
      } catch (err) {
        throw toError(err)
      }
    },
    on: (event, callback) => subscribe<unknown>(event, callback)
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
    shell: createShell(),
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
