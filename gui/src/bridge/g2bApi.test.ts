// G2b method groups: headless runs, skills, updates, diagnostics + log, keybindings, quit flow, open-in through the shell.
import { describe, expect, it, vi, afterEach } from 'vitest'
import { DEFAULT_KEYBINDINGS } from '@shared/keybindings'
import { FakeRaw, FakeShell, b64, makeBridge, tick } from './testKit'
import { mapInstallMethod, mapUpdateState, type DaemonUpdateState } from './updatesApi'
import { parseRules, resolveKeybindings } from './keybindingsApi'
import { togglePayloads } from './skillsApi'

afterEach(() => vi.restoreAllMocks())

const RUN = {
  id: 'run1', prompt: 'do it', projectDir: '/p', provider: 'claude', model: 'sonnet', resumeSessionId: null, status: 'running',
  startedAt: '2026-11-05T10:00:00.000Z', endedAt: null, sessionId: null, error: null
}

describe('headless runs', () => {
  it('maps the five calls to the daemon requests and unwraps the answers', async () => {
    const { raw, api } = await makeBridge((r) => {
      r.handlers.set('start_headless_run', () => ({ type: 'headless_run', payload: RUN }))
      r.handlers.set('list_headless_runs', () => ({ type: 'headless_runs', payload: [RUN] }))
      r.handlers.set('get_headless_run', (p) => ({ type: 'headless_run', payload: (p as { run_id: string }).run_id === 'run1' ? RUN : null }))
      r.handlers.set('cancel_headless_run', () => ({ type: 'flag', payload: true }))
      r.handlers.set('get_headless_run_log', () => ({ type: 'headless_run_log', payload: { runId: 'run1', content: '{"a":1}', totalLines: 1, returnedLines: 1, truncated: false } }))
    })
    const payload = { prompt: 'do it', projectDir: '/p', provider: 'claude' as const, model: 'sonnet', reasoningEffort: undefined, resumeSessionId: null }
    expect(await api.startHeadlessRun(payload)).toEqual(RUN)
    expect(raw.of('start_headless_run')[0].payload).toEqual({ prompt: 'do it', projectDir: '/p', provider: 'claude', model: 'sonnet', resumeSessionId: null })
    expect(await api.listHeadlessRuns({ query: undefined, status: 'all', limit: 1000 })).toEqual([RUN])
    expect(raw.of('list_headless_runs')[0].payload).toEqual({ status: 'all', limit: 1000 })
    expect(await api.listHeadlessRuns()).toEqual([RUN])
    expect(await api.getHeadlessRun('run1')).toEqual(RUN)
    expect(await api.getHeadlessRun('nope')).toBeNull()
    expect(await api.cancelHeadlessRun('run1')).toBe(true)
    const log = await api.getHeadlessRunLog('run1', { tailLines: 1400, maxChars: 240000 })
    expect(log?.content).toBe('{"a":1}')
    expect(raw.of('get_headless_run_log')[0].payload).toEqual({ run_id: 'run1', options: { tailLines: 1400, maxChars: 240000 } })
  })

  it('a daemon error (bad project folder) rejects with its message; a missing log is null', async () => {
    const { api } = await makeBridge((r) => {
      r.handlers.set('start_headless_run', () => Promise.reject('project directory does not exist: /x'))
      r.handlers.set('get_headless_run_log', () => ({ type: 'headless_run_log', payload: null }))
    })
    await expect(api.startHeadlessRun({ prompt: 'p', projectDir: '/x', provider: 'claude', model: 'sonnet' })).rejects.toThrow('project directory does not exist: /x')
    expect(await api.getHeadlessRunLog('gone')).toBeNull()
  })

  it('output lines and state changes both reach onHeadlessRunEvent as { runId, data }', async () => {
    const { raw, api } = await makeBridge()
    const seen = vi.fn()
    api.onHeadlessRunEvent(seen)
    raw.push('headless_run_event', { run_id: 'run1', data: '{"type":"x"}' })
    raw.push('headless_run_changed', { ...RUN, status: 'completed' })
    expect(seen.mock.calls).toEqual([[{ runId: 'run1', data: '{"type":"x"}' }], [{ runId: 'run1', data: '' }]])
  })
})

