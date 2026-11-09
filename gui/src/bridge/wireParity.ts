// Compile-time checks that the front end's own types (Hydra's `@shared/...`) and the wire types generated from Rust
// (`./generated`, ts-rs) stay the same shape where the bridge passes a daemon answer through unchanged. `npm run typecheck`
// fails on drift. Example of what this catches: `UsageSlot.tokensByProvider` was an array on the wire while the charts read
// `tokensByProvider.claude`, so every bar had height 0.
//
// Add a pair here for every type the bridge forwards without a mapper.

import type { UsageSlot as SharedUsageSlot, UsageSummary as SharedUsageSummary } from '@shared/usage/types'
import type { UsageReport as SharedUsageReport } from '@shared/usage/report'
import type { UsageSlot, UsageSummary, UsageReport } from './generated'

type Same<A, B> = [A] extends [B] ? ([B] extends [A] ? true : false) : false
type Assert<T extends true> = T

export type WireParity = [
  Assert<Same<SharedUsageSlot, UsageSlot>>,
  Assert<Same<SharedUsageSummary, UsageSummary>>,
  Assert<Same<SharedUsageReport, UsageReport>>
]
