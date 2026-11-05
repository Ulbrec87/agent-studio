// Adapted from agent-lookout (MIT, © 2026 Idris Olubisi): src/dashboard/components/sessions/SessionsBoard.tsx, BoardCard.tsx

import { useMemo } from 'react'
import type { OverviewSession } from '@shared/overview/types'
import { boardColumns, type BoardColumn } from '@shared/overview/grouping'
import { Where, StatusFact, type SessionItemProps } from './SessionRow'
import { agentLabel } from './sessionFacts'
import styles from './Sessions.module.css'

const EMPTY_TEXT: Record<BoardColumn['key'], string> = {
  'needs-you': 'Nothing waiting',
  working: 'Nothing running',
  idle: 'Nothing idle',
  ended: 'Nothing finished'
}

function BoardCard({
  session,
  now,
  showProvider,
  onOpenDetails,
  onJumpToAgent,
  selected
}: SessionItemProps) {
  return (
    <li className={`${styles.boardCard} ${selected ? styles.selected : ''}`}>
      <button
        type="button"
        className={styles.boardCardMain}
        data-session-trigger={session.agentId}
        onClick={(e) => onOpenDetails(session.agentId, e.currentTarget)}
        aria-label={`Details of ${session.name}`}
      >
        <span className={styles.boardName} title={session.name}>
          {session.name}
        </span>
        <StatusFact session={session} now={now} />
        <Where session={session} />
        <span className={styles.boardAgent}>{agentLabel(session, showProvider)}</span>
      </button>
      <button
        type="button"
        className={styles.jump}
        onClick={() => onJumpToAgent(session.agentId)}
        aria-label={`Jump to ${session.name}`}
      >
        Jump
      </button>
    </li>
  )
}

interface SessionsBoardProps {
  sessions: OverviewSession[]
  now: number
  showProvider: boolean
  selectedId: string | null
  onOpenDetails: SessionItemProps['onOpenDetails']
  onJumpToAgent: SessionItemProps['onJumpToAgent']
}

export function SessionsBoard({ sessions, selectedId, ...rest }: SessionsBoardProps) {
  const columns = useMemo(() => boardColumns(sessions), [sessions])
  return (
    <div className={styles.boardWrap}>
      <div className={styles.board}>
        {columns.map((column) => (
          <section key={column.key} className={styles.boardColumn} aria-label={column.label}>
            <h4 className={styles.boardHeading}>
              <span>{column.label}</span>
              <span className={styles.boardCount}>
                {column.total}
                {column.stale ? ` · ${column.stale} stale` : ''}
              </span>
            </h4>
            {column.items.length === 0 ? (
              <p className={styles.boardEmpty}>{EMPTY_TEXT[column.key]}</p>
            ) : (
              <ul className={styles.boardList}>
                {column.items.map((s) => (
                  <BoardCard key={s.agentId} session={s} selected={s.agentId === selectedId} {...rest} />
                ))}
              </ul>
            )}
            {column.hidden > 0 && <p className={styles.boardMore}>and {column.hidden} more in the list</p>}
            {column.notRunning ? (
              <p className={styles.boardMore}>{column.notRunning} not running, in the list</p>
            ) : null}
          </section>
        ))}
      </div>
    </div>
  )
}
