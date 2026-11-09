// Pure shape adapters between the daemon's JSON and Hydra's TypeScript types (`gui/shared/types.ts`). Most shapes already
// match (camelCase domain structs); these are the ones that do not.
import { DEFAULT_CONFIG, type AppConfig, type HydraNotification, type McpServerStatus, type NotificationType, type PreflightResult, type ProviderId } from '@shared/types'
import type { ProviderPreflight as ProtoPreflight, StudioNotification as ProtoNotification } from './generated'
import type { ProtoResponse } from './protocol'

/** Unwrap `{ type, payload }`, failing loudly when the daemon answered another variant. */
export function expectPayload<T>(response: ProtoResponse, ...types: string[]): T {
  if (!types.includes(response.type)) {
    throw new Error(`unexpected daemon response "${response.type}" (wanted ${types.join(' or ')})`)
  }
  return response.payload as T
}

/** Drop `undefined` values (serde `default` fields accept a missing key but not always an explicit `null`). */
export function compact<T extends Record<string, unknown>>(value: T): Partial<T> {
  return Object.fromEntries(Object.entries(value).filter(([, v]) => v !== undefined)) as Partial<T>
}

/** `Preflight` answers one entry per provider with `path`; Hydra wants one result with `claudePath`. */
export function mapPreflight(entries: ProtoPreflight[], provider?: ProviderId): PreflightResult {
  const wanted = provider ?? 'claude'
  const found = entries.find((e) => e.provider === wanted) ?? entries[0]
  if (!found) return { ok: false, claudePath: null, version: null, error: 'Preflight returned no result' }
  return { ok: found.result.ok, claudePath: found.result.path ?? null, version: found.result.version ?? null, error: found.result.error ?? null }
}

/** `StudioNotification` (snake_case, `kind`) -> Hydra's `HydraNotification` (`type`, `agentId`). */
export function mapNotification(n: ProtoNotification): HydraNotification {
  const out: HydraNotification = { id: n.id, type: n.kind, title: n.title, body: n.body, timestamp: n.timestamp }
  if (n.agent_id) out.agentId = n.agent_id
  return out
}

export function mapMcpStatus(s: { running: boolean; port: number | null; error: string | null; manager_workspace: string | null }): McpServerStatus {
  return { running: s.running, port: s.port ?? null, error: s.error ?? null, managerWorkspace: s.manager_workspace ?? null }
}

/** The daemon's config with defaults for keys it does not know yet (an older daemon predating a Hydra key). */
export function toHydraConfig(daemonConfig: Partial<AppConfig>): AppConfig {
  return { ...DEFAULT_CONFIG, ...daemonConfig } as AppConfig
}
