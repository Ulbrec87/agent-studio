import { describe, expect, it } from 'vitest'
import {
  CODEX_DEFAULT_MODEL,
  flushCodexCompactions,
  initialCodexFileState,
  parseCodexLine,
  type CodexFileState,
  type CodexLineResult,
  type CodexParsedUsage
} from './codexLine'

const BASE = Date.parse('2026-05-19T12:00:00.000Z')
const stamp = (s: number): string => new Date(BASE + s * 1000).toISOString()
const row = (s: number, type: string, payload: Record<string, unknown>): string =>
  JSON.stringify({ timestamp: stamp(s), type, payload })
const count = (s: number, last: unknown, total: unknown, extra: Record<string, unknown> = {}): string =>
  row(s, 'event_msg', { type: 'token_count', info: { last_token_usage: last, total_token_usage: total }, ...extra })

const U = (input: number, cached: number, output: number, reasoning = 0): Record<string, number> => ({
  input_tokens: input,
  cached_input_tokens: cached,
  output_tokens: output,
  reasoning_output_tokens: reasoning,
  total_tokens: input + output
})

interface Run {
  state: CodexFileState
  results: CodexLineResult[]
  usage: CodexParsedUsage[]
}

function feed(lines: string[], state = initialCodexFileState()): Run {
  const results: CodexLineResult[] = []
  const usage: CodexParsedUsage[] = []
  for (const l of lines) {
    const r = parseCodexLine(l, state)
    if (r) {
      results.push(r)
      usage.push(...(r.usage ?? []))
    }
  }
  return { state, results, usage }
}

const meta = (id: string, extra: Record<string, unknown> = {}): string =>
  row(0, 'session_meta', { id, cwd: '/w/proj', ...extra })
const ctx = (s: number, model: string, turn = 't1'): string => row(s, 'turn_context', { turn_id: turn, model })