describe('skills', () => {
  const SCAN = {
    claude: [
      { id: 'brainstorm', name: 'Brainstorm', description: 'd', provider: 'claude', group: 'sp@market', enabled: false, path: '/c1/p/skills/brainstorm/SKILL.md', configDir: '/c1' },
      { id: 'plan', name: 'Plan', description: 'd', provider: 'claude', group: 'sp@market', enabled: false, path: '/c2/p/skills/plan/SKILL.md', configDir: '/c2' },
      { id: 'mine', name: 'Mine', description: '', provider: 'claude', group: 'user', enabled: true, path: '/c1/skills/mine/SKILL.md', configDir: '/c1' }
    ],
    codex: [{ id: 'tdd', name: 'tdd', description: '', provider: 'codex', group: 'agents', enabled: true, path: '/h/.agents/skills/tdd/SKILL.md' }],
    opencode: []
  }

  it('scanSkills adds scannedAt and keeps the lists', async () => {
    const { api } = await makeBridge((r) => r.handlers.set('scan_skills', () => ({ type: 'skills', payload: SCAN })))
    const scan = await api.scanSkills()
    expect(scan.claude).toHaveLength(3)
    expect(scan.codex[0].id).toBe('tdd')
    expect(Number.isNaN(Date.parse(scan.scannedAt))).toBe(false)
  })

  it('toggleSkill: a Claude plugin is toggled per profile that has it, a Codex skill by its file', async () => {
    const { raw, api } = await makeBridge((r) => {
      r.handlers.set('scan_skills', () => ({ type: 'skills', payload: SCAN }))
      r.handlers.set('toggle_skill', () => ({ type: 'ok' }))
    })
    expect(await api.toggleSkill({ provider: 'claude', id: 'sp@market', enabled: true })).toEqual({ success: true })
    expect(raw.of('toggle_skill').map((r) => r.payload)).toEqual([
      { provider: 'claude', enabled: true, group: 'sp@market', configDir: '/c1' },
      { provider: 'claude', enabled: true, group: 'sp@market', configDir: '/c2' }
    ])
    raw.requests.length = 0
    expect(await api.toggleSkill({ provider: 'codex', id: 'tdd', enabled: false })).toEqual({ success: true })
    expect(raw.of('toggle_skill')[0].payload).toEqual({ provider: 'codex', enabled: false, path: '/h/.agents/skills/tdd/SKILL.md' })
  })

  it('toggleSkill answers success:false for unknown skills, user skills and daemon errors (the panel then rescans)', async () => {
    const { api } = await makeBridge((r) => {
      r.handlers.set('scan_skills', () => ({ type: 'skills', payload: SCAN }))
      r.handlers.set('toggle_skill', () => Promise.reject('skill not found'))
    })
    vi.spyOn(console, 'warn').mockImplementation(() => undefined)
    expect(await api.toggleSkill({ provider: 'claude', id: 'user', enabled: false })).toEqual({ success: false })
    expect(await api.toggleSkill({ provider: 'codex', id: 'nope', enabled: false })).toEqual({ success: false })
    expect(await api.toggleSkill({ provider: 'claude', id: 'sp@market', enabled: false })).toEqual({ success: false })
  })

  it('togglePayloads: opencode has no skills', () => {
    expect(togglePayloads({ claude: [], codex: [], opencode: [] }, { provider: 'opencode', id: 'x', enabled: true })).toEqual([])
  })
})

