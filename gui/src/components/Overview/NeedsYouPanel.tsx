import { useMemo } from 'react'
import { PROVIDER_LABELS } from '@shared/types'
import type { OverviewSession, WaitingReason } from '@shared/overview/types'
import { formatDuration, formatMinute, projectName } from './format'
import styles from './NeedsYouPanel.module.css'

/** The last wait that ended, from the events history. */
export interface LastWait {
  name: string
  waitedMs: number
  endedAt: number
}

interface NeedsYouPanelProps {
  /** Sessions with status `needs-you`, in any order. */
  sessions: OverviewSession[]
  now: number
  /** True when the snapshot has agents of more than one provider. */
  showProvider: boolean
  onJumpToAgent: (agentId: string) => void
  /** Opens the details drawer; the agent name becomes a button when given. */
  onOpenDetails?: (agentId: string, trigger: HTMLElement | null) => void
  /** Rendered in the empty state when provided. */
  lastWait?: LastWait
}

export function reasonLabel(reason: WaitingReason | undefined): string {
  if (reason === 'permission') return 'Waiting for permission'
  if (reason === 'question') return 'Asked you a question'
  return 'Waiting for you'
}

function Item({
  session,
  now,
  showProvider,
  large,
  onJumpToAgent,
  onOpenDetails
}: {
  session: OverviewSession
  now: number
  showProvider: boolean
  large: boolean
  onJumpToAgent: (agentId: string) => void
  onOpenDetails?: (agentId: string, trigger: HTMLElement | null) => void
}) {
  const where = [
    session.worktreeBranch
      ? `${projectName(session.projectDir)} on ${session.worktreeBranch}`
      : projectName(session.projectDir),
    showProvider ? PROVIDER_LABELS[session.provider] : null
  ]
    .filter(Boolean)
    .join(' · ')

  return (
    <li className={`${styles.item} ${large ? styles.large : ''}`}>
      <div className={styles.main}>
        <div className={styles.head}>
          {onOpenDetails ? (
            <button
              type="button"
              className={`${styles.name} ${styles.nameButton}`}
              title={session.name}
              data-session-trigger={session.agentId}
              onClick={(e) => onOpenDetails(session.agentId, e.currentTarget)}
              aria-label={`Details of ${session.name}`}
            >
              {session.name}
            </button>
          ) : (
            <span className={styles.name} title={session.name}>
              {session.name}
            </span>
          )}
          <span className={styles.reason}>{reasonLabel(session.waitingReason)}</span>
        </div>
        <div className={styles.where} title={session.projectDir}>
          {where}
        </div>
        {session.waitingText && (
          <div className={styles.text} title={session.waitingText}>
            {session.waitingText}
          </div>
        )}
      </div>
      <span className={styles.timer} aria-label="Waiting for">
        {formatDuration(now - session.statusSince)}
      </span>
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

function AllClear({ lastWait, now }: { lastWait?: LastWait; now: number }) {
  return (
    <div className={styles.clear}>
      <div className={styles.clearMain}>
        <h3 className={styles.clearTitle}>
          <span className={styles.clearIcon} aria-hidden="true" />
          Nothing needs you
        </h3>
        <p className={styles.clearText}>When an agent stops to ask for something, it will show here.</p>
      </div>
      {lastWait && (
        <div className={styles.lastWait}>
          <span className={styles.lastWaitLabel}>Last wait</span>
          <span className={styles.lastWaitValue}>{formatDuration(lastWait.waitedMs)}</span>
          <span className={styles.lastWaitWho}>
            <strong>{lastWait.name}</strong>, answered at {formatMinute(lastWait.endedAt, now)}
          </span>
        </div>
      )}
    </div>
  )
}

export function NeedsYouPanel({ sessions, now, showProvider, onJumpToAgent, onOpenDetails, lastWait }: NeedsYouPanelProps) {
  const sorted = useMemo(
    () => [...sessions].sort((a, b) => a.statusSince - b.statusSince),
    [sessions]
  )

  if (sorted.length === 0) return <AllClear lastWait={lastWait} now={now} />

  return (
    <ul className={styles.list}>
      {sorted.map((s, index) => (
        <Item
          key={s.agentId}
          session={s}
          now={now}
          showProvider={showProvider}
          large={index === 0}
          onJumpToAgent={onJumpToAgent}
          onOpenDetails={onOpenDetails}
        />
      ))}
    </ul>
  )
}
