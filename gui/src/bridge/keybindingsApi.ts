// Key bindings of the desktop app: Hydra's `mod+x` accelerators (`gui/shared/keybindings.ts`) with the user's overrides in
// `<data>/gui-keybindings.json` .
//
// The daemon only reads the raw file (`GetGuiKeybindings`: path, parsed JSON array, error text, change stamp; tolerant of a missing
// or corrupt file) and creates a template on request (`CreateGuiKeybindingsFile`); merging with the defaults and validating command ids
// is TypeScript, like Hydra's `KeybindingStore`. Changes are found by polling the stamp once a second (the same cadence as Hydra's
// `watchFile`), which works through the Tauri shell and the browser dev bridge alike. Unlike Hydra the file is not created at start
// and not rewritten to append missing defaults: it appears when the user opens it from the app, and the user's own edits stay untouched.
import type { GuiKeybindingsFile } from './generated'
import { DEFAULT_KEYBINDINGS, isHydraCommandId, mergeKeybindingsWithDefaults, type KeybindingRule } from '@shared/keybindings'
import type { BridgeContext, MethodGroup } from './context'

export const KEYBINDING_METHODS = ['getKeybindings'] as const
export const KEYBINDING_CHANNELS = ['onKeybindingsChange'] as const
export const KEYBINDINGS_FILE = 'gui-keybindings.json'
const POLL_MS = 1000

/** Answer of the daemon's `GetGuiKeybindings` (generated from Rust `as_core::gui_keybindings`). */
export type { GuiKeybindingsFile }

/** Hydra's `parseRule`: keep `{ command, keys }` entries with a known command and non-empty keys, drop everything else. */
export function parseRules(raw: unknown[]): KeybindingRule[] {
  return raw.flatMap((item): KeybindingRule[] => {
    if (!item || typeof item !== 'object') return []
    const { command, keys } = item as { command?: unknown; keys?: unknown }
    if (typeof command !== 'string' || typeof keys !== 'string' || !isHydraCommandId(command)) return []
    const trimmed = keys.trim()
    return trimmed ? [{ command, keys: trimmed }] : []
  })
}

/** The rules the renderer uses: the defaults with the user's overrides on top. A missing or unusable file means the defaults. */
export function resolveKeybindings(file: Pick<GuiKeybindingsFile, 'rules'> | null): KeybindingRule[] {
  return mergeKeybindingsWithDefaults(parseRules(file?.rules ?? []))
}

export function templateContent(): string {
  return JSON.stringify(DEFAULT_KEYBINDINGS, null, 2)
}

export interface KeybindingDeps {
  ctx: BridgeContext
  /** Poll period in ms; 0 turns the watcher off (tests drive `poll()` themselves). */
  pollMs?: number
}

export function createKeybindingMethods({ ctx, pollMs = POLL_MS }: KeybindingDeps): MethodGroup & {
  poll(): Promise<void>
  /** Create the file from the defaults when it does not exist (before the user opens it). */
  ensureFile(): Promise<void>
} {
  let lastStamp: string | null = null
  let lastJson: string | null = null
  let timer: ReturnType<typeof setInterval> | undefined

  const read = (): Promise<GuiKeybindingsFile> => ctx.call<GuiKeybindingsFile>('get_gui_keybindings', undefined, 'gui_keybindings')

  const remember = (file: GuiKeybindingsFile): KeybindingRule[] => {
    const rules = resolveKeybindings(file)
    lastStamp = file.stamp
    lastJson = JSON.stringify(rules)
    return rules
  }

  const poll = async (): Promise<void> => {
    let file: GuiKeybindingsFile
    try {
      file = await read()
    } catch {
      return // daemon gone for a moment: the next tick (or the reconnect) tries again
    }
    if (file.stamp === lastStamp) return
    const first = lastJson === null
    const before = lastJson
    const rules = remember(file)
    if (!first && lastJson !== before) ctx.emit('onKeybindingsChange', rules)
  }

  const stop = (): void => {
    if (timer !== undefined) clearInterval(timer)
    timer = undefined
  }

  return {
    methods: {
      getKeybindings: (async () => {
        try {
          return remember(await read())
        } catch {
          return DEFAULT_KEYBINDINGS.map((r) => ({ ...r }))
        }
      }) as never
    },
    poll,
    async ensureFile() {
      await ctx.call('create_gui_keybindings_file', { content: templateContent() }, 'flag')
      await poll()
    },
    onConnect() {
      lastStamp = null
      lastJson = null
      stop()
      void poll() // takes the baseline
      if (pollMs > 0) timer = setInterval(() => void poll(), pollMs)
    },
    onDisconnect: stop,
    dispose: stop
  }
}
