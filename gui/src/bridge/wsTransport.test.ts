// @vitest-environment jsdom
import { describe, expect, it, vi } from 'vitest'
import { createWsTransport, wsBridgeParams } from './wsTransport'

class FakeSocket {
  static last: FakeSocket
  sent: string[] = []
  onopen: (() => void) | null = null
  onmessage: ((m: { data: string }) => void) | null = null
  onclose: (() => void) | null = null
  onerror: (() => void) | null = null
  constructor(public url: string) {
    FakeSocket.last = this
  }
  send(data: string): void {
    this.sent.push(data)
  }
  open(): void {
    this.onopen?.()
  }
  receive(value: unknown): void {
    this.onmessage?.({ data: JSON.stringify(value) })
  }
}

const WS = FakeSocket as unknown as typeof WebSocket

describe('wsBridgeParams', () => {
  it('needs both parameters and a loopback ws url', () => {
    expect(wsBridgeParams('?bridge=ws://127.0.0.1:4000&token=abc')).toEqual({ url: 'ws://127.0.0.1:4000', token: 'abc' })
    expect(wsBridgeParams('?bridge=ws://localhost:4000')).toBeNull()
    expect(wsBridgeParams('?token=abc')).toBeNull()
    expect(wsBridgeParams('?bridge=ws://evil.example.com:4000&token=abc')).toBeNull()
    expect(wsBridgeParams('')).toBeNull()
  })
})

describe('createWsTransport', () => {
  it('connects with the token, correlates responses by id and maps error frames to Errors', async () => {
    const transport = createWsTransport('ws://127.0.0.1:4000/', 'a b', WS)
    const socket = FakeSocket.last
    expect(socket.url).toBe('ws://127.0.0.1:4000/?token=a%20b')
    const first = transport.request({ type: 'ping' })
    const second = transport.request({ type: 'nope' })
    socket.open()
    await vi.waitFor(() => expect(socket.sent).toHaveLength(2))
    expect(JSON.parse(socket.sent[0])).toEqual({ id: 1, request: { type: 'ping' } })
    socket.receive({ id: 2, error: 'invalid request: x' })
    socket.receive({ id: 1, result: { type: 'pong' } })
    expect(await first).toEqual({ type: 'pong' })
    await expect(second).rejects.toThrow(new Error('invalid request: x'))
  })

  it('delivers events and status pushes and remembers the last status', async () => {
    const transport = createWsTransport('ws://127.0.0.1:4000', 't', WS)
    const socket = FakeSocket.last
    const events: unknown[] = []
    const statuses: unknown[] = []
    transport.onEvent((e) => events.push(e))
    transport.onStatus((s) => statuses.push(s))
    socket.open()
    socket.receive({ status: { connected: true } })
    socket.receive({ event: { type: 'agents_changed' } })
    expect(events).toEqual([{ type: 'agents_changed' }])
    expect(statuses).toEqual([{ connected: true }])
    expect(await transport.status()).toEqual({ connected: true })
  })

  it('fails pending requests and reports a disconnect when the socket closes', async () => {
    vi.useFakeTimers()
    try {
      const transport = createWsTransport('ws://127.0.0.1:4000', 't', WS)
      const socket = FakeSocket.last
      socket.open()
      const pending = transport.request({ type: 'ping' })
      await vi.advanceTimersByTimeAsync(0) // the request is registered once the socket is known to be open
      const settled = expect(pending).rejects.toThrow(/closed/)
      socket.onclose?.()
      await settled
      expect(await transport.status()).toEqual({ connected: false, error: 'dev bridge connection closed' })
      await vi.advanceTimersByTimeAsync(600)
      expect(FakeSocket.last).not.toBe(socket) // reconnecting
    } finally {
      vi.useRealTimers()
    }
  })
})