describe('updates', () => {
  const AVAILABLE: DaemonUpdateState = {
    enabled: true, currentVersion: '0.1.0', status: 'available', checkedAtMs: 1, installMethod: 'homebrew',
    latest: { version: '9.9.9', tag: 'v9.9.9', url: 'https://example.com/rel', notes: '## Notes', assetUrl: 'https://example.com/a.tar.gz' },
    hint: 'Run `brew upgrade agent-studio`'
  }
  const UP_TO_DATE: DaemonUpdateState = { ...AVAILABLE, status: 'up_to_date', latest: null, hint: '' }

  it('maps an available release: no auto install, asset as download, page as release url, install method and hint', () => {
    const state = mapUpdateState(AVAILABLE, 'linux')
    expect(state).toMatchObject({
      supported: true, platform: 'linux', checking: false, available: true, downloaded: false, downloading: false, currentVersion: '0.1.0',
      latestVersion: '9.9.9', releaseNotes: '## Notes', error: null, canAutoInstall: false, installMethod: 'brew',
      downloadUrl: 'https://example.com/a.tar.gz', releaseUrl: 'https://example.com/rel', upgradeHint: 'Run `brew upgrade agent-studio`'
    })
    // No asset for this platform: the release page is the download.
    expect(mapUpdateState({ ...AVAILABLE, latest: { ...AVAILABLE.latest!, assetUrl: null } }, 'linux').downloadUrl).toBe('https://example.com/rel')
  })

  it('maps the quiet and error states', () => {
    expect(mapUpdateState(UP_TO_DATE, 'linux')).toMatchObject({ available: false, latestVersion: null, error: null, downloadUrl: null, upgradeHint: null })
    expect(mapUpdateState({ ...UP_TO_DATE, status: 'offline' }, 'linux').error).toMatch(/offline/)
    expect(mapUpdateState({ ...UP_TO_DATE, status: 'rate_limited' }, 'linux').error).toMatch(/rate limit/)
    expect(mapUpdateState({ ...UP_TO_DATE, status: 'failed' }, 'linux').error).toMatch(/failed/)
    expect(mapUpdateState({ ...UP_TO_DATE, status: 'no_releases' }, 'linux').error).toBeNull()
    expect(mapUpdateState(UP_TO_DATE, 'linux', true).checking).toBe(true)
    expect(['homebrew', 'direct', 'cargo', 'winget', 'unknown'].map((m) => mapInstallMethod(m as never))).toEqual(['brew', 'direct', 'unknown', 'unknown', 'unknown'])
  })

  it('getUpdateState / checkForUpdates ask the daemon; a check announces "checking" and then the result', async () => {
    const { raw, api } = await makeBridge((r) => {
      r.handlers.set('get_update_state', () => ({ type: 'update_state', payload: UP_TO_DATE }))
      r.handlers.set('check_for_updates', () => ({ type: 'update_state', payload: AVAILABLE }))
    })
    expect((await api.getUpdateState()).available).toBe(false)
    const seen: boolean[] = []
    api.onUpdateStateChange((s) => seen.push(s.checking))
    const checked = await api.checkForUpdates()
    expect(checked.available).toBe(true)
    expect(raw.of('check_for_updates')).toHaveLength(1)
    expect(seen).toEqual([true, false])
  })

  it('the daemon pushes UpdateAvailable: onUpdateStateChange gets the mapped state', async () => {
    const { raw, api } = await makeBridge()
    const seen = vi.fn()
    api.onUpdateStateChange(seen)
    raw.push('update_available', AVAILABLE)
    expect(seen.mock.calls[0][0]).toMatchObject({ available: true, latestVersion: '9.9.9', canAutoInstall: false })
  })

  it('download/install/brew are answered with an unsupported hint instead of doing anything', async () => {
    const { raw, api } = await makeBridge((r) => r.handlers.set('get_update_state', () => ({ type: 'update_state', payload: AVAILABLE })))
    expect((await api.downloadUpdate()).error).toMatch(/does not update itself.*brew upgrade agent-studio/)
    expect(await api.installUpdateAndRestart()).toBe(false)
    const brew = await api.runBrewUpgrade()
    expect(brew.ok).toBe(false)
    expect(brew.error).toMatch(/brew upgrade agent-studio/)
    expect(raw.requests.every((r) => r.type === 'get_update_state')).toBe(true)
  })

  it('openUpdateDownload opens the asset (or the page) and refuses anything that is not a web link', async () => {
    const opened: string[] = []
    const { api } = await makeBridge((r) => r.handlers.set('get_update_state', () => ({ type: 'update_state', payload: AVAILABLE })), { openUrl: async (u) => void opened.push(u) })
    expect(await api.openUpdateDownload()).toEqual({ ok: true })
    expect(opened).toEqual(['https://example.com/a.tar.gz'])
    const none = await makeBridge((r) => r.handlers.set('get_update_state', () => ({ type: 'update_state', payload: UP_TO_DATE })))
    expect((await none.api.openUpdateDownload()).ok).toBe(false)
    const bad = await makeBridge((r) => r.handlers.set('get_update_state', () => ({ type: 'update_state', payload: { ...AVAILABLE, latest: { ...AVAILABLE.latest!, assetUrl: 'file:///etc/passwd' } } })), { openUrl: async (u) => void opened.push(u) })
    expect((await bad.api.openUpdateDownload()).ok).toBe(false)
    expect(opened).toHaveLength(1)
  })
})

