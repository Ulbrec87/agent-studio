import { createMockTransport } from './mockTransport'
import { createTauriTransport, isTauri } from './tauriTransport'

/**
 * The one seam between the renderer and the backend: a request/response call and an event stream, both carrying plain
 * JSON. The Tauri shell forwards them to the daemon (`daemon_request` command, `daemon-event` event, wired in G1); in a
 * plain browser, and in tests, an in-memory mock answers instead.
 */
export interface Transport {
  request(req: unknown): Promise<unknown>
  onEvent(cb: (e: unknown) => void): () => void
}

/** A request made by the `window.hydra` shim: the Hydra method name and its arguments. */
export interface HydraRequest {
  method: string
  args: unknown[]
}

/** An event delivered to the shim: the Hydra subscription name (`onAgentOutput`, ...) and the callback arguments. */
export interface HydraEvent {
  channel: string
  args: unknown[]
}

/** Answer of the `app_info` command (Rust `AppInfo`, camelCase). */
export interface AppInfo {
  name: string
  version: string
  dataDir: string
  platform: string
}

/** Special request answered by the Tauri shell without the daemon (G0) and by the mock in browsers. */
export const APP_INFO_REQUEST = { kind: 'app_info' } as const

/** Tauri transport inside the desktop app, mock everywhere else. */
export function selectTransport(): Transport {
  return isTauri() ? createTauriTransport() : createMockTransport()
}
