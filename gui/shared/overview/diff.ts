import type { HistoryPoint, OverviewEvent, OverviewSession, OverviewStatus } from './types'

/**
 * What changed between two snapshots, as events.
 *
 * - An agent only in `next` has appeared; only in `prev`, it has ended (it was
 *   removed from Agent Studio).
 * - An agent in both with another status changed status. Leaving `needs-you`
 *   carries `waitedMs`, measured from the previous `statusSince`.
 * - Nothing else is an event: a new name, reason or branch is not reported.
 *
 * `prev === null` is the first snapshot after the daemon started. Agents that
 * already exist did not appear at that moment, so no event is emitted for them
 * (agent-lookout shows a "Started watching" row instead; here the `start`
 * record and the `resumed` event cover it).
 */
// Adapted from agent-lookout (MIT, © 2026 Idris Olubisi): src/core/sessions/diff.ts
export function diffSnapshots(
  prev: OverviewSession[] | null,
  next: OverviewSession[],
  now: number
): OverviewEvent[] {
  if (prev === null) return []
  const before = new Map(prev.map((s) => [s.agentId, s]))
  const after = new Set(next.map((s) => s.agentId))
  const events: OverviewEvent[] = []

  for (const s of next) {
    const old = before.get(s.agentId)
    if (!old) {
      events.push({
        id: `${s.agentId}@${now}:appeared`,
        at: now,
        kind: 'appeared',
        agentId: s.agentId,
        name: s.name,
        projectDir: s.projectDir,
        to: s.status
      })
    } else if (old.status !== s.status) {
      const leftWaiting = old.status === 'needs-you'
      events.push({
        id: `${s.agentId}@${now}:status`,
        at: now,
        kind: 'status',
        agentId: s.agentId,
        name: s.name,
        projectDir: s.projectDir,
        from: old.status,
        to: s.status,
        ...(s.status === 'needs-you' && s.waitingReason ? { waitingReason: s.waitingReason } : {}),
        ...(leftWaiting ? { waitedMs: Math.max(0, now - old.statusSince) } : {})
      })
    }
  }

  for (const old of prev) {
    if (after.has(old.agentId)) continue
    events.push({
      id: `${old.agentId}@${now}:ended`,
      at: now,
      kind: 'ended',
      agentId: old.agentId,
      name: old.name,
      projectDir: old.projectDir,
      from: old.status,
      ...(old.status === 'needs-you' ? { waitedMs: Math.max(0, now - old.statusSince) } : {})
    })
  }
  return events
}

/** The sample of one snapshot: counts per status and each agent's status. */
export function pointFromSessions(sessions: OverviewSession[], at: number): HistoryPoint {
  const statuses: Record<string, OverviewStatus> = {}
  let needsYou = 0
  let working = 0
  let idle = 0
  for (const s of sessions) {
    statuses[s.agentId] = s.status
    if (s.status === 'needs-you') needsYou++
    else if (s.status === 'working') working++
    else if (s.status === 'idle') idle++
  }
  return { at, needsYou, working, idle, total: sessions.length, statuses }
}

/** Longest time between two points that still counts as continuous watching. */
export const GAP_AFTER_MS = 15_000

/** The `resumed` event for watching that picked up `gapMs` after the last point, `null` when there was no gap. */
export function resumedEvent(lastPointAt: number | null, now: number): OverviewEvent | null {
  if (lastPointAt === null) return null
  const gapMs = now - lastPointAt
  if (gapMs <= GAP_AFTER_MS) return null
  return { id: `resumed@${now}`, at: now, kind: 'resumed', agentId: '', name: '', projectDir: '', gapMs }
}