describe('usage', () => {
  it('does not double-count cached input and does not add reasoning', () => {
    const { usage } = feed([meta('s1'), ctx(1, 'gpt-5-codex'), count(2, U(1000, 400, 50, 10), U(1000, 400, 50, 10))])
    expect(usage).toHaveLength(1)
    expect(usage[0].record).toEqual({
      ts: BASE + 2000,
      provider: 'codex',
      model: 'gpt-5-codex',
      projectKey: '/w/proj',
      tokens: { input: 600, output: 50, cacheWrite5m: 0, cacheWrite1h: 0, cacheRead: 400 }
    })
  })

  it('uses the latest turn_context model, and the default before any', () => {
    const { usage, state } = feed([
      meta('s1'),
      count(1, U(10, 0, 1), U(10, 0, 1)),
      ctx(2, 'GPT-5-Codex', 'a'),
      count(3, U(20, 0, 2), U(30, 0, 3)),
      ctx(4, 'gpt-5-mini', 'b'),
      count(5, U(30, 0, 3), U(60, 0, 6))
    ])
    expect(usage.map((u) => u.record.model)).toEqual([CODEX_DEFAULT_MODEL, 'gpt-5-codex', 'gpt-5-mini'])
    expect(state.turnModels).toEqual({ a: 'GPT-5-Codex', b: 'gpt-5-mini' })
  })

  it('skips absent, empty and all-zero last_token_usage', () => {
    const { usage } = feed([
      meta('s1'),
      row(1, 'event_msg', { type: 'token_count', info: null }),
      count(2, {}, U(5, 0, 1)),
      count(3, U(0, 0, 0), U(5, 0, 1)),
      row(4, 'event_msg', { type: 'agent_message' })
    ])
    expect(usage).toEqual([])
  })

  it('gives replayed events in a resumed file the same dedupeKey', () => {
    const original = feed([meta('s1'), ctx(1, 'm'), count(2, U(100, 10, 5), U(100, 10, 5)), count(3, U(50, 0, 5), U(150, 10, 10))])
    const resumed = feed([
      meta('s1'),
      ctx(100, 'm'),
      count(101, U(100, 10, 5), U(100, 10, 5)),
      count(102, U(50, 0, 5), U(150, 10, 10)),
      count(103, U(70, 0, 5), U(220, 10, 15))
    ])
    expect(original.usage[0].dedupeKey).toMatch(/^codex-token-v1:s1\|/)
    expect(resumed.usage[0].dedupeKey).toBe(original.usage[0].dedupeKey)
    expect(resumed.usage[1].dedupeKey).toBe(original.usage[1].dedupeKey)
    expect(resumed.usage[2].dedupeKey).not.toBe(original.usage[1].dedupeKey)
    expect(resumed.usage[0].record.ts).not.toBe(original.usage[0].record.ts)
  })

  it('treats missing and zero counters alike in the key', () => {
    const a = feed([meta('s1'), count(1, { input_tokens: 5, output_tokens: 1 }, { input_tokens: 5, output_tokens: 1 })])
    const b = feed([meta('s1'), count(1, U(5, 0, 1, 0), U(5, 0, 1, 0))])
    expect(a.usage[0].dedupeKey).toBe(b.usage[0].dedupeKey)
  })

  it('returns a null key when total, last or the session id is missing', () => {
    const noTotal = feed([meta('s1'), row(1, 'event_msg', { type: 'token_count', info: { last_token_usage: U(5, 0, 1) } })])
    expect(noTotal.usage).toHaveLength(1)
    expect(noTotal.usage[0].dedupeKey).toBeNull()
    const noSession = feed([count(1, U(5, 0, 1), U(5, 0, 1))])
    expect(noSession.usage[0].dedupeKey).toBeNull()
    expect(noSession.usage[0].record.projectKey).toBe('')
  })

  it('drops keyless events replayed from a parent in a fork file', () => {
    const fork = feed([
      meta('child', { forked_from_id: 'parent' }),
      meta('parent'),
      row(1, 'event_msg', { type: 'token_count', info: { last_token_usage: U(5, 0, 1) } }),
      count(2, U(7, 0, 1), U(7, 0, 1))
    ])
    // Replayed parent history: the keyless row is dropped, the keyed one is scoped to the parent.
    expect(fork.usage).toHaveLength(1)
    expect(fork.usage[0].dedupeKey).toMatch(/^codex-token-v1:parent\|/)
    const parentFile = feed([meta('parent'), count(2, U(7, 0, 1), U(7, 0, 1))])
    expect(parentFile.usage[0].dedupeKey).toBe(fork.usage[0].dedupeKey)
  })

  it('tolerates malformed lines', () => {
    const state = initialCodexFileState()
    for (const l of ['', 'not json', '[]', '"x"', 'null', '{"type":"event_msg","payload":5}', '{"type":"event_msg","payload":{"type":"token_count"}}', '{"timestamp":"nope","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":5}}}}']) {
      expect(parseCodexLine(l, state)).toBeNull()
    }
    expect(parseCodexLine(count(1, { input_tokens: 'x' }, { input_tokens: 1 }), state)).toBeNull()
  })

  it('survives a JSON round trip of the state mid-file', () => {
    const lines = [meta('s1', { forked_from_id: 'p' }), ctx(1, 'gpt-5-codex'), count(2, U(10, 0, 1), U(10, 0, 1)), count(3, U(20, 5, 2), U(30, 5, 3))]
    const whole = feed(lines)
    const first = feed(lines.slice(0, 3))
    const restored = JSON.parse(JSON.stringify(first.state)) as CodexFileState
    const second = feed(lines.slice(3), restored)
    expect([...first.usage, ...second.usage]).toEqual(whole.usage)
    expect(restored).toEqual(whole.state)
  })
})

