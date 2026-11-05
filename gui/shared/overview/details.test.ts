import { describe, expect, it } from 'vitest'
import { detailsFromTail, lastSaidInTail, lastUsageInTail, MAX_LAST_MESSAGE_LENGTH } from './details'

const j = (o: unknown) => JSON.stringify(o)

function assistant(id: string, content: unknown[], extra: Record<string, unknown> = {}) {
  return j({ type: 'assistant', message: { id, content, ...extra }, ...extra.line as object })
}

describe('lastSaidInTail', () => {
  it('joins the text blocks of the newest reply and ignores thinking and tools', () => {
    const tail = [
      j({ type: 'user', message: { content: 'hi' } }),
      assistant('m1', [{ type: 'text', text: 'old' }]),
      j({ type: 'user', message: { content: 'again' } }),
      assistant('m2', [{ type: 'thinking', thinking: 'x' }, { type: 'text', text: 'first\nline' }]),
      assistant('m2', [{ type: 'tool_use', id: 't', name: 'Bash', input: {} }]),
      assistant('m2', [{ type: 'text', text: 'second' }])
    ].join('\n')
    expect(lastSaidInTail(tail, true)).toEqual({ text: 'first\nline\n\nsecond' })
  })

  it('skips subagent and meta lines', () => {
    const tail = [
      assistant('m1', [{ type: 'text', text: 'mine' }]),
      j({ type: 'assistant', isSidechain: true, message: { id: 'm9', content: [{ type: 'text', text: 'sub' }] } })
    ].join('\n')
    expect(lastSaidInTail(tail, true)).toEqual({ text: 'mine' })
  })

  it('explains why there is no text', () => {
    expect(lastSaidInTail(j({ type: 'user', message: { content: 'x' } }), true)).toEqual({
      text: null,
      reason: 'nothing-yet'
    })
    expect(lastSaidInTail('{"partial":\n' + j({ type: 'system' }), false)).toEqual({
      text: null,
      reason: 'too-far-back'
    })
  })

  it('marks a message that may start before the tail', () => {
    const tail = 'cut off\n' + assistant('m1', [{ type: 'text', text: 'hello' }])
    expect(lastSaidInTail(tail, false)).toEqual({ text: 'hello', startCut: true })
  })

  it('skips lines that do not parse', () => {
    const tail = 'not json\n' + assistant('m1', [{ type: 'text', text: 'ok' }]) + '\n{broken'
    expect(lastSaidInTail(tail, true)).toEqual({ text: 'ok' })
  })
})

describe('lastUsageInTail', () => {
  const usage = (extra: Record<string, unknown>) => ({
    input_tokens: 10,
    output_tokens: 7,
    cache_creation_input_tokens: 20,
    cache_read_input_tokens: 30,
    ...extra
  })

  it('sums the prompt and reports the cached part', () => {
    const tail = assistant('m1', [{ type: 'text', text: 'a' }], { usage: usage({}) })
    expect(lastUsageInTail(tail, true)).toEqual({ input: 60, cached: 30, output: 7 })
  })

  it('leaves cached out when the cache read is not reported', () => {
    const tail = assistant('m1', [], { usage: { input_tokens: 5, output_tokens: 2 } })
    expect(lastUsageInTail(tail, true)).toEqual({ input: 5, output: 2 })
  })

  it('takes the newest own reply and reads past synthetic ones', () => {
    const tail = [
      assistant('m1', [], { usage: usage({ output_tokens: 1 }) }),
      assistant('m2', [], { usage: usage({ output_tokens: 99 }), model: '<synthetic>' })
    ].join('\n')
    expect(lastUsageInTail(tail, true)?.output).toBe(1)
  })

  it('has no counts after a compaction, or when a count is not a whole number', () => {
    const compact = j({ type: 'user', isCompactSummary: true, message: { content: 's' } })
    expect(lastUsageInTail([assistant('m1', [], { usage: usage({}) }), compact].join('\n'), true)).toBeNull()
    expect(lastUsageInTail(assistant('m1', [], { usage: usage({ output_tokens: 1.5 }) }), true)).toBeNull()
  })
})

describe('detailsFromTail', () => {
  it('keeps the last 2000 characters and flags truncation', () => {
    const text = 'a'.repeat(50) + 'b'.repeat(MAX_LAST_MESSAGE_LENGTH)
    const details = detailsFromTail(assistant('m1', [{ type: 'text', text }]), true)
    if (!('text' in details.lastMessage)) throw new Error('expected text')
    expect(details.lastMessage.truncated).toBe(true)
    expect(details.lastMessage.text).toBe('b'.repeat(MAX_LAST_MESSAGE_LENGTH))
  })

  it('reports unavailable when nothing was said', () => {
    expect(detailsFromTail('', true)).toEqual({ lastMessage: { unavailable: 'Nothing said yet' }, tokens: null })
  })
})