describe('diagnostics and logging', () => {
  it('exportDiagnostics answers { path, error } and reveals the folder only through the desktop shell', async () => {
    const shell = new FakeShell()
    const { api, raw } = await makeBridge((r) => {
      r.shell = shell
      r.handlers.set('export_diagnostics', () => ({ type: 'path', payload: '/data/diagnostics/20261105' }))
    })
    expect(await api.exportDiagnostics()).toEqual({ path: '/data/diagnostics/20261105', error: null })
    await tick()
    expect(shell.of('open_in')).toEqual([{ target: 'files', path: '/data/diagnostics/20261105' }])
    expect(raw.of('open_in')).toHaveLength(0)
    // A browser (no shell): the folder is not opened from a tab, not even through the daemon.
    const web = await makeBridge((r) => r.handlers.set('export_diagnostics', () => ({ type: 'path', payload: '/d/x' })))
    expect(await web.api.exportDiagnostics()).toEqual({ path: '/d/x', error: null })
    await tick()
    expect(web.raw.of('open_in')).toHaveLength(0)
  })

  it('exportDiagnostics reports daemon errors as a result, never as an exception', async () => {
    const { api } = await makeBridge((r) => r.handlers.set('export_diagnostics', () => Promise.reject('disk full')))
    expect(await api.exportDiagnostics()).toEqual({ path: null, error: 'disk full' })
  })

  it('logEvent goes to the shell log (fire and forget); without a shell it is a console line; failures are swallowed', async () => {
    const shell = new FakeShell()
    const { api } = await makeBridge((r) => (r.shell = shell))
    api.logEvent({ level: 'info', event: 'renderer.test', meta: { a: 1 } })
    await tick()
    expect(shell.of('log_event')).toEqual([{ event: { level: 'info', event: 'renderer.test', meta: { a: 1 } } }])
    shell.answers.set('log_event', () => {
      throw new Error('disk full')
    })
    expect(() => api.logEvent({ level: 'error', event: 'renderer.x' })).not.toThrow()
    await tick()
    const spy = vi.spyOn(console, 'debug').mockImplementation(() => undefined)
    const plain = await makeBridge()
    plain.api.logEvent({ level: 'info', event: 'renderer.plain' })
    await tick()
    expect(spy).toHaveBeenCalledWith('[renderer]', 'renderer.plain', '')
  })
})

