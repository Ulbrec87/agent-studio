// Adapted from agent-lookout (MIT, © 2026 Idris Olubisi): src/core/history (measured time, bucketing)
import type { HistoryPoint, OverviewStatus } from './types'

/** Samples are 5 s apart; more than this between two of them means nothing was measured. */
export const MAX_SAMPLE_GAP_MS = 15_000
export const FIVE_MINUTES_MS = 5 * 60 * 1000
export const HOUR_MS = 60 * 60 * 1000

export type AgentFilter = (agentId: string) => boolean

export interface Span {
  start: number
  end: number
}

export type CountKey = 'needsYou' | 'working' | 'idle'

/** A stretch of measured time during which the counts were those of one sample. */
interface Piece extends Span {
  needsYou: number
  working: number
  idle: number
  total: number
  statuses: Record<string, OverviewStatus>
}

function countsOf(point: HistoryPoint, filter?: AgentFilter) {
  if (!filter) {
    return { needsYou: point.needsYou, working: point.working, idle: point.idle, total: point.total }
  }
  let needsYou = 0
  let working = 0
  let idle = 0
  let total = 0
  for (const [agentId, status] of Object.entries(point.statuses)) {
    if (!filter(agentId)) continue
    total += 1
    if (status === 'needs-you') needsYou += 1
    else if (status === 'working') working += 1
    else if (status === 'idle') idle += 1
  }
  return { needsYou, working, idle, total }
}

/**
 * The measured pieces inside the window, oldest first. A sample holds until the
 * next one, unless more than `MAX_SAMPLE_GAP_MS` lies between them or a daemon
 * start does: then the sample covers nothing after itself. The newest sample
 * holds up to `now` while it is fresh.
 */
function pieces(
  points: HistoryPoint[],
  starts: number[],
  now: number,
  windowMs: number,
  filter?: AgentFilter
): Piece[] {
  const from = now - windowMs
  const out: Piece[] = []
  for (let i = 0; i < points.length; i++) {
    const p = points[i]
    const next = points[i + 1]
    let end: number
    if (next) {
      if (next.at - p.at > MAX_SAMPLE_GAP_MS || starts.some((s) => s > p.at && s <= next.at)) continue
      end = next.at
    } else {
      if (now - p.at > MAX_SAMPLE_GAP_MS) continue
      end = now
    }
    const start = Math.max(p.at, from)
    end = Math.min(end, now)
    if (end <= start) continue
    out.push({ start, end, ...countsOf(p, filter), statuses: p.statuses })
  }
  return out
}

/** The spans of the window covered by samples, oldest first; touching pieces are merged. */
export function measuredSpans(
  points: HistoryPoint[],
  starts: number[],
  now: number,
  windowMs: number
): Span[] {
  const spans: Span[] = []
  for (const piece of pieces(points, starts, now, windowMs)) {
    const last = spans[spans.length - 1]
    if (last && piece.start <= last.end) last.end = Math.max(last.end, piece.end)
    else spans.push({ start: piece.start, end: piece.end })
  }
  return spans
}

export interface Bucket {
  start: number
  /** Nominal end; the current bucket ends in the future. */
  end: number
  /** Time inside the bucket (up to `now`) that was measured. */
  measuredMs: number
  avgNeedsYou: number
  avgWorking: number
  avgIdle: number
  avgTotal: number
}

export interface LastHourData {
  buckets: Bucket[]
  /** Sum over the last hour of the sessions waiting on the user, in ms. */
  totalWaitedMs: number
}

/** Time-weighted accumulation of the pieces into `count` buckets of `bucketMs` from `first`. */
function accumulate(
  list: Piece[],
  first: number,
  bucketMs: number,
  count: number,
  now: number
): Bucket[] {
  const sums = Array.from({ length: count }, () => ({ ms: 0, n: 0, w: 0, i: 0, t: 0 }))
  for (const piece of list) {
    const lo = Math.max(0, Math.floor((piece.start - first) / bucketMs))
    const hi = Math.min(count - 1, Math.floor((piece.end - 1 - first) / bucketMs))
    for (let b = lo; b <= hi; b++) {
      const bs = first + b * bucketMs
      const ms = Math.min(piece.end, bs + bucketMs, now) - Math.max(piece.start, bs)
      if (ms <= 0) continue
      const s = sums[b]
      s.ms += ms
      s.n += piece.needsYou * ms
      s.w += piece.working * ms
      s.i += piece.idle * ms
      s.t += piece.total * ms
    }
  }
  return sums.map((s, b) => ({
    start: first + b * bucketMs,
    end: first + (b + 1) * bucketMs,
    measuredMs: s.ms,
    avgNeedsYou: s.ms > 0 ? s.n / s.ms : 0,
    avgWorking: s.ms > 0 ? s.w / s.ms : 0,
    avgIdle: s.ms > 0 ? s.i / s.ms : 0,
    avgTotal: s.ms > 0 ? s.t / s.ms : 0
  }))
}

