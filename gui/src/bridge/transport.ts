import { createMockTransport } from './mockTransport'
import { createTauriTransport, isTauri } from './tauriTransport'
import { createWsTransport, wsBridgeParams } from './wsTransport'
import { createDaemonTransport } from './daemonApi'
import type { DaemonStatus, ProtoEvent } from './protocol'

/**
 * The one seam between the renderer and the backend: a request/response call and an event stream, both carrying plain
 * JSON. Two levels use this shape:
 * - a **raw** transport ([`RawTransport`]) speaks the daemon protocol (Tauri commands, or the dev bridge WebSocket);
 * - the **Hydra-level** transport (`createDaemonTransport`, or the in-memory mock) speaks `{ method, args }` /
 *   `{ channel, args }` and is what `createBridge` consumes.
 */
export interface Transport {
  request(req: unknown): Promise<unknown>
  onEvent(cb: (e: unknown) => void): () => void
}

/** A transport that speaks the daemon protocol: requests are `as-proto` `Request` JSON, events are `Event` JSON. */
export interface RawTransport extends Transport {
  onEvent(cb: (e: ProtoEvent) => void): () => void
  /** Connection state changes (`connected: true` again after a reconnect means: re-subscribe). */
  onStatus(cb: (s: DaemonStatus) => void): () => void
  /** The current state (the shell may already be connected when the page loads). */
  status(): Promise<DaemonStatus>
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

/** Special request answered by the shell without the daemon (and by the mock in browsers). */
export const APP_INFO_REQUEST = { kind: 'app_info' } as const

/** The raw transport of this environment: Tauri in the desktop app, the dev-bridge WebSocket when `?bridge=` is present, else none. */
export function selectRawTransport(): RawTransport | null {
  if (isTauri()) return createTauriTransport()
  const params = wsBridgeParams()
  if (params) return createWsTransport(params.url, params.token)
  return null
}

/** The Hydra-level transport for a given raw one (or the mock when there is none, e.g. a plain browser or tests). */
export function selectTransport(raw: RawTransport | null = selectRawTransport(), info?: AppInfo): Transport {
  const mock = createMockTransport(info ? { appInfo: info } : {})
  return raw ? createDaemonTransport({ raw, fallback: mock, appInfo: info }) : mock
}
