// Adapted from agent-lookout (MIT, © 2026 Idris Olubisi): src/core/waits/waitTotals.ts, measured.ts
import type {
  LongestWait,
  MeasuredSpan,
  OverviewEvent,
  WaitBar,
  WaitDayTotal,
  WaitFigure,
  WaitTotals,
  WaitTransition,
  WaitingNow
} from './types'
import { HOUR_MS, MAX_SAMPLE_GAP_MS, type AgentFilter } from './buckets'

// How long agents waited on the user, worked out from the ledger the daemon
// keeps: the moves into and out of needs-you, the stretches that were
// measured, and the agents waiting now. Pure: the clock and the time zone are
// arguments.
//
// Only measured time counts. A wait that runs on while Agent Studio's daemon was
// stopped, or the computer slept, counts for the time either side that was
// measured, and is still one wait. Each agent's waits are added up, so two
// agents waiting at once for a minute are two minutes.

/** Days of ledger the daemon keeps. */
export const LEDGER_DAYS = 9
/** How many local days "the last 7 days" are: today and the six before it. */
export const WAIT_DAYS = 7
/** Agents named in each "Longest waits" list. */
export const MAX_LONGEST = 5

/** What counts as a day: the local one in the app, UTC in tests. */
export interface DayZone {
  /** The start of the day `at` falls in. */
  midnightOf(at: number): number
  /** The start of the day `days` days after (or before, when negative) the day that starts at `midnight`. */
  shiftDays(midnight: number, days: number): number
  /** `2026-10-22` for the day that `at` falls in. */
  dayKey(at: number): string
}

const pad2 = (n: number): string => String(n).padStart(2, '0')

export const LOCAL_ZONE: DayZone = {
  midnightOf(at) {
    const d = new Date(at)
    d.setHours(0, 0, 0, 0)
    return d.getTime()
  },
  // Through the calendar, so a day where the clocks change is 23 or 25 hours long.
  shiftDays(midnight, days) {
    const d = new Date(midnight)
    d.setDate(d.getDate() + days)
    d.setHours(0, 0, 0, 0)
    return d.getTime()
  },
  dayKey(at) {
    const d = new Date(at)
    return `${d.getFullYear()}-${pad2(d.getMonth() + 1)}-${pad2(d.getDate())}`
  }
}

export const UTC_ZONE: DayZone = {
  midnightOf: (at) => Math.floor(at / 86_400_000) * 86_400_000,
  shiftDays: (midnight, days) => midnight + days * 86_400_000,
  dayKey: (at) => new Date(at).toISOString().slice(0, 10)
}

/**
 * The move into or out of `needs-you` that an event makes, `null` for any
 * other event. An agent that ends while it waits moves out.
 */
export function transitionOfEvent(event: OverviewEvent): WaitTransition | null {
  if (!event.agentId) return null
  let direction: WaitTransition['direction'] | null = null
  if (event.kind === 'status') {
    if (event.to === 'needs-you') direction = 'in'
    else if (event.from === 'needs-you') direction = 'out'
  } else if (event.kind === 'appeared') {
    if (event.to === 'needs-you') direction = 'in'
  } else if (event.kind === 'ended') {
    if (event.from === 'needs-you') direction = 'out'
  }
  if (!direction) return null
  return { agentId: event.agentId, name: event.name, projectDir: event.projectDir, at: event.at, direction }
}

// ── Measured time ────────────────────────────────────────────────────────────

/** Spans in time order with none overlapping: those that touch or overlap are joined. */
export function joinSpans(spans: readonly MeasuredSpan[]): MeasuredSpan[] {
  const ordered = spans.filter((s) => s.end >= s.start).sort((a, b) => a.start - b.start)
  const out: MeasuredSpan[] = []
  for (const span of ordered) {
    const last = out[out.length - 1]
    if (last && span.start <= last.end) last.end = Math.max(last.end, span.end)
    else out.push({ ...span })
  }
  return out
}

/**
 * The spans that count up to `now`: joined, the newest carried on to `now`
 * while its last point is fresh and no daemon start came after it, and cut at
 * `now`.
 */
export function measuredUntil(spans: readonly MeasuredSpan[], starts: readonly number[], now: number): MeasuredSpan[] {
  const joined = joinSpans(spans)
  const newest = joined[joined.length - 1]
  if (
    newest &&
    newest.end < now &&
    now - newest.end <= MAX_SAMPLE_GAP_MS &&
    !starts.some((s) => s > newest.end && s <= now)
  ) {
    newest.end = now
  }
  return joined
    .map((s) => ({ start: s.start, end: Math.min(s.end, now) }))
    .filter((s) => s.end > s.start)
}

