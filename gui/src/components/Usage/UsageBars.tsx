import type { UsagePeriod, UsageSlot } from '@shared/usage/types'
import { USAGE_PROVIDERS } from '@shared/usage/types'
import styles from './UsageBars.module.css'

const slotTotal = (s: UsageSlot): number => USAGE_PROVIDERS.reduce((sum, p) => sum + (s.tokensByProvider[p] ?? 0), 0)

/** The slot with the most tokens, `null` when nothing was used. */
export function peakSlot(slots: UsageSlot[]): UsageSlot | null {
  let best: UsageSlot | null = null
  for (const s of slots) if (slotTotal(s) > (best ? slotTotal(best) : 0)) best = s
  return best
}

function describe(period: UsagePeriod, slots: UsageSlot[]): string {
  const unit = period === 'today' ? 'hour' : 'day'
  const peak = peakSlot(slots)
  const span = period === 'today' ? 'today' : period === 'week' ? 'this week' : 'this month'
  if (!peak) return `Tokens per ${unit} ${span}: no usage`
  return `Tokens per ${unit} ${span}, peak ${period === 'today' ? `${peak.label}:00` : peak.label}`
}

/** Stacked columns (Claude, Codex) per slot: hours today, days this week or month. */
export function UsageBars({ period, slots, tall = false }: { period: UsagePeriod; slots: UsageSlot[]; tall?: boolean }) {
  const max = Math.max(0, ...slots.map(slotTotal))
  const peak = peakSlot(slots)
  const first = slots[0]?.label
  const last = slots[slots.length - 1]?.label

  return (
    <div className={styles.wrap}>
      <div className={styles.chart} data-period={period} data-tall={tall} role="img" aria-label={describe(period, slots)}>
        {slots.map((s) => {
          const total = slotTotal(s)
          return (
            <div key={s.start} className={styles.col}>
              {total > 0 && (
                <div className={styles.stack} style={{ height: `${(total / max) * 100}%` }}>
                  {USAGE_PROVIDERS.map((p) =>
                    s.tokensByProvider[p] > 0 ? (
                      <span key={p} className={styles.seg} data-provider={p} style={{ flexGrow: s.tokensByProvider[p] }} />
                    ) : null
                  )}
                </div>
              )}
            </div>
          )
        })}
      </div>
      {period === 'week' ? (
        <div className={styles.weekLabels} aria-hidden="true">
          {slots.map((s) => (
            <span key={s.start}>{s.label}</span>
          ))}
        </div>
      ) : (
        <div className={styles.caption} aria-hidden="true">
          <span>{first}</span>
          {peak && <span>Peak {period === 'today' ? `${peak.label}:00` : `day ${peak.label}`}</span>}
          <span>{last}</span>
        </div>
      )}
    </div>
  )
}
