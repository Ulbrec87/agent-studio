import type { UsageProvider } from '@shared/usage/types'
import styles from './ProviderDot.module.css'

/** The provider's colour dot (Claude: terracotta, Codex: blue). */
export function ProviderDot({ provider }: { provider: UsageProvider }) {
  return <span className={styles.dot} data-provider={provider} aria-hidden="true" />
}
