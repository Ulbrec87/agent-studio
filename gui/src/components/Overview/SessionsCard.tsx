// Adapted from agent-lookout (MIT, © 2026 Idris Olubisi): src/dashboard/components/sessions/SessionsCard.tsx

import { useMemo, useState } from 'react'
import type { OverviewSession } from '@shared/overview/types'
import { groupByRepo, groupByStatus, type SessionGroup } from '@shared/overview/grouping'
import { Card, EmptyState } from './Card'
import { SessionRow, type SessionItemProps } from './SessionRow'
import { SessionsBoard } from './SessionsBoard'
import { folderOf, repoOf } from './sessionFacts'
import styles from './Sessions.module.css'

export type SessionsLayout = 'list' | 'repos' | 'board'

export const LAYOUT_STORAGE_KEY = 'agentStudio.overview.sessionsLayout'

const LAYOUTS: { id: SessionsLayout; label: string }[] = [
  { id: 'list', label: 'List' },
  { id: 'repos', label: 'Repos' },
  { id: 'board', label: 'Board' }
]

function readLayout(): SessionsLayout {
  try {
    const stored = localStorage.getItem(LAYOUT_STORAGE_KEY)
    return LAYOUTS.some((l) => l.id === stored) ? (stored as SessionsLayout) : 'list'
  } catch {
    return 'list'
  }
}

function writeLayout(layout: SessionsLayout): void {
  try {
    localStorage.setItem(LAYOUT_STORAGE_KEY, layout)
  } catch {
    /* storage unavailable */
  }
}

interface SessionsCardProps {
  sessions: OverviewSession[]
  now: number
  showProvider: boolean
  selectedId: string | null
  onOpenDetails: SessionItemProps['onOpenDetails']
  onJumpToAgent: SessionItemProps['onJumpToAgent']
}

function Groups({
  groups,
  repos,
  selectedId,
  ...rest
}: {
  groups: SessionGroup[]
  repos: boolean
  selectedId: string | null
} & Omit<SessionsCardProps, 'sessions' | 'selectedId'>) {
  const [expanded, setExpanded] = useState<ReadonlySet<string>>(new Set())
  const toggle = (key: string) =>
    setExpanded((prev) => {
      const next = new Set(prev)
      if (!next.delete(key)) next.add(key)
      return next
    })

  return (
    <div className={styles.listWrap}>
      {groups.map((group) => {
        const capped = group.limit !== undefined && group.sessions.length > group.limit
        const open = expanded.has(group.key)
        const shown = capped && !open ? group.sessions.slice(0, group.limit) : group.sessions
        return (
          <section key={group.key} className={styles.group} aria-label={group.label}>
            <h4 className={styles.groupHeading}>
              <span>{group.label}</span>
              <span className={styles.groupCount}>{group.sessions.length}</span>
            </h4>
            <ul className={styles.rows}>
              {shown.map((s) => (
                <SessionRow
                  key={s.agentId}
                  session={s}
                  selected={s.agentId === selectedId}
                  hideFolder={repos && folderOf(s) === group.label}
                  {...rest}
                />
              ))}
            </ul>
            {capped && (
              <button
                type="button"
                className={styles.showMore}
                aria-expanded={open}
                onClick={() => toggle(group.key)}
              >
                {open ? 'Show less' : `Show ${group.sessions.length - shown.length} more`}
              </button>
            )}
          </section>
        )
      })}
    </div>
  )
}

export function SessionsCard({ sessions, selectedId, ...rest }: SessionsCardProps) {
  const [layout, setLayout] = useState<SessionsLayout>(readLayout)

  const choose = (next: SessionsLayout) => {
    setLayout(next)
    writeLayout(next)
  }

  const statusGroups = useMemo(() => groupByStatus(sessions), [sessions])
  const repoGroups = useMemo(() => groupByRepo(sessions, repoOf), [sessions])

  const control = (
    <div className={styles.segmented} role="group" aria-label="Sessions layout">
      {LAYOUTS.map((l) => (
        <button
          key={l.id}
          type="button"
          className={`${styles.segment} ${layout === l.id ? styles.segmentActive : ''}`}
          aria-pressed={layout === l.id}
          onClick={() => choose(l.id)}
        >
          {l.label}
        </button>
      ))}
    </div>
  )

  let body
  if (sessions.length === 0) {
    body = <EmptyState>No agents in this project</EmptyState>
  } else if (layout === 'board') {
    body = <SessionsBoard sessions={sessions} selectedId={selectedId} {...rest} />
  } else if (layout === 'repos') {
    body = <Groups groups={repoGroups} repos selectedId={selectedId} {...rest} />
  } else if (statusGroups.length === 0) {
    body = <EmptyState>Everything needs you. See the panel above.</EmptyState>
  } else {
    body = <Groups groups={statusGroups} repos={false} selectedId={selectedId} {...rest} />
  }

  return (
    <Card title="Sessions" area="sessions" meta={sessions.length} actions={control}>
      {body}
    </Card>
  )
}