/** How much of `from` to `to` the spans cover. They must be joined and in time order. */
export function coveredWithin(spans: readonly MeasuredSpan[], from: number, to: number): number {
  if (to <= from) return 0
  let sum = 0
  for (const span of spans) {
    if (span.end <= from) continue
    if (span.start >= to) break
    sum += Math.min(span.end, to) - Math.max(span.start, from)
  }
  return sum
}

// ── Waits ────────────────────────────────────────────────────────────────────

interface Wait {
  agentId: string
  from: number
  to: number
  /** Still under way: the agent needs the user now. */
  open: boolean
}

export interface WaitsInput {
  transitions: readonly WaitTransition[]
  spans: readonly MeasuredSpan[]
  /** Each start of the daemon. Without them, a stretch may be carried across a restart. */
  starts?: readonly number[]
  waitingNow: readonly WaitingNow[]
  now: number
  /** Default: the local time zone. */
  zone?: DayZone
  /** Leaves out the agents it rejects. */
  agentFilter?: AgentFilter
}

/** The start of the stretch that holds `at`, `null` when none does. */
function spanStartAt(spans: readonly MeasuredSpan[], at: number): number | null {
  for (const s of spans) if (s.start <= at && at <= s.end) return s.start
  return null
}

/** The end of the stretch that holds `at`, `at` itself when none does. */
function spanEndAt(spans: readonly MeasuredSpan[], at: number): number {
  for (const s of spans) if (s.start <= at && at <= s.end) return s.end
  return at
}

/**
 * Every wait the ledger holds, oldest first: a move into needs-you to the
 * agent's next move out of it.
 * - A move out with no move in held (it left the ledger's 9 days, or the wait
 *   began before the daemon watched) counts from the start of the measured
 *   stretch it ended in, or from the agent's previous move when that is later.
 * - A wait with no end runs to `now` while the agent still needs the user
 *   (open). For an agent that does not, it ended unseen (the daemon was
 *   stopped): it runs to the end of the stretch it began in.
 * - An agent waiting now that no move says began is counted from `since`.
 */
function waitsOf(input: WaitsInput, spans: readonly MeasuredSpan[]): { waits: Wait[]; names: Map<string, string> } {
  const { now, agentFilter } = input
  const waitingNow = new Map(
    input.waitingNow.filter((w) => !agentFilter || agentFilter(w.agentId)).map((w) => [w.agentId, w])
  )
  const byAgent = new Map<string, WaitTransition[]>()
  for (const t of input.transitions) {
    if (t.at > now || (agentFilter && !agentFilter(t.agentId))) continue
    const own = byAgent.get(t.agentId)
    if (own) own.push(t)
    else byAgent.set(t.agentId, [t])
  }

  const waits: Wait[] = []
  const names = new Map<string, string>()
  const unended = new Set<string>()
  for (const [agentId, own] of byAgent) {
    own.sort((a, b) => a.at - b.at)
    let began: number | null = null
    let previous = -Infinity
    for (const t of own) {
      names.set(agentId, t.name)
      if (t.direction === 'in') {
        if (began === null) began = t.at
      } else {
        if (began === null) {
          const start = spanStartAt(spans, t.at)
          if (start !== null) began = Math.max(start, previous)
        }
        if (began !== null) waits.push({ agentId, from: began, to: t.at, open: false })
        began = null
      }
      previous = t.at
    }
    if (began !== null) {
      const open = waitingNow.has(agentId)
      waits.push({ agentId, from: began, to: open ? now : spanEndAt(spans, began), open })
      unended.add(agentId)
    }
  }
  for (const [agentId, w] of waitingNow) {
    names.set(agentId, w.name)
    if (unended.has(agentId)) continue
    const own = byAgent.get(agentId)
    const last = own ? own[own.length - 1].at : -Infinity
    waits.push({ agentId, from: Math.max(w.since, last), to: now, open: true })
  }
  const kept = waits.filter((w) => w.to > w.from)
  kept.sort((a, b) => a.from - b.from)
  return { waits: kept, names }
}

interface PeriodTotal extends WaitFigure {
  byAgent: Map<string, { ms: number; count: number }>
}

