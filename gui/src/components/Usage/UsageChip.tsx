import { forwardRef } from 'react'
import { limitSeverity } from '@shared/usage/limits'
import type { UsageState } from '@shared/usage/types'
import { Tooltip } from '../Tooltip'
import { useCoarseNow } from '../../hooks/useUsage'
import { useUsageSettings } from './UsageSettingsContext'
import { chipWindows, isStale, sourceLabel, type TimedWindow } from './model'
import styles from './UsageChip.module.css'

interface UsageChipProps {
  state: UsageState | null
  open: boolean
  onClick: () => void
}

function Meter({ label, timed, now }: { label: string; timed: TimedWindow; now: number }) {
  const percent = Math.round(timed.window.usedPercent)
  const stale = isStale(timed.at, now)
  return (
    <span className={styles.meter} data-testid={`chip-${label}`} data-severity={limitSeverity(percent)} data-stale={stale}>
      <span className={styles.meterLabel}>{label}</span>
      <span className={styles.track} aria-hidden="true">
        <span className={styles.fill} style={{ width: `${percent}%` }} />
      </span>
      <span className={styles.value}>{percent}%</span>
    </span>
  )
}

/** The header's usage button: two mini meters (5h, week), or plain "Usage" when no limits are known. */
export const UsageChip = forwardRef<HTMLButtonElement, UsageChipProps>(function UsageChip({ state, open, onClick }, ref) {
  const now = useCoarseNow()
  const { chipProvider } = useUsageSettings()
  const { session, weekly } = chipWindows(state, chipProvider)
  const high = [session, weekly].some((t) => t && limitSeverity(t.window.usedPercent) === 'high')

  const spoken = [
    session ? `session ${Math.round(session.window.usedPercent)}% used` : null,
    weekly ? `weekly ${Math.round(weekly.window.usedPercent)}% used` : null
  ].filter(Boolean)
  const label = spoken.length ? `Usage and limits: ${spoken.join(', ')}` : 'Usage and limits'

  const sources = [
    session ? `5h: ${sourceLabel(session.window)}` : null,
    weekly ? `Weekly: ${sourceLabel(weekly.window)}` : null
  ].filter(Boolean)

  return (
    <Tooltip content={sources.length ? sources.join(' · ') : 'Usage and limits'} disabled={open}>
      <button
        ref={ref}
        type="button"
        className={styles.chip}
        data-high={high}
        aria-haspopup="dialog"
        aria-expanded={open}
        aria-label={label}
        onClick={onClick}
      >
        {session || weekly ? (
          <>
            {session && <Meter label="5h" timed={session} now={now} />}
            {weekly && <Meter label="wk" timed={weekly} now={now} />}
          </>
        ) : (
          'Usage'
        )}
      </button>
    </Tooltip>
  )
})
