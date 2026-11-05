import { readFileSync } from 'node:fs'
import { describe, expect, it } from 'vitest'
import { WIRED_CHANNELS, WIRED_METHODS } from './daemonApi'

describe('bridge README', () => {
  it('lists exactly the methods the daemon layer answers as "wired"', () => {
    const readme = readFileSync(new URL('./README.md', import.meta.url), 'utf8')
    const rows = [...readme.matchAll(/^\| `(\w+)` \| (\w+) \| (G\d) \| (\w+) \|$/gm)]
    expect(rows.length).toBeGreaterThan(100)
    const wired = new Set(rows.filter((r) => r[4] === 'wired').map((r) => r[1]))
    const implemented = new Set<string>([...WIRED_METHODS, ...WIRED_CHANNELS])
    // Agent Studio extensions are documented below the table, not in it.
    implemented.delete('getDaemonStatus')
    expect([...wired].sort()).toEqual([...implemented].sort())
    // G1 rows that are still stubs must be the documented no-ops.
    const g1Stubs = rows.filter((r) => r[3] === 'G1' && r[4] === 'stub').map((r) => r[1]).sort()
    expect(g1Stubs).toEqual(['confirmQuit', 'logEvent', 'onConfirmQuit', 'onKeybindingsChange', 'quitBackground'])
  })
})