/**
 * Buckets on the clock's marks (the last one is the current, partial one) with
 * the time-weighted average counts over their measured time.
 */
export function lastHourBuckets(
  points: HistoryPoint[],
  starts: number[],
  now: number,
  options: { bucketMs?: number; count?: number; agentFilter?: AgentFilter } = {}
): LastHourData {
  const bucketMs = options.bucketMs ?? FIVE_MINUTES_MS
  const count = options.count ?? 12
  const lastStart = Math.floor(now / bucketMs) * bucketMs
  const first = lastStart - (count - 1) * bucketMs
  const list = pieces(points, starts, now, now - first, options.agentFilter)
  const buckets = accumulate(list, first, bucketMs, count, now)
  const hourFrom = now - HOUR_MS
  let totalWaitedMs = 0
  for (const piece of list) {
    const ms = piece.end - Math.max(piece.start, hourFrom)
    if (ms > 0) totalWaitedMs += piece.needsYou * ms
  }
  return { buckets, totalWaitedMs }
}

export interface SeriesBucket {
  start: number
  end: number
  measuredMs: number
  /** Average count over the measured time, `null` when nothing was measured. */
  value: number | null
}

/** The average of one count over `bucketCount` equal buckets ending at `now`. */
export function countSeries(
  points: HistoryPoint[],
  starts: number[],
  key: CountKey,
  now: number,
  windowMs: number,
  bucketCount: number,
  agentFilter?: AgentFilter
): SeriesBucket[] {
  const bucketMs = windowMs / bucketCount
  const first = now - windowMs
  const list = pieces(points, starts, now, windowMs, agentFilter)
  return accumulate(list, first, bucketMs, bucketCount, now).map((b) => ({
    start: b.start,
    end: b.end,
    measuredMs: b.measuredMs,
    value:
      b.measuredMs <= 0
        ? null
        : key === 'needsYou'
          ? b.avgNeedsYou
          : key === 'working'
            ? b.avgWorking
            : b.avgIdle
  }))
}

export interface Segment extends Span {
  status: OverviewStatus | 'unmeasured'
}

/**
 * Status segments per agent over the window, oldest first, equal neighbours
 * merged. An agent absent from a sample has no segment there. Unmeasured time
 * is a segment only for an agent seen before it, or, when it opens the window,
 * seen after it (an agent that first shows up after a gap has no segment for it).
 */
export function timelineSegments(
  points: HistoryPoint[],
  starts: number[],
  agentIds: string[],
  now: number,
  windowMs: number = HOUR_MS
): Record<string, Segment[]> {
  const from = now - windowMs
  const list = pieces(points, starts, now, windowMs)
  type Item = { span: Span; piece?: Piece }
  const items: Item[] = []
  let cursor = from
  for (const piece of list) {
    if (piece.start > cursor) items.push({ span: { start: cursor, end: piece.start } })
    items.push({ span: piece, piece })
    cursor = piece.end
  }
  if (cursor < now) items.push({ span: { start: cursor, end: now } })

  const result: Record<string, Segment[]> = {}
  for (const agentId of agentIds) {
    const segments: Segment[] = []
    let lastPresent = -1
    items.forEach((item, idx) => {
      if (item.piece && agentId in item.piece.statuses) lastPresent = idx
    })
    let seen = false
    items.forEach((item, idx) => {
      let status: Segment['status']
      if (item.piece) {
        if (!(agentId in item.piece.statuses)) return
        status = item.piece.statuses[agentId]
        seen = true
      } else {
        const leading = idx === 0
        if (!(seen || (leading && lastPresent > idx))) return
        status = 'unmeasured'
      }
      const prev = segments[segments.length - 1]
      if (prev && prev.status === status && prev.end === item.span.start) prev.end = item.span.end
      else segments.push({ start: item.span.start, end: item.span.end, status })
    })
    result[agentId] = segments
  }
  return result
}
