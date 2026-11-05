// @vitest-environment jsdom
import { afterEach, describe, expect, it } from 'vitest'
import type { FsWatchEventPayload } from '@shared/types'
import { createFsWatcher, mapDirEntries, mapFileContent, mapSearchHits, toDaemonPath } from './filesApi'
import { makeBridge, tick } from './testKit'
import type { DaemonTransport } from './daemonApi'

const entry = (name: string, isDir = false) => ({ name, path: name, isDir, isSymlink: false, size: 1 })

describe('file mappers', () => {
  it('lists like Hydra: no ignored names or dot entries (except .env.example)', () => {
    const list = [entry('src', true), entry('node_modules', true), entry('.git', true), entry('.env'), entry('.env.example'), entry('a.ts'), entry('dist', true)]
    expect(mapDirEntries(list)).toEqual([{ name: 'src', isDirectory: true }, { name: '.env.example', isDirectory: false }, { name: 'a.ts', isDirectory: false }])
  })

  it('maps file content variants; binary and oversized files are errors', () => {
    expect(mapFileContent({ kind: 'text', text: 'hi', truncated: false, size: 2, mtimeMs: 1 }, 'a.txt')).toEqual({ content: 'hi', path: 'a.txt' })
    expect(() => mapFileContent({ kind: 'binary', size: 3 }, 'a.bin')).toThrow('Cannot open binary files')
    expect(() => mapFileContent({ kind: 'tooLarge', size: 3 * 1024 * 1024 }, 'big')).toThrow(/3\.0 MB, max 1 MB/)
  })

  it('search drops ignored folders, dot folders and binary files, and honours the limit', () => {
    const hit = (path: string) => ({ path, name: path.split('/').pop() ?? path, score: 1 })
    const hits = [hit('src/a.ts'), hit('node_modules/x/a.ts'), hit('.github/a.yml'), hit('logo.png'), hit('src/b.ts'), hit('src/c.ts')]
    expect(mapSearchHits(hits, 2).map((h) => h.path)).toEqual(['src/a.ts', 'src/b.ts'])
    expect(mapSearchHits(hits, 10)[0]).toEqual({ path: 'src/a.ts', name: 'a.ts', isDirectory: false })
  })

  it('paths: the renderer root "." is the daemon root ""', () => {
    expect([toDaemonPath('.'), toDaemonPath('./src'), toDaemonPath('src/a.ts')]).toEqual(['', 'src', 'src/a.ts'])
  })
})

let transport: DaemonTransport | null = null
afterEach(() => transport?.dispose())
async function setup(configure?: Parameters<typeof makeBridge>[0], options?: Parameters<typeof makeBridge>[1]) {
  const made = await makeBridge(configure, options)
  transport = made.transport
  return made
}

describe('file methods', () => {
  it('resolve the agent project (cached) and send daemon-relative paths', async () => {
    const { raw, api } = await setup((r) => {
      r.handlers.set('get_agent', (p) => ({ type: 'agent', payload: (p as { agent_id: string }).agent_id === 'late0001' ? { id: 'late0001', projectDir: '/late' } : null }))
      r.handlers.set('fs_read_dir', () => ({ type: 'fs_entries', payload: [entry('src', true), entry('.env')] }))
      r.handlers.set('fs_read_file', () => ({ type: 'file_content', payload: { kind: 'text', text: 'x', truncated: false, size: 1, mtimeMs: 5 } }))
      r.handlers.set('fs_write_file', () => ({ type: 'ok' }))
      r.handlers.set('fs_search', () => ({ type: 'search_hits', payload: [{ path: 'src/a.ts', name: 'a.ts', score: 3 }] }))
    })
    expect(await api.readDir('ddbc7683', '.')).toEqual([{ name: 'src', isDirectory: true }])
    expect(raw.of('fs_read_dir')[0].payload).toEqual({ project_dir: '/tmp/asg1-proj', path: '', show_hidden: true })
    expect(await api.readFile('ddbc7683', 'src/a.ts')).toEqual({ content: 'x', path: 'src/a.ts' })
    expect(await api.writeFile('ddbc7683', 'src/a.ts', 'new\ncontent')).toBe(true)
    expect(raw.of('fs_write_file')[0].payload).toEqual({ project_dir: '/tmp/asg1-proj', path: 'src/a.ts', content: 'new\ncontent' })
    expect(await api.searchFiles('ddbc7683', 'a', 20)).toEqual([{ path: 'src/a.ts', name: 'a.ts', isDirectory: false }])
    expect(raw.of('fs_search')[0].payload).toEqual({ project_dir: '/tmp/asg1-proj', query: 'a', limit: 80 })
    expect(await api.searchFiles('ddbc7683', '   ')).toEqual([])
    // Agents from the list are known without asking; others are asked once and remembered.
    expect(raw.of('get_agent')).toHaveLength(0)
    await api.readDir('late0001', 'src')
    await api.readDir('late0001', 'src')
    expect(raw.of('get_agent')).toHaveLength(1)
    expect(raw.of('fs_read_dir')[1].payload).toEqual({ project_dir: '/late', path: 'src', show_hidden: true })
    await expect(api.readDir('nobody', '.')).rejects.toThrow('Agent nobody not found')
  })

  it('daemon errors (traversal, directory) reach the renderer as Errors', async () => {
    const { api } = await setup((r) => {
      r.handlers.set('fs_write_file', () => Promise.reject('path is outside the project: ../x'))
    })
    await expect(api.writeFile('ddbc7683', '../x', 'y')).rejects.toThrow('outside the project')
  })

  it('watchDir/readDir/readFile feed the polling watcher and onFsWatchEvent reports changes', async () => {
    const mtimes = new Map<string, number>([['', 100], ['src', 200], ['src/a.ts', 300]])
    const { api } = await setup(
      (r) => {
        r.handlers.set('get_agent', () => ({ type: 'agent', payload: { id: 'ddbc7683', projectDir: '/proj' } }))
        r.handlers.set('fs_mtime', (p) => {
          const m = mtimes.get((p as { path: string }).path)
          return m === undefined ? Promise.reject('missing') : { type: 'number', payload: m }
        })
        r.handlers.set('fs_read_dir', () => ({ type: 'fs_entries', payload: [] }))
        r.handlers.set('fs_read_file', (p) => ({
          type: 'file_content',
          payload: { kind: 'text', text: '', truncated: false, size: 0, mtimeMs: mtimes.get((p as { path: string }).path) ?? 0 }
        }))
      },
      { fsPollMs: 15 }
    )
    const events: FsWatchEventPayload[] = []
    api.onFsWatchEvent((e) => events.push(e))
    api.watchDir('ddbc7683')
    await api.readDir('ddbc7683', 'src')
    await api.readFile('ddbc7683', 'src/a.ts')
    await tick(50)
    expect(events).toEqual([])
    mtimes.set('src/a.ts', 301)
    await tick(50)
    expect(events).toEqual([{ agentId: 'ddbc7683', eventType: 'change', path: 'src/a.ts' }])
    mtimes.set('src', 201)
    await tick(50)
    expect(events[1]).toEqual({ agentId: 'ddbc7683', eventType: 'rename', path: 'src' })
    api.unwatchDir('ddbc7683')
    mtimes.set('src', 202)
    await tick(50)
    expect(events).toHaveLength(2)
  })
})

