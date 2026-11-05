import { describe, expect, it } from 'vitest'
import type { HistoryPoint, OverviewStatus } from './types'
import { countSeries, lastHourBuckets, measuredSpans, timelineSegments } from './buckets'

const MIN = 60_000
const T0 = Date.UTC(2026, 0, 1, 12, 0, 0) // on a 5 min mark

function point(at: number, statuses: Record<string, OverviewStatus>): HistoryPoint {
  const v = Object.values(statuses)
  return {
    at,
    needsYou: v.filter((s) => s === 'needs-you').length,
    working: v.filter((s) => s === 'working').length,
    idle: v.filter((s) => s === 'idle').length,
    total: v.length,
    statuses
  }
}

/** One sample every 5 s from `from` to `to` inclusive. */
function run(from: number, to: number, statuses: Record<string, OverviewStatus>): HistoryPoint[] {
  const out: HistoryPoint[] = []
  for (let t = from; t <= to; t += 5000) out.push(point(t, statuses))
  return out
}

describe('measuredSpans', () => {
  it('covers samples and extends the fresh newest one to now', () => {
    const points = run(T0, T0 + MIN, { a: 'working' })
    expect(measuredSpans(points, [], T0 + MIN + 3000, 10 * MIN)).toEqual([{ start: T0, end: T0 + MIN + 3000 }])
  })

  it('breaks on a gap above 15 s', () => {
    const points = [...run(T0, T0 + MIN, { a: 'working' }), ...run(T0 + 2 * MIN, T0 + 3 * MIN, { a: 'working' })]
    expect(measuredSpans(points, [], T0 + 3 * MIN, 10 * MIN)).toEqual([
      { start: T0, end: T0 + MIN },
      { start: T0 + 2 * MIN, end: T0 + 3 * MIN }
    ])
  })

  it('keeps a 15 s gap whole', () => {
    const points = [point(T0, {}), point(T0 + 15_000, {})]
    expect(measuredSpans(points, [], T0 + 15_000, MIN)).toEqual([{ start: T0, end: T0 + 15_000 }])
  })

  it('breaks on a start between two samples', () => {
    const points = run(T0, T0 + 20_000, { a: 'idle' })
    expect(measuredSpans(points, [T0 + 7000], T0 + 20_000, MIN)).toEqual([
      { start: T0, end: T0 + 5000 },
      { start: T0 + 10_000, end: T0 + 20_000 }
    ])
  })

  it('treats time before the first sample and a stale tail as unmeasured', () => {
    const points = run(T0 + 30 * MIN, T0 + 31 * MIN, {})
    expect(measuredSpans(points, [], T0 + 40 * MIN, 60 * MIN)).toEqual([
      { start: T0 + 30 * MIN, end: T0 + 31 * MIN }
    ])
  })

  it('clips to the window', () => {
    const points = run(T0, T0 + 10 * MIN, {})
    expect(measuredSpans(points, [], T0 + 10 * MIN, 4 * MIN)).toEqual([
      { start: T0 + 6 * MIN, end: T0 + 10 * MIN }
    ])
  })
})

describe('lastHourBuckets', () => {
  it('aligns to 5 minute marks with a partial last bucket', () => {
    const now = T0 + 7 * MIN
    const { buckets } = lastHourBuckets([], [], now)
    expect(buckets).toHaveLength(12)
    expect(buckets[11].start).toBe(T0 + 5 * MIN)
    expect(buckets[0].start).toBe(T0 - 50 * MIN)
    expect(buckets.every((b) => b.start % (5 * MIN) === 0)).toBe(true)
    expect(buckets.every((b) => b.measuredMs === 0)).toBe(true)
  })

  it('averages by time over measured time only', () => {
    const now = T0 + 5 * MIN
    // first half of the bucket 2 working + 1 needs-you, second half 1 idle
    const points = [
      ...run(T0, T0 + 2 * MIN + 25_000, { a: 'working', b: 'working', c: 'needs-you' }),
      ...run(T0 + 2 * MIN + 30_000, now, { a: 'idle' })
    ]
    const { buckets } = lastHourBuckets(points, [], now - 1)
    const b = buckets[11]
    expect(b.start).toBe(T0)
    expect(b.measuredMs).toBe(5 * MIN - 1)
    expect(b.avgNeedsYou).toBeCloseTo(0.5, 1)
    expect(b.avgWorking).toBeCloseTo(1, 1)
    expect(b.avgIdle).toBeCloseTo(0.5, 1)
    expect(b.avgTotal).toBeCloseTo(2, 1)
  })

  it('leaves a gap unmeasured and a partial bucket short', () => {
    const now = T0 + 2 * MIN
    const points = [...run(T0, T0 + 30_000, { a: 'working' }), ...run(T0 + MIN + 30_000, now, { a: 'working' })]
    const { buckets } = lastHourBuckets(points, [], now)
    const cur = buckets[11]
    expect(cur.start).toBe(T0)
    expect(cur.measuredMs).toBe(30_000 + 30_000)
    expect(cur.avgWorking).toBe(1)
  })

  it('sums waited time over the last hour', () => {
    const now = T0 + 20 * MIN
    const points = [
      ...run(T0, T0 + 4 * MIN + 55_000, { a: 'needs-you', b: 'needs-you' }),
      ...run(T0 + 5 * MIN, now, { a: 'working', b: 'idle' })
    ]
    expect(lastHourBuckets(points, [], now).totalWaitedMs).toBe(2 * 5 * MIN)
  })

  it('does not count waits older than an hour', () => {
    const now = T0 + 90 * MIN
    const points = [...run(T0, T0 + 10 * MIN, { a: 'needs-you' }), ...run(T0 + 89 * MIN, now, { a: 'idle' })]
    expect(lastHourBuckets(points, [], now).totalWaitedMs).toBe(0)
  })

  it('recomputes from per-agent statuses with a filter', () => {
    const now = T0 + MIN
    const points = run(T0, now, { a: 'needs-you', b: 'working', c: 'working' })
    const { buckets, totalWaitedMs } = lastHourBuckets(points, [], now, {
      agentFilter: (id) => id !== 'a'
    })
    expect(buckets[11].avgNeedsYou).toBe(0)
    expect(buckets[11].avgWorking).toBe(2)
    expect(buckets[11].avgTotal).toBe(2)
    expect(totalWaitedMs).toBe(0)
  })

  it('honours bucketMs and count', () => {
    const { buckets } = lastHourBuckets([], [], T0 + MIN, { bucketMs: MIN, count: 3 })
    expect(buckets.map((b) => b.start)).toEqual([T0 - MIN, T0, T0 + MIN])
  })
})