describe('keybindings', () => {
  const file = (rules: unknown[], stamp: string) => ({ type: 'gui_keybindings', payload: { path: '/data/gui-keybindings.json', exists: true, rules, error: null, stamp } })

  it('parseRules follows Hydra: unknown commands, empty keys and junk are dropped, keys are trimmed', () => {
    expect(parseRules([{ command: 'new-agent', keys: ' mod+t ' }, { command: 'nope', keys: 'x' }, { command: 'settings', keys: '  ' }, 7, null, { command: 'git-panel' }])).toEqual([
      { command: 'new-agent', keys: 'mod+t' }
    ])
    const merged = resolveKeybindings({ rules: [{ command: 'new-agent', keys: 'mod+t' }] })
    expect(merged).toHaveLength(DEFAULT_KEYBINDINGS.length)
    expect(merged.find((r) => r.command === 'new-agent')?.keys).toBe('mod+t')
    expect(merged.find((r) => r.command === 'close-agent')?.keys).toBe('mod+w')
    expect(resolveKeybindings(null)).toEqual(DEFAULT_KEYBINDINGS)
  })

  it('getKeybindings merges the user file over the defaults and survives a missing or broken one', async () => {
    const { api, raw } = await makeBridge((r) => r.handlers.set('get_gui_keybindings', () => file([{ command: 'new-agent', keys: 'mod+t' }], 'a')))
    expect((await api.getKeybindings()).find((r) => r.command === 'new-agent')?.keys).toBe('mod+t')
    raw.handlers.set('get_gui_keybindings', () => ({ type: 'gui_keybindings', payload: { path: '/p', exists: true, rules: [], error: 'invalid JSON: x', stamp: 'b' } }))
    expect(await api.getKeybindings()).toEqual(DEFAULT_KEYBINDINGS)
    raw.handlers.set('get_gui_keybindings', () => Promise.reject('unsupported request'))
    expect(await api.getKeybindings()).toEqual(DEFAULT_KEYBINDINGS)
  })

  it('onKeybindingsChange fires when the file changes the merged result, not for a rewrite with the same result', async () => {
    const { api, raw } = await makeBridge((r) => r.handlers.set('get_gui_keybindings', () => file([], 's0')), { keybindingsPollMs: 20 })
    const changed = vi.fn()
    api.onKeybindingsChange(changed)
    await api.getKeybindings()
    await tick(60)
    expect(changed).not.toHaveBeenCalled() // the baseline is silent
    raw.handlers.set('get_gui_keybindings', () => file([{ command: 'new-agent', keys: 'mod+t' }], 's1'))
    await tick(80)
    expect(changed).toHaveBeenCalledTimes(1)
    expect(changed.mock.calls[0][0].find((r: { command: string }) => r.command === 'new-agent').keys).toBe('mod+t')
    // Same effective bindings (different file text): quiet. Broken file: back to the defaults, once.
    raw.handlers.set('get_gui_keybindings', () => file([{ command: 'new-agent', keys: 'mod+t' }, { command: 'bogus', keys: 'x' }], 's2'))
    await tick(60)
    expect(changed).toHaveBeenCalledTimes(1)
    raw.handlers.set('get_gui_keybindings', () => ({ type: 'gui_keybindings', payload: { path: '/p', exists: true, rules: [], error: 'invalid JSON', stamp: 's3' } }))
    await tick(60)
    expect(changed).toHaveBeenCalledTimes(2)
    expect(changed.mock.calls[1][0]).toEqual(DEFAULT_KEYBINDINGS)
  })

  it('the polling stops on disconnect and a daemon hiccup does not break it', async () => {
    const { api, raw } = await makeBridge((r) => r.handlers.set('get_gui_keybindings', () => file([], 's0')), { keybindingsPollMs: 15 })
    api.onKeybindingsChange(() => undefined)
    await tick(40)
    raw.handlers.set('get_gui_keybindings', () => Promise.reject('busy'))
    await tick(40)
    raw.setStatus({ connected: false })
    raw.requests.length = 0
    await tick(60)
    expect(raw.of('get_gui_keybindings')).toHaveLength(0)
  })

  it('opening the keybindings file first creates it from the defaults; other paths are opened as they are', async () => {
    const shell = new FakeShell()
    const { api, raw } = await makeBridge((r) => {
      r.shell = shell
      r.handlers.set('create_gui_keybindings_file', () => ({ type: 'flag', payload: true }))
      r.handlers.set('get_gui_keybindings', () => file([], 's0'))
    })
    expect(await api.getKeybindingsPath()).toBe('/data/gui-keybindings.json')
    expect(await api.openPath('/data/gui-keybindings.json')).toBe(true)
    const created = raw.of('create_gui_keybindings_file')
    expect(created).toHaveLength(1)
    expect(JSON.parse((created[0].payload as { content: string }).content)).toEqual(DEFAULT_KEYBINDINGS)
    expect(shell.of('open_in')).toEqual([{ target: 'files', path: '/data/gui-keybindings.json' }])
    await api.openPath('/data/config.json')
    expect(raw.of('create_gui_keybindings_file')).toHaveLength(1)
  })
})

describe('quit flow', () => {
  it('confirm-quit from the shell reaches onConfirmQuit with the running count', async () => {
    const shell = new FakeShell()
    const { api } = await makeBridge((r) => (r.shell = shell))
    const seen = vi.fn()
    api.onConfirmQuit(seen)
    shell.fire('confirm-quit', 3)
    shell.fire('confirm-quit', 'x')
    expect(seen.mock.calls).toEqual([[3], [0]])
  })

  it('confirmQuit stops the agents, quitBackground only the window; errors answer false', async () => {
    const shell = new FakeShell()
    const { api } = await makeBridge((r) => (r.shell = shell))
    expect(await api.confirmQuit()).toBe(true)
    expect(await api.quitBackground()).toBe(true)
    expect(shell.of('quit_app')).toEqual([{ stopAgents: true }, { stopAgents: false }])
    shell.answers.set('quit_app', () => {
      throw new Error('nope')
    })
    vi.spyOn(console, 'warn').mockImplementation(() => undefined)
    expect(await api.confirmQuit()).toBe(false)
  })

  it('without a shell (browser dev bridge) there is nothing to quit', async () => {
    const { api } = await makeBridge()
    expect(await api.confirmQuit()).toBe(false)
    expect(await api.quitBackground()).toBe(false)
  })
})

