import type { OverviewSession } from '@shared/overview/types'
import { isEnded } from '@shared/overview/status'
import { formatAgo, formatClock, formatDuration } from './format'
import { agentLabel, branchOf, dotKey, folderOf, quietText, statusLabel } from './sessionFacts'
import styles from './Sessions.module.css'

export interface SessionItemProps {
  session: OverviewSession
  now: number
  /** True when the snapshot has agents of more than one provider. */
  showProvider: boolean
  /** Opens the details drawer; `trigger` gets the focus back when it closes. */
  onOpenDetails: (agentId: string, trigger: HTMLElement | null) => void
  onJumpToAgent: (agentId: string) => void
  /** True when this item is the one shown in the drawer. */
  selected?: boolean
}

/** Folder (unless it only repeats the repository name) and branch, or the short commit when detached. */
export function Where({ session, hideFolder }: { session: OverviewSession; hideFolder?: boolean }) {
  const branch = branchOf(session)
  const folder = hideFolder ? null : folderOf(session)
  return (
    <span className={styles.where} title={session.projectDir}>
      {folder && <span className={styles.folder}>{folder}</span>}
      {branch ? (
        <span className={styles.branch}>{branch}</span>
      ) : session.commit ? (
        <span className={`${styles.branch} ${styles.mono}`} title="Detached HEAD">
          {session.commit}
        </span>
      ) : null}
    </span>
  )
}

export function StatusFact({ session, now }: { session: OverviewSession; now: number }) {
  const quiet = quietText(session, now)
  const ended = isEnded(session.status)
  const endedAt = session.endedAt ?? session.statusSince
  return (
    <span className={styles.statusCell}>
      <span className={styles.statusLine}>
        <span className={`${styles.dot} ${styles[dotKey(session)]}`} aria-hidden="true" />
        <span className={styles.statusText}>{statusLabel(session)}</span>
        {session.status === 'failed' && session.exitCode !== undefined && (
          <span className={styles.duration}>exit {session.exitCode}</span>
        )}
        {ended ? (
          <span className={`${styles.duration} ${styles.endedAgo}`} title={formatClock(endedAt, now)}>
            {session.status === 'not-running' ? 'last active' : 'ended'} {formatAgo(endedAt, now)}
          </span>
        ) : (
          <span className={styles.duration}>{formatDuration(now - session.statusSince, { compact: true })}</span>
        )}
      </span>
      {quiet && (
        <span
          className={styles.quiet}
          title={session.lastOutputAt > 0 ? `Last output at ${formatClock(session.lastOutputAt, now)}` : undefined}
        >
          {quiet}
        </span>
      )}
    </span>
  )
}

export function SessionRow({
  session,
  now,
  showProvider,
  onOpenDetails,
  onJumpToAgent,
  selected,
  hideFolder
}: SessionItemProps & { hideFolder?: boolean }) {
  return (
    <li className={`${styles.row} ${selected ? styles.selected : ''}`}>
      <button
        type="button"
        className={styles.rowMain}
        data-session-trigger={session.agentId}
        onClick={(e) => onOpenDetails(session.agentId, e.currentTarget)}
        aria-label={`Details of ${session.name}`}
      >
        <span className={`${styles.cell} ${styles.colName}`} title={session.name}>
          {session.name}
        </span>
        <span className={`${styles.cell} ${styles.colAgent}`}>{agentLabel(session, showProvider)}</span>
        <span className={`${styles.cell} ${styles.colWhere}`}>
          <Where session={session} hideFolder={hideFolder} />
        </span>
        <span className={`${styles.cell} ${styles.colStatus}`}>
          <StatusFact session={session} now={now} />
        </span>
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
