// Files and the editor: Hydra's `FileSystemService` methods on top of `FsReadDir` / `FsReadFile` / `FsWriteFile` / `FsSearch`,
// plus a polling replacement for Hydra's recursive `fs.watch` (the daemon has no watcher, only `FsMtime`).
import type { FsDirEntry, FsReadFileResult, FsSearchResult, FsWatchEventPayload } from '@shared/types'
import type { BridgeContext, MethodGroup } from './context'

/** Names Hydra's tree never shows (`IGNORED_NAMES` in Hydra's FileSystemService). */
const IGNORED_NAMES = new Set([
  'node_modules', '.git', '.next', '.nuxt', 'dist', 'build', 'out', '.cache', '.turbo', '.vercel', '.svelte-kit', '__pycache__',
  '.DS_Store', 'Thumbs.db', 'coverage', '.nyc_output', '.parcel-cache'
])

const BINARY_EXTENSIONS = new Set([
  '.png', '.jpg', '.jpeg', '.gif', '.bmp', '.ico', '.webp', '.avif', '.mp3', '.mp4', '.wav', '.ogg', '.webm', '.avi', '.mov',
  '.zip', '.tar', '.gz', '.bz2', '.7z', '.rar', '.woff', '.woff2', '.ttf', '.otf', '.eot', '.exe', '.dll', '.so', '.dylib',
  '.pdf', '.doc', '.docx', '.xls', '.xlsx', '.sqlite', '.db'
])

export interface ProtoFsEntry {
  name: string
  path: string
  isDir: boolean
}

export type ProtoFileContent =
  | { kind: 'text'; text: string; truncated: boolean; size: number; mtimeMs: number }
  | { kind: 'binary'; size: number }
  | { kind: 'tooLarge'; size: number }

export interface ProtoSearchHit {
  path: string
  name: string
  score: number
}

function isBinaryName(name: string): boolean {
  const dot = name.lastIndexOf('.')
  return dot !== -1 && BINARY_EXTENSIONS.has(name.slice(dot).toLowerCase())
}

/** Hydra's directory listing rules: no ignored names, no dot entries (except `.env.example`), folders first. */
export function mapDirEntries(entries: ProtoFsEntry[]): FsDirEntry[] {
  return entries
    .filter((e) => !IGNORED_NAMES.has(e.name) && !(e.name.startsWith('.') && e.name !== '.env.example'))
    .map((e) => ({ name: e.name, isDirectory: e.isDir }))
}

/** `FileContent` -> Hydra's `{ content, path }`; binary and oversized files are errors in Hydra too. */
export function mapFileContent(content: ProtoFileContent, path: string): FsReadFileResult {
  switch (content.kind) {
    case 'text':
      return { content: content.text, path }
    case 'binary':
      throw new Error('Cannot open binary files')
    case 'tooLarge':
      throw new Error(`File too large (${(content.size / 1024 / 1024).toFixed(1)} MB, max 1 MB)`)
    default:
      throw new Error('Unreadable file')
  }
}

/** Search hits, minus what Hydra's search skips (ignored folders, dot folders, binary files). */
export function mapSearchHits(hits: ProtoSearchHit[], max: number): FsSearchResult[] {
  const out: FsSearchResult[] = []
  for (const hit of hits) {
    const parts = hit.path.split('/')
    if (parts.some((p) => IGNORED_NAMES.has(p) || p.startsWith('.')) || isBinaryName(hit.name)) continue
    out.push({ path: hit.path, name: hit.name, isDirectory: false })
    if (out.length >= max) break
  }
  return out
}

