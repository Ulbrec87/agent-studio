import { useEffect, useState } from 'react'
import type { ClaudeProfile } from '@shared/types'

/** Claude Code profiles (`~/.claude`, `~/.claude-<name>`) available on this machine. */
export function useClaudeProfiles(): ClaudeProfile[] {
  const [profiles, setProfiles] = useState<ClaudeProfile[]>([])

  useEffect(() => {
    if (typeof window.hydra.listClaudeProfiles !== 'function') return

    let cancelled = false
    window.hydra.listClaudeProfiles()
      .then((list) => { if (!cancelled) setProfiles(list) })
      .catch(() => { /* No profiles: the pickers stay hidden. */ })

    return () => {
      cancelled = true
    }
  }, [])

  return profiles
}