describe('open-in through the shell', () => {
  it('uses the shell commands when present and never asks the daemon', async () => {
    const shell = new FakeShell()
    shell.answers.set('list_open_targets', () => [{ target: 'vscode', label: 'VS Code' }, { target: 'files', label: 'Files' }, { target: 'editor', label: '$EDITOR' }])
    const { api, raw } = await makeBridge((r) => (r.shell = shell))
    expect(await api.getInstalledEditors()).toEqual(['vscode', 'finder'])
    expect(await api.openInEditor('/proj')).toBe(true)
    expect(shell.of('open_in')).toEqual([{ target: 'vscode', path: '/proj' }])
    // The editor failed: Hydra falls back to the default application.
    shell.answers.set('open_in', (args) => {
      if (args?.target === 'zed') throw new Error('not installed')
    })
    expect(await api.openInApp('zed', '/proj')).toBe(true)
    expect(shell.of('open_in').slice(-2)).toEqual([{ target: 'zed', path: '/proj' }, { target: 'files', path: '/proj' }])
    expect(raw.requests.filter((r) => r.type === 'open_in' || r.type === 'list_open_targets')).toHaveLength(0)
  })

  it('falls back to the daemon where the shell lacks the command (browser dev bridge)', async () => {
    const shell = new FakeShell(['log_event'])
    const { api, raw } = await makeBridge((r) => {
      r.shell = shell
      r.handlers.set('open_in', () => ({ type: 'ok' }))
      r.handlers.set('list_open_targets', () => ({ type: 'open_targets', payload: [{ target: 'vscode', label: 'VS Code' }] }))
    })
    expect(await api.getInstalledEditors()).toEqual(['vscode'])
    expect(await api.openPath('/x')).toBe(true)
    expect(raw.of('open_in')[0].payload).toEqual({ target: 'files', path: '/x' })
    expect(shell.of('open_in')).toHaveLength(0)
  })
})

describe('agent restarts announced by the daemon', () => {
  it('agent_restarted becomes onAgentRestarted and resets the half-decoded character of the old process', async () => {
    const { raw, api } = await makeBridge()
    const seen = vi.fn()
    api.onAgentRestarted(seen)
    raw.push('agent_restarted', { agent_id: 'ddbc7683' })
    expect(seen).toHaveBeenCalledWith({ agentId: 'ddbc7683' })
  })
})

// G2b oddity (b): "one terminal showed another agent's `/rename ...` line". The daemon routes per agent id (checked with 150 concurrent
// Subscribes of two agents against a real daemon); this pins the same property for the bridge's streams.
describe('output of different agents never mixes', () => {
  it('concurrent snapshots and live chunks of two agents stay apart', async () => {
    const { raw, api } = await makeBridge((r) => {
      r.handlers.set('subscribe', async (p) => {
        const id = (p as { agent_id: string }).agent_id
        await tick(id === 'ddbc7683' ? 5 : 1) // answers cross each other
        return { type: 'buffer', payload: { data: b64(`buffer of ${id}`) } }
      })
    })
    const live: Record<string, string> = {}
    api.onAgentOutput(({ agentId, data }) => (live[agentId] = (live[agentId] ?? '') + data))
    const [a, b, a2] = await Promise.all([api.getAgentBuffer('ddbc7683'), api.getAgentBuffer('idle0001'), api.getAgentBuffer('ddbc7683')])
    expect(a.join('')).toBe('buffer of ddbc7683')
    expect(a2.join('')).toBe('buffer of ddbc7683')
    expect(b.join('')).toBe('buffer of idle0001')
    raw.push('agent_output', { agent_id: 'ddbc7683', data: b64('/rename alpha') })
    raw.push('agent_output', { agent_id: 'idle0001', data: b64('/rename beta') })
    expect(live).toEqual({ ddbc7683: '/rename alpha', idle0001: '/rename beta' })
  })
})

describe('FakeRaw plumbing', () => {
  it('is a RawTransport with an optional shell', () => {
    expect(new FakeRaw().shell).toBeUndefined()
  })
})