describe('timelineSegments', () => {
  it('merges equal statuses and splits on change', () => {
    const now = T0 + 3 * MIN
    const points = [
      ...run(T0, T0 + MIN - 5000, { a: 'working' }),
      ...run(T0 + MIN, now, { a: 'idle' })
    ]
    const seg = timelineSegments(points, [], ['a'], now, 3 * MIN)
    expect(seg.a).toEqual([
      { start: T0, end: T0 + MIN, status: 'working' },
      { start: T0 + MIN, end: now, status: 'idle' }
    ])
  })

  it('marks a gap unmeasured for an agent seen before it', () => {
    const now = T0 + 3 * MIN
    const points = [...run(T0, T0 + 30_000, { a: 'working' }), ...run(T0 + 2 * MIN, now, { a: 'working' })]
    const seg = timelineSegments(points, [], ['a'], now, 3 * MIN)
    expect(seg.a.map((s) => s.status)).toEqual(['working', 'unmeasured', 'working'])
    expect(seg.a[1]).toEqual({ start: T0 + 30_000, end: T0 + 2 * MIN, status: 'unmeasured' })
  })

  it('hatches the unmeasured start of the window only for agents seen later', () => {
    const now = T0 + 3 * MIN
    const points = run(T0 + 2 * MIN, now, { a: 'idle' })
    const seg = timelineSegments(points, [], ['a', 'ghost'], now, 3 * MIN)
    expect(seg.a[0]).toEqual({ start: T0, end: T0 + 2 * MIN, status: 'unmeasured' })
    expect(seg.ghost).toEqual([])
  })

  it('has no segment while an agent is absent, and none for a late arrival after a gap', () => {
    const now = T0 + 4 * MIN
    const points = [
      ...run(T0, T0 + 30_000, { a: 'idle' }),
      ...run(T0 + 2 * MIN, now, { a: 'idle', b: 'working' })
    ]
    const seg = timelineSegments(points, [], ['a', 'b'], now, 4 * MIN)
    expect(seg.b).toEqual([{ start: T0 + 2 * MIN, end: now, status: 'working' }])
  })

  it('splits at a start', () => {
    const points = run(T0, T0 + 20_000, { a: 'working' })
    const seg = timelineSegments(points, [T0 + 7000], ['a'], T0 + 20_000, 20_000)
    expect(seg.a.map((s) => s.status)).toEqual(['working', 'unmeasured', 'working'])
  })
})

describe('countSeries', () => {
  it('averages one count per bucket and flags unmeasured ones', () => {
    const now = T0 + 15 * MIN
    const points = run(T0 + 5 * MIN, now, { a: 'working', b: 'idle' })
    const series = countSeries(points, [], 'working', now, 15 * MIN, 3)
    expect(series.map((s) => s.value)).toEqual([null, 1, 1])
    expect(countSeries(points, [], 'idle', now, 15 * MIN, 3)[1].value).toBe(1)
  })

  it('applies the filter', () => {
    const now = T0 + 5 * MIN
    const points = run(T0, now, { a: 'working', b: 'working' })
    const series = countSeries(points, [], 'working', now, 5 * MIN, 1, (id) => id === 'a')
    expect(series[0].value).toBe(1)
  })
})
