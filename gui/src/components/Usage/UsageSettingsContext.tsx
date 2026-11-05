import { createContext, useContext, type ReactNode } from 'react'
import { DEFAULT_USAGE_SETTINGS, type UsageSettings } from '@shared/usage/settings'

const UsageSettingsContext = createContext<UsageSettings>(DEFAULT_USAGE_SETTINGS)

/** Gives the usage surfaces (chip, popover, Overview card) the user's usage settings. */
export function UsageSettingsProvider({ settings, children }: { settings: UsageSettings | undefined; children: ReactNode }) {
  return <UsageSettingsContext.Provider value={settings ?? DEFAULT_USAGE_SETTINGS}>{children}</UsageSettingsContext.Provider>
}

/** The usage settings; the defaults outside a provider. */
export function useUsageSettings(): UsageSettings {
  return useContext(UsageSettingsContext)
}
