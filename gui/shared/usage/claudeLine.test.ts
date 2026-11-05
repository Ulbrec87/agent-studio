import { describe, expect, it } from 'vitest'
import { MAX_LINE_BYTES, claudeProjectKeyLabel, parseClaudeUsageLine } from './claudeLine'

interface Over {
  type?: string
  timestamp?: unknown
  requestId?: unknown
  cwd?: unknown
  message?: Record<string, unknown> | null
  usage?: Record<string, unknown>
}

function line(o: Over = {}): string {
  const obj: Record<string, unknown> = {
    type: o.type ?? 'assistant',
    requestId: 'req_1',
    timestamp: '2026-10-22T17:45:01.123Z',
    cwd: '/home/u/proj',
    sessionId: 's1',
    message: {
      id: 'msg_1',
      model: 'claude-opus-5',
      role: 'assistant',
      content: [],
      usage: {
        input_tokens: 2,
        cache_creation_input_tokens: 76124,
        cache_read_input_tokens: 0,
        output_tokens: 1312,
        output_tokens_details: { thinking_tokens: 1043 },
        cache_creation: { ephemeral_1h_input_tokens: 76124, ephemeral_5m_input_tokens: 0 },
        ...o.usage
      }
    }
  }
  if ('timestamp' in o) obj.timestamp = o.timestamp
  if ('requestId' in o) obj.requestId = o.requestId
  if ('cwd' in o) obj.cwd = o.cwd
  if (o.message !== undefined) obj.message = o.message
  return JSON.stringify(obj)
}

describe('parseClaudeUsageLine', () => {
  it('parses the full sample with 1h cache split and no double-counted thinking', () => {
    const p = parseClaudeUsageLine(line())!
    expect(p.record).toEqual({
      ts: Date.parse('2026-10-22T17:45:01.123Z'),
      provider: 'claude',
      model: 'claude-opus-5',
      projectKey: '/home/u/proj',
      tokens: { input: 2, output: 1312, cacheWrite5m: 0, cacheWrite1h: 76124, cacheRead: 0 }
    })
    expect(p.dedupeKey).toBe('msg_1:req_1')
  })

  it('puts legacy cache_creation_input_tokens into the 5m bucket', () => {
    const p = parseClaudeUsageLine(
      line({ usage: { cache_creation: undefined, cache_creation_input_tokens: 500, cache_read_input_tokens: 7 } })
    )!
    expect(p.record.tokens).toMatchObject({ cacheWrite5m: 500, cacheWrite1h: 0, cacheRead: 7 })
  })

  it('uses a partial split when only one ephemeral field is numeric', () => {
    const p = parseClaudeUsageLine(line({ usage: { cache_creation: { ephemeral_5m_input_tokens: 10 } } }))!
    expect(p.record.tokens).toMatchObject({ cacheWrite5m: 10, cacheWrite1h: 0 })
  })

  it('skips synthetic models', () => {
    expect(parseClaudeUsageLine(line({ message: { id: 'm', model: '<synthetic>', usage: { input_tokens: 5 } } }))).toBeNull()
  })

  it('skips all-zero usage', () => {
    const zero = { input_tokens: 0, output_tokens: 0, cache_creation_input_tokens: 0, cache_read_input_tokens: 0, cache_creation: undefined }
    expect(parseClaudeUsageLine(line({ usage: zero }))).toBeNull()
  })

  it('skips non-assistant lines', () => {
    expect(parseClaudeUsageLine(line({ type: 'user' }))).toBeNull()
    expect(parseClaudeUsageLine(line({ type: 'system' }))).toBeNull()
    expect(parseClaudeUsageLine('{"type":"summary","summary":"an assistant usage"}')).toBeNull()
  })

  it('skips assistant lines without an object usage', () => {
    expect(parseClaudeUsageLine('{"type":"assistant","timestamp":"2026-10-22T17:45:01Z","message":{"usage":5}}')).toBeNull()
    expect(parseClaudeUsageLine('{"type":"assistant","usage":{}}')).toBeNull()
  })

  it('returns null for malformed or non-object JSON', () => {
    expect(parseClaudeUsageLine('{"type":"assistant","usage":')).toBeNull()
    expect(parseClaudeUsageLine('')).toBeNull()
    expect(parseClaudeUsageLine('"assistant" "usage"')).toBeNull()
    expect(parseClaudeUsageLine('["assistant","usage"]')).toBeNull()
  })

  it('returns null for a missing or invalid timestamp', () => {
    expect(parseClaudeUsageLine(line({ timestamp: undefined }))).toBeNull()
    expect(parseClaudeUsageLine(line({ timestamp: 'not a date' }))).toBeNull()
    expect(parseClaudeUsageLine(line({ timestamp: 12345 }))).toBeNull()
  })

  it('builds dedupe keys', () => {
    expect(parseClaudeUsageLine(line())!.dedupeKey).toBe('msg_1:req_1')
    expect(parseClaudeUsageLine(line({ requestId: undefined }))!.dedupeKey).toBe('msg_1')
    expect(parseClaudeUsageLine(line({ requestId: '' }))!.dedupeKey).toBe('msg_1')
    const noId = { model: 'claude-opus-5', usage: { input_tokens: 3 } }
    expect(parseClaudeUsageLine(line({ message: noId }))!.dedupeKey).toBeNull()
  })

  it('normalises the model, and falls back to unknown', () => {
    const m = (model?: string) => ({ id: 'm', ...(model ? { model } : {}), usage: { input_tokens: 1 } })
    expect(parseClaudeUsageLine(line({ message: m('claude-sonnet-4-5-20250929') }))!.record.model).toBe('claude-sonnet-4-5')
    expect(parseClaudeUsageLine(line({ message: m() }))!.record.model).toBe('unknown')
  })

  it('treats negative, NaN-ish and non-numeric values as 0 and floors floats', () => {
    const p = parseClaudeUsageLine(
      line({
        usage: {
          input_tokens: -5,
          output_tokens: 'lots',
          cache_read_input_tokens: 9.9,
          cache_creation: { ephemeral_5m_input_tokens: null, ephemeral_1h_input_tokens: 4 }
        }
      })
    )!
    expect(p.record.tokens).toEqual({ input: 0, output: 0, cacheWrite5m: 0, cacheWrite1h: 4, cacheRead: 9 })
  })

  it('defaults projectKey to an empty string', () => {
    expect(parseClaudeUsageLine(line({ cwd: undefined }))!.record.projectKey).toBe('')
  })

  it('pre-filters lines lacking "assistant" or "usage", and oversize lines', () => {
    expect(parseClaudeUsageLine('{"type":"user","message":{"content":"hi"}}')).toBeNull()
    expect(parseClaudeUsageLine('{"type":"assistant","message":{"content":"hi"}}')).toBeNull()
    const huge = line().replace('"cwd":"/home/u/proj"', `"cwd":"${'x'.repeat(MAX_LINE_BYTES)}"`)
    expect(parseClaudeUsageLine(huge)).toBeNull()
  })
})

describe('claudeProjectKeyLabel', () => {
  it('returns the last path segment', () => {
    expect(claudeProjectKeyLabel('/home/u/PhpstormProjects/hydra')).toBe('hydra')
    expect(claudeProjectKeyLabel('/home/u/hydra/')).toBe('hydra')
    expect(claudeProjectKeyLabel('C:\\Users\\u\\hydra')).toBe('hydra')
  })
  it('falls back to Unknown', () => {
    expect(claudeProjectKeyLabel('')).toBe('Unknown')
    expect(claudeProjectKeyLabel('/')).toBe('Unknown')
  })
})