/** Hydra paths are `'.'`-rooted and `/`-separated; the daemon wants `''` for the root. */
export function toDaemonPath(path: string): string {
  return path === '.' || path === './' ? '' : path.replace(/^\.\//, '')
}

// ── polling watcher ─────────────────────────────────────────────────────────

export interface FsWatcherOptions {
  /** Poll period in ms (default 1000). 0 disables the timer (tests call `poll()`). */
  pollMs?: number
  /** Most paths watched per agent (default 64); the least recently registered are dropped first. */
  maxPaths?: number
  /** How many mtimes are asked at once (default 4). */
  concurrency?: number
}

interface WatchState {
  agentId: string
  /** project-relative path (`'.'` is the root) -> kind and last seen mtime (`null`: missing). Insertion order = age. */
  paths: Map<string, Entry>
}

interface Entry {
  dir: boolean
  mtime: number | null
  /** The baseline is known (a poll before that would report the first reading as a change). */
  ready: boolean
}

/** The key of a path: Hydra's `'.'` for the root, no leading `./`. */
const canonical = (path: string): string => toDaemonPath(path) || '.'

/**
 * Hydra emits `onFsWatchEvent({ agentId, eventType, path })` whenever anything under the project changes, and the renderer
 * simply reloads on every event. The daemon has no watcher, so this polls `FsMtime` once a second for what the renderer is
 * actually showing: the project root and every directory it listed (a directory's mtime moves when an entry is created,
 * deleted or renamed -> `rename`) and every file it opened (-> `change`). Only agents between `watchDir` and `unwatchDir`
 * (the editor panel being open) are polled, at most `maxPaths` paths each. A change inside a directory the tree never
 * listed is therefore not seen, which is fine: nobody shows it.
 */
export function createFsWatcher(
  deps: {
    projectOf(agentId: string): Promise<string>
    mtime(projectDir: string, path: string): Promise<number>
    emit(payload: FsWatchEventPayload): void
  },
  options: FsWatcherOptions = {}
) {
  const pollMs = options.pollMs ?? 1000
  const maxPaths = options.maxPaths ?? 64
  const concurrency = Math.max(1, options.concurrency ?? 4)
  const watches = new Map<string, WatchState>()
  let timer: ReturnType<typeof setInterval> | null = null
  let polling: Promise<void> | null = null

  const ensureTimer = (): void => {
    if (timer || pollMs <= 0 || watches.size === 0) return
    timer = setInterval(() => void poll(), pollMs)
  }
  const stopTimer = (): void => {
    if (timer && watches.size === 0) {
      clearInterval(timer)
      timer = null
    }
  }

  const readMtime = async (agentId: string, path: string): Promise<number | null> => {
    try {
      return await deps.mtime(await deps.projectOf(agentId), toDaemonPath(path))
    } catch {
      return null
    }
  }

  async function pollOnce(): Promise<void> {
    const jobs: Array<() => Promise<void>> = []
    for (const watch of watches.values()) {
      for (const [path, seen] of [...watch.paths]) {
        jobs.push(async () => {
          const now = await readMtime(watch.agentId, path)
          if (watches.get(watch.agentId) !== watch || watch.paths.get(path) !== seen) return
          if (!seen.ready || now === seen.mtime) return
          seen.mtime = now
          // A directory whose entries changed, or a file that appeared or disappeared, is a "rename"; a rewritten file is a "change".
          const eventType = seen.dir || now === null ? 'rename' : 'change'
          deps.emit({ agentId: watch.agentId, eventType, path })
        })
      }
    }
    for (let i = 0; i < jobs.length; i += concurrency) await Promise.all(jobs.slice(i, i + concurrency).map((job) => job()))
  }

  /** One poll round (a round already running is joined, so slow answers never pile up). */
  function poll(): Promise<void> {
    polling ??= pollOnce().finally(() => {
      polling = null
    })
    return polling
  }

  async function register(agentId: string, path: string, dir: boolean, mtime: number | undefined): Promise<void> {
    const key = canonical(path)
    const watch = watches.get(agentId)
    if (!watch || watch.paths.has(key)) return
    // Claim the slot first (so a second read does not register twice), then learn the baseline; polls skip it until then.
    const entry: Entry = { dir, mtime: mtime ?? null, ready: mtime !== undefined }
    watch.paths.set(key, entry)
    while (watch.paths.size > maxPaths) {
      // Drop the oldest path; the root stays (it is always shown) and so does the one just added.
      const oldest = [...watch.paths.keys()].find((k) => k !== '.' && k !== key)
      if (oldest === undefined) break
      watch.paths.delete(oldest)
    }
    if (mtime === undefined) {
      entry.mtime = await readMtime(agentId, key)
      entry.ready = true
    }
  }

  return {
    watch(agentId: string): void {
      watches.set(agentId, { agentId, paths: new Map() })
      // The root is always shown. Its baseline is read right away; a failure leaves `null` and a later success reports a change.
      void register(agentId, '.', true, undefined)
      ensureTimer()
    },
    unwatch(agentId: string): void {
      watches.delete(agentId)
      stopTimer()
    },
    isWatching: (agentId: string): boolean => watches.has(agentId),
    /** Start following a path the renderer just read. `mtime` is the baseline when the read already told it (files). */
    async register(agentId: string, path: string, dir: boolean, mtime?: number): Promise<void> {
      await register(agentId, path, dir, mtime)
    },
    /** A path the renderer wrote itself: its new mtime is not a change worth reporting. */
    async acknowledge(agentId: string, path: string): Promise<void> {
      const entry = watches.get(agentId)?.paths.get(canonical(path))
      if (entry) entry.mtime = await readMtime(agentId, path)
    },
    poll,
    dispose(): void {
      watches.clear()
      stopTimer()
    }
  }
}

export type FsWatcher = ReturnType<typeof createFsWatcher>

// ── methods ─────────────────────────────────────────────────────────────────

export interface FilesDeps {
  ctx: BridgeContext
  /** The project directory of an agent (cached; asks the daemon when unknown). */
  projectOf(agentId: string): Promise<string>
  watcher?: FsWatcherOptions
}

export function createFileMethods({ ctx, projectOf, watcher: watcherOptions }: FilesDeps): MethodGroup & { watcher: FsWatcher } {
  const watcher = createFsWatcher(
    {
      projectOf,
      mtime: (projectDir, path) => ctx.call<number>('fs_mtime', { project_dir: projectDir, path }, 'number'),
      emit: (payload) => ctx.emit('onFsWatchEvent', payload)
    },
    watcherOptions
  )

  const methods: MethodGroup['methods'] = {
    readDir: (async (agentId: string, dirPath: string) => {
      const project_dir = await projectOf(agentId)
      const path = toDaemonPath(dirPath)
      // The baseline is taken before the listing, so a change in between shows up at the next poll instead of being lost.
      if (watcher.isWatching(agentId)) await watcher.register(agentId, dirPath, true)
      const entries = await ctx.call<ProtoFsEntry[]>('fs_read_dir', { project_dir, path, show_hidden: true }, 'fs_entries')
      return mapDirEntries(entries)
    }) as never,
    readFile: (async (agentId: string, filePath: string) => {
      const project_dir = await projectOf(agentId)
      const content = await ctx.call<ProtoFileContent>('fs_read_file', { project_dir, path: toDaemonPath(filePath) }, 'file_content')
      const result = mapFileContent(content, filePath)
      if (content.kind === 'text' && watcher.isWatching(agentId)) void watcher.register(agentId, filePath, false, content.mtimeMs)
      return result
    }) as never,
    writeFile: (async (agentId: string, filePath: string, content: string) => {
      const project_dir = await projectOf(agentId)
      await ctx.call('fs_write_file', { project_dir, path: toDaemonPath(filePath), content }, 'ok')
      await watcher.acknowledge(agentId, filePath)
      return true
    }) as never,
    searchFiles: (async (agentId: string, query: string, maxResults?: number) => {
      if (!query.trim()) return []
      const max = Math.min(500, Math.max(1, maxResults ?? 100))
      const project_dir = await projectOf(agentId)
      // Over-fetch: the daemon only knows .gitignore, Hydra's own ignore list is applied here.
      const hits = await ctx.call<ProtoSearchHit[]>('fs_search', { project_dir, query, limit: Math.min(1000, max * 4) }, 'search_hits')
      return mapSearchHits(hits, max)
    }) as never,
    watchDir: ((agentId: string) => watcher.watch(agentId)) as never,
    unwatchDir: ((agentId: string) => watcher.unwatch(agentId)) as never
  }
  return { methods, watcher, dispose: () => watcher.dispose() }
}

export const FILE_METHODS = ['readDir', 'readFile', 'writeFile', 'searchFiles', 'watchDir', 'unwatchDir'] as const