function totalIn(waits: readonly Wait[], spans: readonly MeasuredSpan[], from: number, to: number): PeriodTotal {
  const total: PeriodTotal = { ms: 0, count: 0, measuredMs: coveredWithin(spans, from, to), openMs: 0, byAgent: new Map() }
  for (const wait of waits) {
    if (wait.from >= to || wait.to <= from) continue
    const ms = coveredWithin(spans, Math.max(wait.from, from), Math.min(wait.to, to))
    if (ms <= 0) continue
    total.ms += ms
    total.count += 1
    if (wait.open) total.openMs += ms
    const own = total.byAgent.get(wait.agentId) ?? { ms: 0, count: 0 }
    own.ms += ms
    own.count += 1
    total.byAgent.set(wait.agentId, own)
  }
  return total
}

const figureOf = ({ ms, count, measuredMs, openMs }: PeriodTotal): WaitFigure => ({ ms, count, measuredMs, openMs })

function longestOf(
  total: PeriodTotal,
  names: ReadonlyMap<string, string>,
  waitingNow: ReadonlySet<string>
): LongestWait[] {
  return [...total.byAgent]
    .map(([agentId, own]) => ({
      agentId,
      name: names.get(agentId) ?? agentId,
      ms: own.ms,
      count: own.count,
      waitingNow: waitingNow.has(agentId)
    }))
    .sort((a, b) => b.ms - a.ms || a.name.localeCompare(b.name))
    .slice(0, MAX_LONGEST)
}

/**
 * How long agents waited today (from local midnight) and over the last 7 days
 * (today and the six before it, from midnight), cut into local days, with the
 * five agents that waited longest in each.
 */
export function waitTotals(input: WaitsInput): WaitTotals {
  const { now } = input
  const zone = input.zone ?? LOCAL_ZONE
  const spans = measuredUntil(input.spans, input.starts ?? [], now)
  const { waits, names } = waitsOf(input, spans)
  const waitingNow = new Set(
    input.waitingNow.filter((w) => !input.agentFilter || input.agentFilter(w.agentId)).map((w) => w.agentId)
  )
  const today = zone.midnightOf(now)
  const weekStart = zone.shiftDays(today, -(WAIT_DAYS - 1))

  const todayTotal = totalIn(waits, spans, today, now)
  const weekTotal = totalIn(waits, spans, weekStart, now)
  const byDay: WaitDayTotal[] = []
  for (let start = weekStart; start <= today; start = zone.shiftDays(start, 1)) {
    const end = Math.min(zone.shiftDays(start, 1), now)
    const figure = figureOf(totalIn(waits, spans, start, end))
    byDay.push({ day: zone.dayKey(start), ...figure, measured: figure.measuredMs > 0 })
  }
  return {
    today: figureOf(todayTotal),
    week: figureOf(weekTotal),
    byDay,
    longest: { today: longestOf(todayTotal, names, waitingNow), week: longestOf(weekTotal, names, waitingNow) }
  }
}

export interface LastHourWaits {
  /** Longest first. */
  bars: WaitBar[]
  /** Where the bars begin: an hour back, or the daemon's start when that is later. */
  from: number
  /** Time in `from`..`now` that was not measured. */
  unmeasuredMs: number
}

/**
 * Who waited on the user in the last hour, one bar per agent, longest first,
 * and how much of that hour was not measured. `options.from` moves the
 * beginning of the window (the caller passes the daemon's start when later).
 */
export function lastHourWaits(
  transitions: readonly WaitTransition[],
  waitingNow: readonly WaitingNow[],
  spans: readonly MeasuredSpan[],
  now: number,
  options: { starts?: readonly number[]; agentFilter?: AgentFilter; from?: number } = {}
): LastHourWaits {
  const from = Math.max(now - HOUR_MS, options.from ?? 0)
  const measured = measuredUntil(spans, options.starts ?? [], now)
  const { waits, names } = waitsOf(
    { transitions, spans, waitingNow, now, agentFilter: options.agentFilter },
    measured
  )
  const { byAgent } = totalIn(waits, measured, from, now)
  const open = new Set<string>()
  for (const wait of waits) if (wait.open) open.add(wait.agentId)
  const bars: WaitBar[] = [...byAgent]
    .map(([agentId, own]) => ({ agentId, name: names.get(agentId) ?? agentId, ms: own.ms, count: own.count, open: open.has(agentId) }))
    .sort((a, b) => b.ms - a.ms || a.name.localeCompare(b.name))
  return { bars, from, unmeasuredMs: Math.max(0, now - from - coveredWithin(measured, from, now)) }
}
