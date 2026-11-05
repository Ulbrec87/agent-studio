import { limitSeverity } from '@shared/usage/limits'
import type { UsageState } from '@shared/usage/types'
import { formatAge, formatResets } from './format'
import { buildLimitGroups, lowWindows, profileProblem, type LimitGroup, type TimedWindow } from './model'
import { useUsageSettings } from './UsageSettingsContext'
import { ProviderDot } from './ProviderDot'
import styles from './LimitRows.module.css'

export type LimitsFilter = 'low' | 'all'

interface LimitRowsProps {
  state: UsageState
  filter: LimitsFilter
  now: number
}

/** The plan-limit groups of the popover, one per provider account. */
export function LimitRows({ state, filter, now }: LimitRowsProps) {
  const settings = useUsageSettings()
  const groups = buildLimitGroups(state)
  if (groups.length === 0) return <p className={styles.quiet}>No plan limits yet.</p>

  if (filter === 'low') {
    const low = lowWindows(groups, settings)
    const visible = groups
      .map((g) => ({ ...g, windows: g.windows.filter((t) => low.has(t.window)) }))
      .filter((g) => g.windows.length > 0)
    if (visible.length === 0) return <p className={styles.quiet}>No plan window is near its limit.</p>
    return (
      <div className={styles.groups}>
        {visible.map((g) => (
          <Group key={g.key} group={g} now={now} compact />
        ))}
      </div>
    )
  }

  return (
    <div className={styles.groups}>
      {groups.map((g) => (
        <Group key={g.key} group={g} now={now} />
      ))}
    </div>
  )
}

function Group({ group, now, compact = false }: { group: LimitGroup; now: number; compact?: boolean }) {
  const { profile } = group
  const problem = profile ? profileProblem(profile) : null
  const dimmed = profile !== null && profile.status !== 'ok'
  return (
    <section className={styles.group} aria-label={`${group.name} limits`}>
      <header className={styles.groupHeader}>
        <ProviderDot provider={group.provider} />
        <span className={styles.groupName}>{group.name}</span>
        {group.subtitle && <span className={styles.groupSub}>{group.subtitle}</span>}
        <span className={styles.groupSource}>
          {group.provider === 'codex' && group.codexAt !== null && (
            <>
              from session log · {formatAge(now - group.codexAt)}
              <span className={styles.pill}>ESTIMATED</span>
            </>
          )}
          {profile?.status === 'ok' && 'live'}
        </span>
      </header>
      {!compact && problem && (
        <div className={styles.problem} data-status={profile?.status}>
          <p className={styles.problemTitle}>{problem.title}</p>
          {problem.hint && <p className={styles.problemHint}>{problem.hint}</p>}
        </div>
      )}
      {group.windows.map((t) => (
        <Row key={t.window.id} timed={t} now={now} dimmed={dimmed} />
      ))}
      {!compact && dimmed && profile!.windows.length > 0 && profile!.fetchedAt !== null && (
        <p className={styles.note}>Showing the last values from {formatAge(now - profile!.fetchedAt)} ago.</p>
      )}
    </section>
  )
}

function Row({ timed, now, dimmed }: { timed: TimedWindow; now: number; dimmed: boolean }) {
  const { window: w } = timed
  const percent = Math.round(w.usedPercent)
  return (
    <div className={styles.row} data-dimmed={dimmed} data-severity={limitSeverity(w.usedPercent)}>
      <div className={styles.rowTop}>
        <span className={styles.rowLabel}>{w.label}</span>
        <span className={styles.rowUsed}>{percent}% used</span>
        <span className={styles.rowReset}>{formatResets(w.resetsAt, now)}</span>
      </div>
      <div
        className={styles.bar}
        role="progressbar"
        aria-label={w.label}
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={percent}
      >
        <div className={styles.barFill} style={{ width: `${percent}%` }} />
      </div>
    </div>
  )
}
