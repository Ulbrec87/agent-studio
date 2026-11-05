// Open-in, folder suggestions and the native pieces (folder dialog, opening URLs, clipboard image).
// "Open in editor/app/path" are daemon requests (`OpenIn`, `ListOpenTargets`): the daemon starts the editor. The dialog and the
// URL opener are Tauri plugins and only exist in the desktop app; in a browser (dev bridge) they degrade as documented below.
import type { DirSuggestion, EditorId } from '@shared/types'
import type { BridgeContext, MethodGroup } from './context'
import { isTauri } from './tauriTransport'

/** The daemon's `OpenTarget` names for Hydra's `EditorId`s. */
const TARGET_FOR_EDITOR: Record<EditorId, string> = {
  vscode: 'vscode',
  cursor: 'cursor',
  windsurf: 'windsurf',
  antigravity: 'antigravity',
  zed: 'zed',
  finder: 'files',
  terminal: 'terminal'
}

/** The reverse map; the daemon's `editor` ($EDITOR in a terminal) has no Hydra counterpart and is left out. */
export function editorIdsFromTargets(options: Array<{ target: string }>): EditorId[] {
  const wanted = new Map(Object.entries(TARGET_FOR_EDITOR).map(([id, target]) => [target, id as EditorId]))
  return options.flatMap((o) => wanted.get(o.target) ?? [])
}

/** Hydra opens only web and mail links. */
export function isOpenableUrl(url: unknown): url is string {
  if (typeof url !== 'string') return false
  try {
    return ['http:', 'https:', 'mailto:'].includes(new URL(url).protocol)
  } catch {
    return false
  }
}

/** `data:image/png;base64,...` -> a Blob; null when it is not an image data URL. */
export function dataUrlToBlob(dataUrl: string): Blob | null {
  const match = /^data:(image\/[\w.+-]+)(;base64)?,(.*)$/s.exec(dataUrl)
  if (!match) return null
  const [, type, base64, body] = match
  const binary = base64 ? atob(body) : decodeURIComponent(body)
  const bytes = new Uint8Array(binary.length)
  for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i)
  return new Blob([bytes], { type })
}

export interface NativeDeps {
  ctx: BridgeContext
  /** Test seams; the defaults use the Tauri plugins and the browser. */
  pickDirectory?: () => Promise<string | null>
  openUrl?: (url: string) => Promise<void>
  writeImage?: (blob: Blob) => Promise<void>
}

async function tauriPickDirectory(): Promise<string | null> {
  if (!isTauri()) return null // Browser (dev bridge): there is no native dialog; the New Agent dialog still takes a typed path.
  const { open } = await import('@tauri-apps/plugin-dialog')
  const picked = await open({ directory: true, multiple: false, title: 'Select Project Directory' })
  return typeof picked === 'string' ? picked : null
}

async function tauriOpenUrl(url: string): Promise<void> {
  if (isTauri()) {
    const { openUrl } = await import('@tauri-apps/plugin-opener')
    await openUrl(url)
  } else {
    window.open(url, '_blank', 'noopener,noreferrer')
  }
}

// The webview's async clipboard handles images on WebView2, WKWebView and current WebKitGTK; no clipboard plugin is needed.
async function webWriteImage(blob: Blob): Promise<void> {
  await navigator.clipboard.write([new ClipboardItem({ [blob.type]: blob })])
}

export function createNativeMethods({ ctx, pickDirectory = tauriPickDirectory, openUrl = tauriOpenUrl, writeImage = webWriteImage }: NativeDeps): MethodGroup {
  const openIn = async (target: string, path: string): Promise<boolean> => {
    try {
      await ctx.call('open_in', { target, path }, 'ok')
      return true
    } catch {
      return false
    }
  }
  /** Hydra: try the editor, fall back to the default application for the path. */
  const openInApp = async (editor: EditorId, path: string): Promise<boolean> => {
    const target = TARGET_FOR_EDITOR[editor]
    if (!target) return openIn('files', path)
    if (target === 'files') return openIn('files', path)
    return (await openIn(target, path)) || (await openIn('files', path))
  }

  return {
    methods: {
      getInstalledEditors: (async () =>
        editorIdsFromTargets(await ctx.call<Array<{ target: string }>>('list_open_targets', undefined, 'open_targets'))) as never,
      openInEditor: ((dir: string) => openInApp('vscode', dir)) as never,
      openInApp: ((editor: EditorId, dir: string) => openInApp(editor, dir)) as never,
      openPath: ((path: string) => openIn('files', path)) as never,
      openExternal: (async (url: unknown) => {
        if (!isOpenableUrl(url)) return false
        try {
          await openUrl(new URL(url).toString())
          return true
        } catch {
          return false
        }
      }) as never,
      selectDirectory: (() => pickDirectory().catch(() => null)) as never,
      listDirectories: (async (query: string) => {
        if (typeof query !== 'string' || !query) return []
        return ctx.call<DirSuggestion[]>('fs_list_dirs', { query }, 'dir_suggestions')
      }) as never,
      writeClipboardImage: (async (dataUrl: string) => {
        const blob = dataUrlToBlob(String(dataUrl))
        if (!blob) return false
        try {
          await writeImage(blob)
          return true
        } catch (err) {
          console.warn('[bridge] clipboard image failed', err)
          return false
        }
      }) as never
    }
  }
}

export const NATIVE_METHODS = [
  'getInstalledEditors', 'openInEditor', 'openInApp', 'openPath', 'openExternal', 'selectDirectory', 'listDirectories', 'writeClipboardImage'
] as const
