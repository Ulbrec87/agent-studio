import type { ClaudeProfile } from '@shared/types'

interface ClaudeProfileOptionsProps {
  profiles: ClaudeProfile[]
  /** Currently mapped config dir, listed even if that profile no longer exists on disk. */
  selected?: string
}

/** `<option>`s for a Claude profile `<select>`; the default profile's value is `''`. */
export function ClaudeProfileOptions({ profiles, selected }: ClaudeProfileOptionsProps) {
  const missing = selected !== undefined && selected !== '' && !profiles.some((p) => p.configDir === selected)
  return (
    <>
      {profiles.map((profile) => (
        <option key={profile.configDir} value={profile.isDefault ? '' : profile.configDir}>
          {profile.label} ({profile.configDir})
        </option>
      ))}
      {missing && <option value={selected}>{selected} (missing)</option>}
    </>
  )
}
