import { useCallback, useLayoutEffect, useRef, useState } from 'react'
import type { AppConfig, ThemeId } from '@shared/types'

export type AppearanceThemeId = ThemeId | 'modern-workspace-soft'
export const APPEARANCE_STORAGE_KEY = 'agentStudio.appearance:v1'
const SOFT = 'modern-workspace-soft'

function readOverride(): boolean {
  try {
    return localStorage.getItem(APPEARANCE_STORAGE_KEY) === SOFT
  } catch {
    return false
  }
}

/** A renderer-only appearance choice: Soft must never enter daemon configuration. */
export function useAppearance(
  backendTheme: ThemeId,
  updateConfig: (partial: Partial<AppConfig>) => Promise<unknown>
) {
  const [soft, setSoft] = useState(readOverride)
  const [error, setError] = useState<string | null>(null)
  const requestRef = useRef(0)
  const theme: AppearanceThemeId = soft ? SOFT : backendTheme

  useLayoutEffect(() => {
    document.documentElement.dataset.theme = theme
  }, [theme])

  const selectTheme = useCallback(async (next: AppearanceThemeId) => {
    const request = ++requestRef.current
    setError(null)
    if (next !== SOFT) {
      try {
        await updateConfig({ theme: next })
      } catch (err) {
        if (request === requestRef.current) {
          setError(err instanceof Error ? err.message : 'Could not save the color theme.')
        }
        return
      }
    }
    // A slower previous save must not overwrite a newer appearance selection.
    if (request !== requestRef.current) return
    setSoft(next === SOFT)
    try {
      if (next === SOFT) localStorage.setItem(APPEARANCE_STORAGE_KEY, SOFT)
      else localStorage.removeItem(APPEARANCE_STORAGE_KEY)
    } catch {
      // The selection still works for this window when storage is unavailable.
    }
  }, [updateConfig])

  return { theme, selectTheme, error }
}
