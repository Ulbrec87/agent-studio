// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from 'vitest'
import { dataUrlToBlob, editorIdsFromTargets, isOpenableUrl } from './nativeApi'
import { makeBridge } from './testKit'
import type { DaemonTransport } from './daemonApi'
import type { NativeDeps } from './nativeApi'

let transport: DaemonTransport | null = null
afterEach(() => transport?.dispose())
async function setup(configure?: Parameters<typeof makeBridge>[0], native?: Partial<Omit<NativeDeps, 'ctx'>>) {
  const made = await makeBridge(configure, { native })
  transport = made.transport
  return made
}

describe('native helpers', () => {
  it('maps open targets to Hydra editor ids (the $EDITOR target has none)', () => {
    expect(editorIdsFromTargets([{ target: 'vscode' }, { target: 'editor' }, { target: 'files' }, { target: 'terminal' }, { target: 'zed' }])).toEqual(['vscode', 'finder', 'terminal', 'zed'])
  })

  it('only web and mail links are opened', () => {
    expect(['https://a.b/c', 'http://x', 'mailto:a@b.c'].every(isOpenableUrl)).toBe(true)
    expect(['file:///etc/passwd', 'javascript:alert(1)', 'not a url', 42, null].some(isOpenableUrl)).toBe(false)
  })

  it('decodes image data URLs', async () => {
    const blob = dataUrlToBlob('data:image/png;base64,iVBORw0KGgo=')
    expect(blob?.type).toBe('image/png')
    expect(blob?.size).toBe(8)
    expect(dataUrlToBlob('data:text/plain;base64,aGk=')).toBeNull()
    expect(dataUrlToBlob('nonsense')).toBeNull()
  })
})

describe('open-in and native methods', () => {
  it('lists the installed editors from the daemon targets', async () => {
    const { api } = await setup((r) =>
      r.handlers.set('list_open_targets', () => ({ type: 'open_targets', payload: [{ target: 'vscode', label: 'VS Code' }, { target: 'files', label: 'File manager' }] }))
    )
    expect(await api.getInstalledEditors()).toEqual(['vscode', 'finder'])
  })

  it('opens editors and falls back to the default application like Hydra', async () => {
    const { raw, api } = await setup((r) =>
      r.handlers.set('open_in', (p) => ((p as { target: string }).target === 'cursor' ? Promise.reject('cannot start cursor') : { type: 'ok' }))
    )
    expect(await api.openInEditor('/p')).toBe(true)
    expect(raw.of('open_in')[0].payload).toEqual({ target: 'vscode', path: '/p' })
    expect(await api.openInApp('finder', '/p')).toBe(true)
    expect(raw.of('open_in')[1].payload).toEqual({ target: 'files', path: '/p' })
    expect(await api.openInApp('cursor', '/p')).toBe(true) // cursor failed, files worked
    expect(raw.of('open_in').slice(2).map((r) => (r.payload as { target: string }).target)).toEqual(['cursor', 'files'])
    expect(await api.openPath('/home/u/config.json')).toBe(true)
    expect(raw.of('open_in').at(-1)?.payload).toEqual({ target: 'files', path: '/home/u/config.json' })
  })

  it('reports false when nothing could open the path', async () => {
    const { api } = await setup((r) => r.handlers.set('open_in', () => Promise.reject('xdg-open missing')))
    expect(await api.openPath('/x')).toBe(false)
    expect(await api.openInApp('zed', '/x')).toBe(false)
  })

  it('openExternal validates the URL and delegates to the opener', async () => {
    const openUrl = vi.fn().mockResolvedValue(undefined)
    const { api } = await setup(undefined, { openUrl })
    expect(await api.openExternal('https://example.com/a b')).toBe(true)
    expect(openUrl).toHaveBeenCalledWith('https://example.com/a%20b')
    expect(await api.openExternal('file:///etc/passwd')).toBe(false)
    expect(openUrl).toHaveBeenCalledTimes(1)
    openUrl.mockRejectedValueOnce(new Error('no browser'))
    expect(await api.openExternal('https://example.com')).toBe(false)
  })

  it('selectDirectory returns the picked folder or null (also when the dialog fails)', async () => {
    const pick = vi.fn().mockResolvedValueOnce('/home/u/proj').mockResolvedValueOnce(null).mockRejectedValueOnce(new Error('no portal'))
    const { api } = await setup(undefined, { pickDirectory: pick })
    expect(await api.selectDirectory()).toBe('/home/u/proj')
    expect(await api.selectDirectory()).toBeNull()
    expect(await api.selectDirectory()).toBeNull()
  })

  it('without Tauri (browser dev mode) selectDirectory is a documented no-op that returns null', async () => {
    const { api } = await setup()
    expect(await api.selectDirectory()).toBeNull()
  })

  it('listDirectories asks the daemon (empty queries never leave the page)', async () => {
    const suggestions = [{ path: '/home/u/proj', name: 'proj', isGitRepo: true }]
    const { raw, api } = await setup((r) => r.handlers.set('fs_list_dirs', () => ({ type: 'dir_suggestions', payload: suggestions })))
    expect(await api.listDirectories('/home/u/p')).toEqual(suggestions)
    expect(raw.of('fs_list_dirs')[0].payload).toEqual({ query: '/home/u/p' })
    expect(await api.listDirectories('')).toEqual([])
    expect(raw.of('fs_list_dirs')).toHaveLength(1)
  })

  it('writes an image to the clipboard, false for garbage or a refusing clipboard', async () => {
    const writeImage = vi.fn().mockResolvedValue(undefined)
    const { api } = await setup(undefined, { writeImage })
    expect(await api.writeClipboardImage('data:image/png;base64,iVBORw0KGgo=')).toBe(true)
    expect((writeImage.mock.calls[0][0] as Blob).type).toBe('image/png')
    expect(await api.writeClipboardImage('hello')).toBe(false)
    writeImage.mockRejectedValueOnce(new Error('denied'))
    expect(await api.writeClipboardImage('data:image/png;base64,iVBORw0KGgo=')).toBe(false)
  })
})
