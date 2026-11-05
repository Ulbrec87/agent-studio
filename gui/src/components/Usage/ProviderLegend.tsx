import { USAGE_PROVIDERS, type UsageSummary } from '@shared/usage/types'
import { formatCompactTokens, formatUsd } from './format'
import { useUsageSettings } from './UsageSettingsContext'
import { ProviderDot } from './ProviderDot'
import styles from './ProviderLegend.module.css'

export const PROVIDER_NAMES = { claude: 'Claude', codex: 'Codex' } as const

/** One row per provider: tokens and API-equivalent cost of the period. */
export function ProviderLegend({ summary }: { summary: UsageSummary }) {
  const { showCost } = useUsageSettings()
  return (
    <ul className={styles.providers}>
      {USAGE_PROVIDERS.map((id) => {
        const p = summary.byProvider.find((x) => x.provider === id)
        const used = !!p && p.totalTokens > 0
        return (
          <li key={id} className={styles.provider}>
            <ProviderDot provider={id} />
            <span className={styles.providerName}>{PROVIDER_NAMES[id]}</span>
            {used ? (
              <>
                <span className={styles.providerTokens}>{formatCompactTokens(p.totalTokens)}</span>
                {showCost && (
                  <span className={styles.providerCost}>
                    {p.costPartial ? '≥' : ''}
                    {formatUsd(p.costUsd)}
                  </span>
                )}
              </>
            ) : (
              <span className={styles.providerNone}>no usage</span>
            )}
          </li>
        )
      })}
    </ul>
  )
}
