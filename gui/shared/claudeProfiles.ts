import type { ClaudeProfile } from './types'

function normalizeDir(dir: string): string {
  return dir.replace(/\\/g, '/').replace(/\/+$/, '')
}

/**
 * The Claude config dir mapped to `projectDir`, matching the project itself or
 * any parent; the longest mapped path wins. `''` means the default profile and
 * `null` means no mapping, so the agent inherits Agent Studio's own environment.
 */
export function resolveClaudeProfile(
  projectDir: string,
  profileByProject: Record<string, string> | undefined
): string | null {
  if (!profileByProject) return null
  const target = normalizeDir(projectDir)
  let best: { length: number; configDir: string } | null = null
  for (const [mappedDir, configDir] of Object.entries(profileByProject)) {
    const prefix = normalizeDir(mappedDir)
    if (!prefix) continue
    const matches = target === prefix || target.startsWith(`${prefix}/`)
    if (matches && (!best || prefix.length > best.length)) {
      best = { length: prefix.length, configDir }
    }
  }
  return best ? best.configDir : null
}

/** Returns a copy of the mapping with `projectDir` set to `configDir`, or removed when `null`. */
export function withClaudeProfile(
  profileByProject: Record<string, string>,
  projectDir: string,
  configDir: string | null
): Record<string, string> {
  const next = { ...profileByProject }
  if (configDir === null) delete next[projectDir]
  else next[projectDir] = configDir
  return next
}

/** Display name for a mapped config dir: the profile label, or the raw path if it no longer exists. */
export function claudeProfileLabel(configDir: string, profiles: ClaudeProfile[]): string {
  if (configDir === '') return profiles.find((profile) => profile.isDefault)?.label ?? 'default'
  return profiles.find((profile) => profile.configDir === configDir)?.label ?? configDir
}