describe('limits', () => {
  const rl = (extra: Record<string, unknown>, legacy = false): string =>
    legacy
      ? row(10, 'event_msg', { type: 'token_count', info: { rate_limits: extra } })
      : row(10, 'event_msg', { type: 'token_count', info: null, rate_limits: extra })
  const limits = (line: string): CodexLineResult['limits'] => feed([line]).results[0]?.limits

  it('reads rate_limits from the payload and from info (legacy)', () => {
    const body = { plan_type: 'plus', account_id: 'acc-1', primary: { used_percent: 12.5, window_minutes: 300, resets_at: 1779200000 } }
    for (const legacy of [false, true]) {
      const l = limits(rl(body, legacy))!
      expect(l).toMatchObject({ at: BASE + 10_000, account: 'acc-1', plan: 'plus' })
      expect(l.windows).toEqual([
        { provider: 'codex', account: 'acc-1', id: 'session', kind: 'session', label: '5-hour window', usedPercent: 12.5, resetsAt: 1779200000_000 }
      ])
    }
  })

  it('classifies 300 and 10080 minutes whatever the position', () => {
    const l = limits(rl({ primary: { used_percent: 40, window_minutes: 10080, resets_at: 2e9 }, secondary: { used_percent: 10, window_minutes: 300, resets_at: 1.9e9 } }))!
    expect(l.windows.map((w) => [w.id, w.kind, w.label, w.usedPercent])).toEqual([
      ['weekly', 'weekly', 'Weekly', 40],
      ['session', 'session', '5-hour window', 10]
    ])
    expect(l.account).toBe('default')
    expect(l.plan).toBeNull()
  })

  it('takes primary as session and secondary as weekly for a pair without minutes', () => {
    const l = limits(rl({ primary: { used_percent: 20, resets_at: 2e9 }, secondary: { used_percent: 60, resets_at: 2e9 } }))!
    expect(l.windows.map((w) => [w.id, w.usedPercent])).toEqual([['session', 20], ['weekly', 60]])
  })

  it('infers the missing side from one recognised duration', () => {
    const l = limits(rl({ primary: { used_percent: 20, resets_at: 2e9 }, secondary: { used_percent: 60, window_minutes: 300, resets_at: 2e9 } }))!
    expect(l.windows.map((w) => [w.id, w.usedPercent])).toEqual([['weekly', 20], ['session', 60]])
  })

  it('treats a single window as weekly, unless it is recognised as 5-hour', () => {
    expect(limits(rl({ primary: { used_percent: 30, resets_at: 2e9 } }))!.windows.map((w) => w.id)).toEqual(['weekly'])
    expect(limits(rl({ secondary: { used_percent: 30, resets_at: 2e9 } }))!.windows.map((w) => w.id)).toEqual(['weekly'])
    expect(limits(rl({ primary: { used_percent: 30, window_minutes: 300, resets_at: 2e9 } }))!.windows.map((w) => w.id)).toEqual(['session'])
  })

  it('keeps other durations as their own window', () => {
    const l = limits(rl({ primary: { used_percent: 5, window_minutes: 1440, resets_at: 2e9 }, secondary: { used_percent: 6, window_minutes: 10080, resets_at: 2e9 } }))!
    expect(l.windows.map((w) => [w.id, w.kind, w.label])).toEqual([
      ['weekly', 'weekly', 'Weekly'],
      ['window:1440', 'other', '1-day window']
    ])
  })

  it('parses resets_at as ISO, epoch seconds, epoch ms and resets_in_seconds', () => {
    const at = (w: Record<string, unknown>): number | null | undefined =>
      limits(rl({ primary: { used_percent: 50, window_minutes: 300, ...w } }))?.windows[0].resetsAt
    expect(at({ resets_at: '2026-05-19T15:00:00Z' })).toBe(Date.parse('2026-05-19T15:00:00Z'))
    expect(at({ resets_at: 1779200000 })).toBe(1779200000_000)
    expect(at({ resets_at: 1779200000123 })).toBe(1779200000123)
    expect(at({ resets_at: '1779200000' })).toBe(1779200000_000)
    expect(at({ resets_in_seconds: 600 })).toBe(BASE + 10_000 + 600_000)
    expect(at({})).toBeNull()
    expect(at({ resets_at: 'garbage' })).toBeNull()
  })

  it('clamps percentages, does not scale fractions, and nulls the reset of an idle window', () => {
    const l = limits(rl({ primary: { used_percent: 0.5, window_minutes: 300, resets_at: 2e9 }, secondary: { used_percent: 130, window_minutes: 10080, resets_at: 2e9 } }))!
    expect(l.windows.map((w) => w.usedPercent)).toEqual([0.5, 100])
    const idle = limits(rl({ primary: { used_percent: 0, window_minutes: 300, resets_at: 2e9 } }))!
    expect(idle.windows[0]).toMatchObject({ usedPercent: 0, resetsAt: null })
  })

  it('ignores missing, empty or percent-less rate_limits and keeps usage', () => {
    expect(limits(count(1, U(5, 0, 1), U(5, 0, 1)))).toBeUndefined()
    expect(limits(rl({}))).toBeUndefined()
    expect(limits(rl({ primary: { window_minutes: 300 } }))).toBeUndefined()
    expect(limits(rl({ primary: null, secondary: 'x' }))).toBeUndefined()
    const both = feed([meta('s1'), count(1, U(5, 0, 1), U(5, 0, 1), { rate_limits: { primary: { used_percent: 1, resets_at: 2e9 } } })])
    expect(both.results[0].usage).toHaveLength(1)
    expect(both.results[0].limits?.windows).toHaveLength(1)
  })
})

