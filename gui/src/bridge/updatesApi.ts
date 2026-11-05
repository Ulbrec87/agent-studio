// App updates. Agent Studio never replaces itself: the daemon asks GitHub Releases (`GetUpdateState`, `CheckForUpdates`, event
// `UpdateAvailable`) and knows how the app was installed. Hydra's `AppUpdateState` is filled from the daemon's `UpdateState` with
// `canAutoInstall: false`, so the UI offers "Download" (opens the release asset) or the brew hint instead of "Install & Restart".
import type { AppUpdateState, UpdateInstallMethod } from '@shared/types'
import type { BridgeContext, MethodGroup } from './context'
import { isOpenableUrl, tauriOpenUrl } from './nativeApi'

export const UPDATE_METHODS = ['getUpdateState', 'checkForUpdates', 'downloadUpdate', 'installUpdateAndRestart', 'runBrewUpgrade', 'openUpdateDownload'] as const
export const UPDATE_CHANNELS = ['onUpdateStateChange'] as const

/** The daemon's `UpdateState` (Rust `as_core::updates`, camelCase; `status` and `installMethod` are snake_case words). */
export interface DaemonUpdateState {
  enabled: boolean
  currentVersion: string
  status: 'idle' | 'up_to_date' | 'available' | 'no_releases' | 'offline' | 'rate_limited' | 'failed'
  checkedAtMs: number | null
  latest: { version: string; tag: string; url: string; notes: string; assetUrl: string | null } | null
  installMethod: 'cargo' | 'homebrew' | 'winget' | 'scoop' | 'chocolatey' | 'direct' | 'unknown'
  hint: string
}

const STATUS_ERRORS: Partial<Record<DaemonUpdateState['status'], string>> = {
  offline: 'Could not reach GitHub (offline?)',
  rate_limited: 'GitHub rate limit reached, try again later',
  failed: 'Update check failed'
}

/** Hydra knows `brew`, `direct` and `unknown`; the other package managers only differ in the hint text. */
export function mapInstallMethod(method: DaemonUpdateState['installMethod']): UpdateInstallMethod {
  return method === 'homebrew' ? 'brew' : method === 'direct' ? 'direct' : 'unknown'
}

export function mapUpdateState(state: DaemonUpdateState, platform: string, checking = false): AppUpdateState {
  const available = state.status === 'available' && state.latest !== null
  return {
    supported: true,
    platform,
    checking,
    available,
    downloaded: false,
    downloading: false,
    currentVersion: state.currentVersion,
    latestVersion: state.latest?.version ?? null,
    releaseDate: null,
    releaseNotes: available ? state.latest?.notes || null : null,
    error: STATUS_ERRORS[state.status] ?? null,
    canAutoInstall: false,
    installMethod: mapInstallMethod(state.installMethod),
    downloadUrl: available ? (state.latest?.assetUrl ?? state.latest?.url ?? null) : null,
    releaseUrl: available ? (state.latest?.url ?? null) : null,
    upgradeHint: available ? state.hint || null : null
  }
}

export interface UpdateDeps {
  ctx: BridgeContext
  /** Test seam; the default is the Tauri opener (or `window.open` in a browser). */
  openUrl?: (url: string) => Promise<void>
}

export function createUpdateMethods({ ctx, openUrl = tauriOpenUrl }: UpdateDeps): MethodGroup {
  let last: DaemonUpdateState | undefined
  let checking = false
  const platform = (): string => ctx.appInfo()?.platform ?? 'linux'
  const current = (): AppUpdateState | undefined => (last ? mapUpdateState(last, platform(), checking) : undefined)
  const load = async (): Promise<DaemonUpdateState> => {
    last = await ctx.call<DaemonUpdateState>('get_update_state', undefined, 'update_state')
    return last
  }
  const unsupported = async (): Promise<string> => {
    const state = last ?? (await load())
    return state.hint ? `Agent Studio does not update itself. ${state.hint}` : 'Agent Studio does not update itself: download the new version from the releases page.'
  }

  return {
    methods: {
      getUpdateState: (async () => mapUpdateState(await load(), platform(), checking)) as never,
      checkForUpdates: (async () => {
        checking = true
        const before = current()
        if (before) ctx.emit('onUpdateStateChange', before)
        try {
          last = await ctx.call<DaemonUpdateState>('check_for_updates', undefined, 'update_state')
        } finally {
          checking = false
        }
        const next = mapUpdateState(last, platform())
        ctx.emit('onUpdateStateChange', next)
        return next
      }) as never,
      // `canAutoInstall` is false, so the UI never offers these; if something calls them anyway the answer says why nothing happened.
      downloadUpdate: (async () => {
        const state = last ?? (await load())
        return { ...mapUpdateState(state, platform()), error: await unsupported() }
      }) as never,
      installUpdateAndRestart: (async () => false) as never,
      runBrewUpgrade: (async () => ({ ok: false, error: await unsupported() })) as never,
      openUpdateDownload: (async () => {
        const state = last ?? (await load())
        const url = state.latest?.assetUrl ?? state.latest?.url
        if (!url) return { ok: false, error: 'No update is available.' }
        if (!isOpenableUrl(url)) return { ok: false, error: 'The download address is not a web link.' }
        try {
          await openUrl(new URL(url).toString())
          return { ok: true }
        } catch (err) {
          return { ok: false, error: err instanceof Error ? err.message : String(err) }
        }
      }) as never
    },
    onEvent(type, payload) {
      if (type !== 'update_available') return false
      last = payload as unknown as DaemonUpdateState
      ctx.emit('onUpdateStateChange', mapUpdateState(last, platform()))
      return true
    }
  }
}
