import { base64ToBytes, type ProtoRequest, type ProtoResponse } from './protocol'

/** A repeated chunk shorter than this is never taken for the daemon's re-delivered last chunk (a spinner frame is legitimately repeated). */
const MIN_DEDUPE_BYTES = 4

interface StreamDeps {
  send(req: ProtoRequest): Promise<ProtoResponse>
  /** Deliver decoded output to the renderer (`onAgentOutput`). */
  emit(agentId: string, text: string): void
  /** The request that subscribes an id (default: an agent's `subscribe`; free terminals use `subscribe_terminal`). */
  subscribeRequest?(id: string): ProtoRequest
}

/** Free terminals stream exactly like agents, only the subscribe request differs. */
export const terminalSubscribeRequest = (id: string): ProtoRequest => ({ type: 'subscribe_terminal', payload: { terminal_id: id } })

function endsWith(haystack: Uint8Array, needle: Uint8Array): boolean {
  if (needle.length === 0 || needle.length > haystack.length) return false
  const offset = haystack.length - needle.length
  for (let i = 0; i < needle.length; i++) if (haystack[offset + i] !== needle[i]) return false
  return true
}

/**
 * Per-agent (or per-free-terminal, see `subscribeRequest`) output streams. The daemon pushes `AgentOutput` bytes only to connections that `Subscribe`d, and PTY chunks can
 * end inside a UTF-8 sequence, so each agent has its own streaming `TextDecoder`.
 *
 * `Subscribe` answers with the buffered output; the first subscription of an agent holds live chunks that arrive before that
 * answer (events and responses travel on different paths) and drops the daemon's documented repeat of the last chunk
 * (see AGENTLOG "Subscribe must not duplicate or lose bytes").
 */
export function createAgentStreams(deps: StreamDeps) {
  const subscribed = new Set<string>()
  const inflight = new Map<string, Promise<void>>()
  const held = new Map<string, Uint8Array[]>()
  const decoders = new Map<string, TextDecoder>()

  const emitBytes = (agentId: string, bytes: Uint8Array): void => {
    let decoder = decoders.get(agentId)
    if (!decoder) {
      decoder = new TextDecoder('utf-8')
      decoders.set(agentId, decoder)
    }
    const text = decoder.decode(bytes, { stream: true })
    if (text) deps.emit(agentId, text)
  }

  async function subscribeOnce(agentId: string): Promise<Uint8Array> {
    const first = !subscribed.has(agentId)
    if (first) {
      subscribed.add(agentId)
      held.set(agentId, [])
    }
    let buffer: Uint8Array
    try {
      const response = await deps.send(deps.subscribeRequest?.(agentId) ?? { type: 'subscribe', payload: { agent_id: agentId } })
      buffer = base64ToBytes(String((response.payload as { data?: string } | undefined)?.data ?? ''))
    } catch (err) {
      if (first) {
        subscribed.delete(agentId)
        held.delete(agentId)
      }
      throw err
    }
    if (first) {
      const chunks = held.get(agentId) ?? []
      held.delete(agentId)
      if (chunks.length > 0 && chunks[0].length >= MIN_DEDUPE_BYTES && endsWith(buffer, chunks[0])) chunks.shift()
      for (const chunk of chunks) emitBytes(agentId, chunk)
    }
    return buffer
  }

  return {
    /** Live output event (base64 from the daemon). */
    handleOutput(agentId: string, dataBase64: string): void {
      const bytes = base64ToBytes(dataBase64)
      const queue = held.get(agentId)
      if (queue) queue.push(bytes)
      else emitBytes(agentId, bytes)
    },

    /** Start streaming an agent (idempotent; the buffer is not delivered here, `snapshot` serves it). */
    ensure(agentId: string): Promise<void> {
      if (subscribed.has(agentId)) return Promise.resolve()
      let promise = inflight.get(agentId)
      if (!promise) {
        promise = subscribeOnce(agentId)
          .then(() => undefined)
          .finally(() => inflight.delete(agentId))
        inflight.set(agentId, promise)
      }
      return promise
    },

    /** `getAgentBuffer`: the daemon's buffer as one decoded string (subscribes when needed). */
    async snapshot(agentId: string): Promise<string[]> {
      const bytes = await subscribeOnce(agentId)
      const text = new TextDecoder('utf-8').decode(bytes)
      return text ? [text] : []
    },

    /** The agent is gone: stop streaming it. Returns true when it was subscribed. */
    drop(agentId: string): boolean {
      decoders.delete(agentId)
      held.delete(agentId)
      return subscribed.delete(agentId)
    },

    /** A restarted process starts a new byte stream: forget a half-decoded character. */
    resetDecoder(agentId: string): void {
      decoders.delete(agentId)
    },

    has: (agentId: string): boolean => subscribed.has(agentId),
    ids: (): string[] => [...subscribed],

    /** The connection was lost: subscriptions belonged to it. */
    reset(): void {
      subscribed.clear()
      held.clear()
      inflight.clear()
      decoders.clear()
    }
  }
}

export type AgentStreams = ReturnType<typeof createAgentStreams>
