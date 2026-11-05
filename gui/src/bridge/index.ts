import { createBridge } from './bridge'
import { createHydraApi, type HydraAPI } from './hydraApi'
import { APP_INFO_REQUEST, selectRawTransport, selectTransport, type AppInfo, type RawTransport, type Transport } from './transport'
import { createMockTransport } from './mockTransport'

export type { HydraAPI } from './hydraApi'
export type { Transport, RawTransport, AppInfo } from './transport'

/** What the shell reports about itself (name, version, data dir, platform). Falls back to the mock's answer. */
export async function loadAppInfo(transport: Transport): Promise<AppInfo> {
  try {
    const timeout = new Promise<never>((_, reject) => setTimeout(() => reject(new Error('app_info timed out')), 2000))
    return (await Promise.race([transport.request(APP_INFO_REQUEST), timeout])) as AppInfo
  } catch {
    return (await createMockTransport().request(APP_INFO_REQUEST)) as AppInfo
  }
}

/** Resolve when the raw transport reports a connected daemon, or after `timeoutMs` (the UI then shows its own state). */
export function waitForDaemon(raw: RawTransport, timeoutMs: number): Promise<boolean> {
  return new Promise((resolve) => {
    let done = false
    let off: () => void = () => undefined
    const finish = (ok: boolean): void => {
      if (done) return
      done = true
      clearTimeout(timer)
      off()
      resolve(ok)
    }
    const timer = setTimeout(() => finish(false), timeoutMs)
    off = raw.onStatus((s) => s.connected && finish(true))
    void raw.status().then((s) => s.connected && finish(true))
  })
}

/**
 * Install `window.hydra` before the first render. In the desktop app (or a browser opened with `?bridge=...&token=...` against
 * `agent-studio-gui --dev-bridge`) the methods are answered by the daemon through the mapping layer (./daemonApi.ts); in a plain
 * browser and in tests an in-memory mock answers. The G2 methods still fall through to the mock.
 */
export async function installHydra(raw: RawTransport | null = selectRawTransport()): Promise<HydraAPI> {
  const info = raw ? await loadAppInfo(raw) : undefined
  // The daemon is started by the shell in the background: give it a moment so the first render already finds it.
  if (raw) await waitForDaemon(raw, 4000)
  const api = createHydraApi(createBridge(selectTransport(raw, info)))
  window.hydra = api
  return api
}
