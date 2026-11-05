// Adapted from agent-lookout (MIT, © 2026 Idris Olubisi): src/dashboard/components/panels/SessionPanel.tsx, LastMessage.tsx, TokensFact.tsx

import { useCallback, useEffect, useRef, useState, type ReactNode } from 'react'
import { isEnded } from '@shared/overview/status'
import type { AgentDetails, OverviewSession } from '@shared/overview/types'
import { formatAgo, formatClock, formatCount, formatDuration } from './format'
import { reasonLabel } from './NeedsYouPanel'
import { agentLabel, branchOf, dotKey, quietText, repoOf, statusLabel } from './sessionFacts'
import styles from './SessionDetails.module.css'
import rowStyles from './Sessions.module.css'

/** How often the transcript is re-read while the drawer is open and visible. */
export const DETAILS_REFRESH_MS = 2000

export interface SessionDetailsProps {
  agentId: string
  /** The session in the latest snapshot; `undefined` once the agent is gone. */
  session: OverviewSession | undefined
  now: number
  onClose: () => void
  onJumpToAgent: (agentId: string) => void
  /** The events of this session. Nothing is rendered when absent. */
  ownEvents?: ReactNode
  /** Filled by Step 9: this session's row of the timeline. Nothing is rendered when absent. */
  timelineRow?: ReactNode
  /** How often this session waited on the user in the last hour (the Waited on you window). Nothing is shown when `count` is 0 or absent. */
  waited?: { count: number; ms: number; from: number }
}

function Fact({ label, children }: { label: string; children: ReactNode }) {
  return (
    <>
      <dt className={styles.label}>{label}</dt>
      <dd className={styles.value}>{children}</dd>
    </>
  )
}

function useAgentDetails(agentId: string, active: boolean): AgentDetails | null {
  const [details, setDetails] = useState<AgentDetails | null>(null)

  useEffect(() => {
    if (!active) return undefined
    let cancelled = false
    const load = () => {
      if (typeof window.hydra?.getOverviewAgentDetails !== 'function') return
      window.hydra
        .getOverviewAgentDetails(agentId)
        .then((next) => {
          // `null` means the daemon does not know the agent (any more): keep what we had.
          if (!cancelled && next) setDetails(next)
        })
        .catch(() => { /* keep the last details */ })
    }
    const tick = () => {
      if (document.visibilityState !== 'hidden') load()
    }
    load()
    const timer = setInterval(tick, DETAILS_REFRESH_MS)
    document.addEventListener('visibilitychange', tick)
    return () => {
      cancelled = true
      clearInterval(timer)
      document.removeEventListener('visibilitychange', tick)
    }
  }, [agentId, active])

  return details
}