describe('compaction', () => {
  const BEFORE = U(1000, 400, 50, 10)
  const COMPACTION = U(5000, 3000, 700)
  const AFTER = U(200, 100, 30, 5)
  const ZERO = U(0, 0, 0)
  const add = (...us: Array<Record<string, number>>): Record<string, number> => {
    const out: Record<string, number> = {}
    for (const k of Object.keys(BEFORE)) out[k] = us.reduce((a, u) => a + u[k], 0)
    return out
  }

  function rollout(shape: 'uncarried' | 'carried' | 'carried_after' | 'unrecorded', id = 's1', at = 0): string[] {
    const omitted = shape === 'uncarried' || shape === 'unrecorded'
    const thread = add(BEFORE, COMPACTION)
    const record = { response_id: 'resp-compact', turn_id: 'turn-1', usage: COMPACTION, thread_token_usage: thread }
    const rows = [
      row(at, 'session_meta', { id, cwd: '/w' }),
      row(at + 1, 'turn_context', { turn_id: 'turn-1', model: 'gpt-5-codex' }),
      count(at + 2, BEFORE, add(BEFORE))
    ]
    if (shape !== 'unrecorded') rows.push(row(at + 3, 'token_usage_record', record))
    if (shape === 'carried') rows.push(count(at + 3, COMPACTION, thread))
    rows.push(row(at + 3, 'compacted', shape === 'unrecorded' ? {} : { compaction_response_id: 'resp-compact', latest_token_usage_record: record }))
    rows.push(row(at + 4, 'turn_context', { turn_id: 'turn-1', model: 'gpt-5-codex' }))
    if (shape === 'carried_after') rows.push(count(at + 3, COMPACTION, thread))
    else if (omitted) rows.push(count(at + 5, ZERO, add(BEFORE)))
    rows.push(count(at + 6, AFTER, omitted ? add(BEFORE, AFTER) : add(BEFORE, COMPACTION, AFTER)))
    return rows
  }

  const sum = (us: CodexParsedUsage[]): number[] =>
    us.reduce((a, u) => [a[0] + u.record.tokens.input, a[1] + u.record.tokens.cacheRead, a[2] + u.record.tokens.output], [0, 0, 0])
  const single = [600 + 2000 + 100, 400 + 3000 + 100, 50 + 700 + 30]

  it('bills an uncarried compaction exactly once', () => {
    const run = feed(rollout('uncarried'))
    expect(run.usage).toHaveLength(2)
    const flushed = flushCodexCompactions(run.state)
    expect(flushed).toHaveLength(1)
    expect(flushed[0].record).toMatchObject({ ts: BASE + 3000, model: 'gpt-5-codex', projectKey: '/w' })
    expect(flushed[0].record.tokens).toMatchObject({ input: 2000, cacheRead: 3000, output: 700 })
    expect(flushed[0].dedupeKey).toMatch(/^codex-compaction-v1:s1\|/)
    expect(sum([...run.usage, ...flushed])).toEqual(single)
    expect(flushCodexCompactions(run.state)).toEqual([])
  })

  it.each(['carried', 'carried_after'] as const)('does not bill a compaction a token_count carries (%s)', (shape) => {
    const run = feed(rollout(shape))
    expect(run.usage).toHaveLength(3)
    expect(flushCodexCompactions(run.state)).toEqual([])
    expect(sum(run.usage)).toEqual(single)
  })

  it('bills a compaction without a token_usage_record when the compacted event inlines it', () => {
    const lines = rollout('uncarried').filter((l) => !l.includes('"token_usage_record"'))
    const run = feed(lines)
    expect(flushCodexCompactions(run.state)).toHaveLength(1)
  })

  it('ignores a compaction the log cannot identify', () => {
    const run = feed(rollout('unrecorded'))
    expect(flushCodexCompactions(run.state)).toEqual([])
  })

  it('does not repeat a flushed compaction after more lines, across a state round trip', () => {
    const lines = rollout('uncarried')
    const cut = lines.findIndex((l) => l.includes('"compacted"')) + 1
    const first = feed(lines.slice(0, cut))
    const flushed1 = flushCodexCompactions(first.state)
    expect(flushed1).toHaveLength(1)
    const restored = JSON.parse(JSON.stringify(first.state)) as CodexFileState
    const second = feed(lines.slice(cut), restored)
    expect(flushCodexCompactions(second.state)).toEqual([])
    expect(sum([...first.usage, ...second.usage, ...flushed1])).toEqual(single)
  })

  it('collapses a resumed history copy against the original by key', () => {
    const a = flushCodexCompactions(feed(rollout('uncarried', 's1', 0)).state)
    const b = flushCodexCompactions(feed(rollout('uncarried', 's1', 500)).state)
    expect(a[0].dedupeKey).not.toBeNull()
    expect(b[0].dedupeKey).toBe(a[0].dedupeKey)
    expect(b[0].record.ts).not.toBe(a[0].record.ts)
  })

  it('bills a carried compaction only through its token_count even if a later line repeats its usage', () => {
    const run = feed([...rollout('carried'), count(20, COMPACTION, add(BEFORE, COMPACTION, AFTER, COMPACTION))])
    expect(flushCodexCompactions(run.state)).toEqual([])
  })
})
