// What the method groups (git, files, terminals, native) get from `createDaemonTransport`: typed protocol calls and events.
import type { AppInfo, ShellBridge } from './transport'
import type { ProtoResponse } from './protocol'

export interface BridgeContext {
  /** Send a protocol request and unwrap the payload of the expected response variant(s). */
  call<T>(type: string, payload: unknown, ...expected: string[]): Promise<T>
  /** Send a protocol request and return the raw response. */
  send(type: string, payload?: unknown): Promise<ProtoResponse>
  /** Deliver an `on*` event to the renderer. */
  emit(channel: string, ...args: unknown[]): void
  /** What the shell reported about itself (data dir, platform); undefined until loaded. */
  appInfo(): AppInfo | undefined
  /** The desktop shell's own commands and window events; undefined in a plain browser and in the tests that leave it out. */
  shell: ShellBridge | undefined
}

/** A group of Hydra methods: the method table, plus the hooks the transport calls on daemon events and connection changes. */
export interface MethodGroup {
  methods: Record<string, (...args: never[]) => unknown>
  /** Handle a daemon event; true when it was consumed. */
  onEvent?(type: string, payload: Record<string, unknown>): boolean
  /** The connection dropped: forget subscriptions. */
  onDisconnect?(): void
  /** The connection is (again) up: subscribe again and tell the renderer what changed. */
  onConnect?(): void
  dispose?(): void
}