export function SessionDetails({
  agentId,
  session,
  now,
  onClose,
  onJumpToAgent,
  ownEvents,
  timelineRow,
  waited
}: SessionDetailsProps) {
  // Keep the last known data when the agent disappears from the snapshot.
  const [known, setKnown] = useState<OverviewSession | undefined>(session)
  useEffect(() => {
    if (session) setKnown(session)
  }, [session])
  const shown = session ?? known
  const gone = session === undefined

  const details = useAgentDetails(agentId, !gone)
  const dialogRef = useRef<HTMLElement>(null)
  const [copied, setCopied] = useState(false)
  const [busy, setBusy] = useState(false)
  const [actionError, setActionError] = useState<string | null>(null)

  useEffect(() => {
    dialogRef.current?.focus()
  }, [])

  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        e.preventDefault()
        onClose()
      }
    }
    document.addEventListener('keydown', onKeyDown)
    return () => document.removeEventListener('keydown', onKeyDown)
  }, [onClose])

  const copyPath = useCallback(() => {
    if (!shown) return
    navigator.clipboard
      ?.writeText(shown.projectDir)
      .then(() => {
        setCopied(true)
        setTimeout(() => setCopied(false), 1500)
      })
      .catch(() => { /* clipboard unavailable */ })
  }, [shown])

  const runAction = async (action: 'kill' | 'restart') => {
    setBusy(true)
    setActionError(null)
    try {
      if (action === 'kill') await window.hydra.killAgent(agentId)
      else await window.hydra.restartAgent(agentId)
    } catch (err) {
      setActionError(err instanceof Error ? err.message : String(err))
    } finally {
      setBusy(false)
    }
  }

  if (!shown) return null

  const branch = branchOf(shown)
  const quiet = quietText(shown, now)
  const open = !isEnded(shown.status)
  const message = details?.lastMessage

  return (
    <aside
      ref={dialogRef}
      className={styles.drawer}
      role="dialog"
      aria-label={`Details of ${shown.name}`}
      tabIndex={-1}
    >
      <header className={styles.header}>
        <h3 className={styles.name} title={shown.name}>
          {shown.name}
        </h3>
        <button type="button" className={styles.close} onClick={onClose} aria-label="Close details">
          ×
        </button>
      </header>

      <div className={styles.body}>
        {gone && (
          <p className={styles.gone} role="status">
            This agent is no longer running
          </p>
        )}

        <dl className={styles.facts}>
          <Fact label="Status">
            <span className={styles.statusLine}>
              <span className={`${rowStyles.dot} ${rowStyles[dotKey(shown)]}`} aria-hidden="true" />
              {statusLabel(shown)}
            </span>
            <span className={styles.sub}>
              {isEnded(shown.status) ? (
                <>
                  {shown.status === 'not-running' ? 'Last active' : 'Ended at'}{' '}
                  {formatClock(shown.endedAt ?? shown.statusSince, now)} (
                  {formatAgo(shown.endedAt ?? shown.statusSince, now)})
                </>
              ) : (
                <>
                  since {formatClock(shown.statusSince, now)} ({formatDuration(now - shown.statusSince)})
                </>
              )}
            </span>
          </Fact>

          {shown.exitCode !== undefined && (shown.status === 'failed' || shown.status === 'finished') && (
            <Fact label="Exit code">{shown.exitCode}</Fact>
          )}

          {shown.status === 'needs-you' && (
            <Fact label="Waiting">
              {reasonLabel(shown.waitingReason)}
              {shown.waitingText && <span className={styles.sub}>{shown.waitingText}</span>}
            </Fact>
          )}

          {quiet && (
            <Fact label="Quiet">
              {quiet}
              {shown.lastOutputAt > 0 && (
                <span className={styles.sub}>last output at {formatClock(shown.lastOutputAt, now)}</span>
              )}
            </Fact>
          )}

          <Fact label="Agent">{agentLabel(shown, true)}</Fact>

          <Fact label="Folder">
            <span className={styles.pathRow}>
              <code className={styles.path}>{shown.projectDir}</code>
              <button type="button" className={styles.copy} onClick={copyPath}>
                {copied ? 'Copied' : 'Copy'}
              </button>
            </span>
          </Fact>

          {repoOf(shown) && <Fact label="Repository">{repoOf(shown)}</Fact>}
          {branch && <Fact label="Branch">{branch}</Fact>}
          {shown.commit && (
            <Fact label="Commit">
              <code className={styles.inline}>{shown.commit}</code>
              <span className={styles.sub}>detached HEAD</span>
            </Fact>
          )}

          {waited && waited.count > 0 && (
            <Fact label="Waited on you">
              Waited {waited.count === 1 ? 'once' : `${waited.count} times`}, total {formatDuration(waited.ms)}
              <span className={styles.sub}>since {formatClock(waited.from, now)}</span>
            </Fact>
          )}

          <Fact label="Started">{shown.startedAt > 0 ? formatClock(shown.startedAt, now) : 'Never started'}</Fact>
          <Fact label="PID">{shown.pid ?? '—'}</Fact>

          {details?.tokens && (
            <Fact label="Tokens">
              {formatCount(details.tokens.input)} in · {formatCount(details.tokens.output)} out
              {details.tokens.cached !== undefined && (
                <span className={styles.sub}>of which {formatCount(details.tokens.cached)} cached</span>
              )}
            </Fact>
          )}
        </dl>

        <section className={styles.section} aria-label="Last message">
          <h4 className={styles.sectionTitle}>Last message</h4>
          {message === undefined ? (
            <p className={styles.muted}>Loading…</p>
          ) : 'text' in message ? (
            <>
              <pre className={styles.message}>{message.text}</pre>
              {message.truncated && <p className={styles.muted}>Showing the end of a longer message.</p>}
            </>
          ) : (
            <p className={styles.muted}>{message.unavailable}</p>
          )}
        </section>

        {ownEvents}
        {timelineRow}
      </div>

      <footer className={styles.actions}>
        {actionError && (
          <p className={styles.error} role="alert">
            {actionError}
          </p>
        )}
        <button type="button" className={rowStyles.jump} disabled={gone} onClick={() => onJumpToAgent(agentId)}>
          Jump
        </button>
        {open && (
          <button type="button" className={styles.secondary} disabled={gone || busy} onClick={() => runAction('kill')}>
            Stop
          </button>
        )}
        <button type="button" className={styles.secondary} disabled={gone || busy} onClick={() => runAction('restart')}>
          Restart
        </button>
      </footer>
    </aside>
  )
}
