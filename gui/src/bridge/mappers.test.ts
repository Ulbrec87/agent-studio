import { describe, expect, it } from 'vitest'
import { mapPreflight } from './mappers'
import type { ProviderPreflight } from './generated'

const missing = { ok: false, path: null, version: null, error: 'CLI not found' }
const installed = { ok: true, path: 'C:\\tools\\agent.exe', version: '1.0', error: null }

describe('provider preflight selection', () => {
  it.each(['codex', 'opencode'] as const)('accepts an installed %s when Claude is missing', (provider) => {
    const entries: ProviderPreflight[] = [
      { provider: 'claude', result: missing },
      { provider, result: installed }
    ]
    expect(mapPreflight(entries).ok).toBe(true)
    expect(mapPreflight(entries).claudePath).toBe(installed.path)
    expect(mapPreflight(entries, 'claude').ok).toBe(false)
  })

  it('does not substitute another provider for an explicitly requested one', () => {
    expect(mapPreflight([{ provider: 'codex', result: installed }], 'claude').ok).toBe(false)
  })

  it('reports failure when all providers are missing or the response is empty', () => {
    expect(mapPreflight([{ provider: 'claude', result: missing }]).ok).toBe(false)
    expect(mapPreflight([]).ok).toBe(false)
  })
})
