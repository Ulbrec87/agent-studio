import { createBridge } from './bridge'
import { createHydraApi, type HydraAPI } from './hydraApi'
import { APP_INFO_REQUEST, selectTransport, type AppInfo, type Transport } from './transport'
import { createMockTransport } from './mockTransport'

export type { HydraAPI } from './hydraApi'
export type { Transport, AppInfo } from './transport'

/** What the shell reports about itself (name, version, data dir, platform). Falls back to the mock's answer. */
export async function loadAppInfo(transport: Transport = selectTransport()): Promise<AppInfo> {
  try {
    return (await transport.request(APP_INFO_REQUEST)) as AppInfo
  } catch {
    return (await createMockTransport().request(APP_INFO_REQUEST)) as AppInfo
  }
}

/**
 * Install `window.hydra` before the first render. G0: every method is answered by an in-memory mock (see ./README.md);
 * only `app_info` comes from the Tauri shell when running in the desktop app. G1: pass the selected `transport` to
 * `createBridge` instead of the mock.
 */
export async function installHydra(transport: Transport = selectTransport()): Promise<HydraAPI> {
  const info = await loadAppInfo(transport)
  const stub = createMockTransport({ appInfo: info })
  const api = createHydraApi(createBridge(stub))
  window.hydra = api
  return api
}
