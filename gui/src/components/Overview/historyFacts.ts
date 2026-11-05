import type { OverviewEvent } from '@shared/overview/types'
import { formatDuration } from './format'
import type { LastWait } from './NeedsYouPanel'

const HOUR_MS = 60 * 60 * 1000

/** The newest wait that ended within the last hour (leaving needs-you), `undefined` when none. */
export function lastWaitFrom(events: OverviewEvent[], now: number): LastWait | undefined {
  let best: OverviewEvent | undefined
  for (const e of events) {
    if (e.from !== 'needs-you' || e.waitedMs === undefined || e.at < now - HOUR_MS) continue
    if (!best || e.at > best.at) best = e
  }
  return best && { name: best.name, waitedMs: best.waitedMs as number, endedAt: best.at }
}

const REASON_TEXT = {
  permission: 'is waiting for permission',
  question: 'is asking a question',
  other: 'is waiting for you'
} as const

/** What an agent did when it moved to a status, without its name. */
function toStatusText(e: OverviewEvent): string {
  switch (e.to) {
    case 'needs-you':
      return REASON_TEXT[e.waitingReason ?? 'other']
    case 'working':
      return 'is working'
    case 'idle':
      return 'went idle'
    case 'starting':
      return 'is starting'
    case 'finished':
      return 'finished'
    case 'failed':
      return 'failed'
    case 'not-running':
      return 'stopped'
    default:
      return 'changed status'
  }
}

/**
 * One line for an event, without the agent's name (`started`, `is working after waiting 4m 12s`).
 * A `resumed` event has no agent, so its line stands alone.
 */
export function eventText(e: OverviewEvent): string {
  switch (e.kind) {
    case 'appeared':
      return 'started'
    case 'ended':
      return 'ended'
    case 'resumed':
      return `Watching resumed \u2014 ${formatDuration(e.gapMs ?? 0, { compact: true })} not measured`
    default: {
      const waited = e.from === 'needs-you' && e.waitedMs !== undefined ? ` after waiting ${formatDuration(e.waitedMs)}` : ''
      return `${toStatusText(e)}${waited}`
    }
  }
}

/** Key for the mark of an event: its status, else `ended` or `resumed`. */
export function eventMarkKey(e: OverviewEvent): string {
  if (e.kind === 'ended' || e.kind === 'resumed') return e.kind
  return (e.to ?? 'idle').replace('-', '_')
}

/** The events of one agent, newest first. */
export function eventsOfAgent(events: OverviewEvent[], agentId: string): OverviewEvent[] {
  return events.filter((e) => e.agentId === agentId).sort((a, b) => b.at - a.at)
}
