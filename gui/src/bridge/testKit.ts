// Shared by the bridge tests: a scripted raw transport (records requests, answers by request type) and small helpers.
import { bytesToBase64, type DaemonStatus, type ProtoEvent, type ProtoRequest, type ProtoResponse } from './protocol'
import type { RawTransport } from './transport'

export type Handler = (payload: unknown) => ProtoResponse | Promise<ProtoResponse>

export const AGENT = {
  createdAt: '2026-11-05T17:54:11.095Z', endedAt: null, id: 'ddbc7683', initialPrompt: '', isManager: false,
  lastActivityAt: '2026-11-05T17:54:11.095Z', model: 'sonnet', name: 'shape', pid: 14269, projectDir: '/tmp/asg1-proj',
  provider: 'claude', restartCount: 0, sessionId: null, startedAt: '2026-11-05T17:54:11.098Z', status: 'running',
  workMode: 'local', worktreeBranch: null, worktreePath: null, yolo: false
}
export const IDLE = { ...AGENT, id: 'idle0001', status: 'idle', pid: null }

export class FakeRaw implements RawTransport {
  requests: ProtoRequest[] = []
  handlers = new Map<string, Handler>()
  private events = new Set<(e: ProtoEvent) => void>()
  private statuses = new Set<(s: DaemonStatus) => void>()
  current: DaemonStatus = { connected: true }

  constructor() {
    this.handlers.set('list_agents', () => ({ type: 'agents', payload: [AGENT, IDLE] }))
    this.handlers.set('subscribe', () => ({ type: 'buffer', payload: { data: '' } }))
    this.handlers.set('client_focus', () => ({ type: 'ok' }))
  }
  async request(req: unknown): Promise<unknown> {
    const r = req as ProtoRequest
    this.requests.push(r)
    const handler = this.handlers.get(r.type)
    if (!handler) throw new Error(`no fake handler for ${r.type}`)
    return handler(r.payload)
  }
  onEvent(cb: (e: ProtoEvent) => void): () => void {
    this.events.add(cb)
    return () => this.events.delete(cb)
  }
  onStatus(cb: (s: DaemonStatus) => void): () => void {
    this.statuses.add(cb)
    return () => this.statuses.delete(cb)
  }
  async status(): Promise<DaemonStatus> {
    return this.current
  }
  push(type: string, payload?: unknown): void {
    for (const cb of [...this.events]) cb({ type, payload })
  }
  setStatus(s: DaemonStatus): void {
    this.current = s
    for (const cb of [...this.statuses]) cb(s)
  }
  of(type: string): ProtoRequest[] {
    return this.requests.filter((r) => r.type === type)
  }
}

export const tick = (ms = 0) => new Promise((r) => setTimeout(r, ms))
export const b64 = (text: string | number[]) => bytesToBase64(typeof text === 'string' ? new TextEncoder().encode(text) : Uint8Array.from(text))


import { createBridge } from './bridge'
import { createDaemonTransport, type DaemonTransport, type DaemonTransportOptions } from './daemonApi'
import { createHydraApi, type HydraAPI } from './hydraApi'
import { createMockTransport } from './mockTransport'

/** A bridge over a scripted raw transport, like a real one (`window.hydra` on top of the mapping layer). */
export async function makeBridge(
  configure?: (raw: FakeRaw) => void,
  options: Partial<DaemonTransportOptions> = {}
): Promise<{ raw: FakeRaw; api: HydraAPI; transport: DaemonTransport }> {
  const raw = new FakeRaw()
  configure?.(raw)
  const transport = createDaemonTransport({
    raw,
    fallback: createMockTransport(),
    appInfo: { name: 'Agent Studio', version: '1', dataDir: '/data', platform: 'linux' },
    storage: null,
    fsPollMs: 0,
    ...options
  })
  const api = createHydraApi(createBridge(transport))
  await tick() // let the initial status/resync settle
  raw.requests.length = 0
  return { raw, api, transport }
}
