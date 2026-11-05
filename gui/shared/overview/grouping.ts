import { isEnded } from './status'
import type { OverviewSession } from './types'

export interface SessionGroup {
  key: string
  label: string
  sessions: OverviewSession[]
  /** Rows shown before "Show more"; no limit when unset. */
  limit?: number
}

export const NO_REPOSITORY = 'No repository'

/** Rows shown in the Finished / Failed and Not running groups before "Show more". */
export const ENDED_GROUP_LIMIT = 10

/** Order within the list: Working, Idle, Idle (stale), Failed, Finished, Not running, Starting. */
function rank(s: OverviewSession): number {
  switch (s.status) {
    case 'needs-you':
      return 0
    case 'working':
      return 1
    case 'idle':
      return s.stale ? 3 : 2
    case 'failed':
      return 4
    case 'finished':
      return 5
    case 'not-running':
      return 6
    default:
      return 7
  }
}

/**
 * Sorts by status group, then longest in that status first (ended sessions:
 * most recently ended first), then name.
 */
function sortByStatus(sessions: readonly OverviewSession[]): OverviewSession[] {
  const since = (s: OverviewSession) => (isEnded(s.status) ? -s.statusSince : s.statusSince)
  return [...sessions].sort(
    (a, b) => rank(a) - rank(b) || since(a) - since(b) || a.name.localeCompare(b.name)
  )
}

/**
 * Groups for the List layout: Working, Idle (non-stale then stale), Finished /
 * Failed (failed first), Starting. Sessions that need the user are left out:
 * they are in the Needs you panel. Empty groups are dropped.
 */
export function groupByStatus(sessions: readonly OverviewSession[]): SessionGroup[] {
  const sorted = sortByStatus(sessions.filter((s) => s.status !== 'needs-you'))
  const defs: { key: string; label: string; ranks: number[]; limit?: number }[] = [
    { key: 'working', label: 'Working', ranks: [1] },
    { key: 'idle', label: 'Idle', ranks: [2, 3] },
    { key: 'ended', label: 'Finished / Failed', ranks: [4, 5], limit: ENDED_GROUP_LIMIT },
    { key: 'not-running', label: 'Not running', ranks: [6], limit: ENDED_GROUP_LIMIT },
    { key: 'starting', label: 'Starting', ranks: [7] }
  ]
  return defs
    .map((d) => ({
      key: d.key,
      label: d.label,
      sessions: sorted.filter((s) => d.ranks.includes(rank(s))),
      ...(d.limit !== undefined ? { limit: d.limit } : {})
    }))
    .filter((g) => g.sessions.length > 0)
}

/**
 * Groups for the Repos layout: by repository name (sorted), "No repository"
 * last. Rows keep the status order; needs-you sessions stay (the repo view
 * shows every session).
 */
export function groupByRepo(
  sessions: readonly OverviewSession[],
  repoOf: (session: OverviewSession) => string | null | undefined
): SessionGroup[] {
  const byRepo = new Map<string, OverviewSession[]>()
  for (const s of sortByStatus(sessions)) {
    const repo = repoOf(s) || NO_REPOSITORY
    const list = byRepo.get(repo)
    if (list) list.push(s)
    else byRepo.set(repo, [s])
  }
  return [...byRepo.entries()]
    .sort(([a], [b]) => {
      if (a === NO_REPOSITORY) return b === NO_REPOSITORY ? 0 : 1
      if (b === NO_REPOSITORY) return -1
      return a.localeCompare(b)
    })
    .map(([label, list]) => ({ key: label, label, sessions: list }))
}

export interface BoardColumn {
  key: 'needs-you' | 'working' | 'idle' | 'ended'
  label: string
  items: OverviewSession[]
  /** Sessions in the column that are not shown. */
  hidden: number
  /** Total in the column. */
  total: number
  /** Idle column only: how many of `total` are stale (they come last). */
  stale?: number
  /** Ended column only: not-running sessions, which are left out and only in the list. */
  notRunning?: number
}

/** The four Board columns, each capped at `max` cards. */
export function boardColumns(sessions: readonly OverviewSession[], max = 5): BoardColumn[] {
  const sorted = sortByStatus(sessions)
  const column = (
    key: BoardColumn['key'],
    label: string,
    pick: (s: OverviewSession) => boolean
  ): BoardColumn => {
    const all = sorted.filter(pick)
    const items = all.slice(0, max)
    return { key, label, items, hidden: all.length - items.length, total: all.length }
  }
  const idle = column('idle', 'Idle', (s) => s.status === 'idle')
  idle.stale = sorted.filter((s) => s.status === 'idle' && s.stale).length
  const ended = column('ended', 'Finished / Failed', (s) => s.status === 'finished' || s.status === 'failed')
  ended.notRunning = sorted.filter((s) => s.status === 'not-running').length
  return [
    column('needs-you', 'Needs you', (s) => s.status === 'needs-you'),
    column('working', 'Working', (s) => s.status === 'working' || s.status === 'starting'),
    idle,
    ended
  ]
}