describe('polling watcher', () => {
  function rig(mtimes: Map<string, number | null>, options = {}) {
    const events: FsWatchEventPayload[] = []
    const asked: string[] = []
    const watcher = createFsWatcher(
      {
        projectOf: async () => '/proj',
        mtime: async (_dir, path) => {
          asked.push(path)
          const m = mtimes.get(path)
          if (m === null || m === undefined) throw new Error('missing')
          return m
        },
        emit: (e) => events.push(e)
      },
      { pollMs: 0, ...options }
    )
    return { watcher, events, asked }
  }

  it('reports a changed directory as "rename" and a rewritten file as "change", once each', async () => {
    const mtimes = new Map<string, number | null>([['', 1], ['src', 2], ['src/a.ts', 3]])
    const { watcher, events } = rig(mtimes)
    watcher.watch('a1')
    await watcher.register('a1', 'src', true)
    await watcher.register('a1', 'src/a.ts', false, 3)
    await watcher.poll()
    expect(events).toEqual([])
    mtimes.set('src/a.ts', 4)
    await watcher.poll()
    expect(events).toEqual([{ agentId: 'a1', eventType: 'change', path: 'src/a.ts' }])
    mtimes.set('src', 5)
    mtimes.set('', 6)
    await watcher.poll()
    expect(events.slice(1)).toEqual(expect.arrayContaining([{ agentId: 'a1', eventType: 'rename', path: 'src' }, { agentId: 'a1', eventType: 'rename', path: '.' }]))
    const count = events.length
    await watcher.poll()
    expect(events).toHaveLength(count)
  })

  it('a deleted file is a "rename" and comes back as another one; our own save is acknowledged silently', async () => {
    const mtimes = new Map<string, number | null>([['', 1], ['f.txt', 10]])
    const { watcher, events } = rig(mtimes)
    watcher.watch('a1')
    await watcher.register('a1', 'f.txt', false, 10)
    await watcher.poll()
    mtimes.set('f.txt', null)
    await watcher.poll()
    expect(events).toEqual([{ agentId: 'a1', eventType: 'rename', path: 'f.txt' }])
    mtimes.set('f.txt', 11)
    await watcher.poll()
    expect(events).toHaveLength(2)
    mtimes.set('f.txt', 12)
    await watcher.acknowledge('a1', 'f.txt')
    await watcher.poll()
    expect(events).toHaveLength(2)
  })

  it('does not poll after unwatch, and a path registered before its baseline is known is not reported', async () => {
    const mtimes = new Map<string, number | null>([['', 1], ['x', 5]])
    const { watcher, events, asked } = rig(mtimes)
    watcher.watch('a1')
    const pending = watcher.register('a1', 'x', true) // baseline in flight
    await watcher.poll()
    await pending
    expect(events).toEqual([])
    watcher.unwatch('a1')
    asked.length = 0
    mtimes.set('x', 6)
    await watcher.poll()
    expect(asked).toEqual([])
    expect(events).toEqual([])
  })

  it('keeps at most maxPaths per agent (the root and the newest stay) and never overlaps polls', async () => {
    const mtimes = new Map<string, number | null>()
    for (let i = 0; i < 6; i++) mtimes.set(`d${i}`, i)
    mtimes.set('', 0)
    const { watcher, asked } = rig(mtimes, { maxPaths: 3 })
    watcher.watch('a1')
    await tick()
    for (let i = 0; i < 6; i++) await watcher.register('a1', `d${i}`, true)
    asked.length = 0
    await Promise.all([watcher.poll(), watcher.poll()])
    expect(asked.sort()).toEqual(['', 'd4', 'd5'])
  })

  it('a daemon that cannot be reached is not a change', async () => {
    const mtimes = new Map<string, number | null>([['', 1]])
    const { watcher, events } = rig(mtimes)
    watcher.watch('a1')
    await tick()
    mtimes.set('', null)
    await watcher.poll()
    expect(events).toHaveLength(1) // a vanished root is a rename; the point is that nothing throws
    mtimes.set('', 1)
    await watcher.poll()
    expect(events).toHaveLength(2)
  })
})
