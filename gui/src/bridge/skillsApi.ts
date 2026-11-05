// Skills: scan (`ScanSkills`) and toggle (`ToggleSkill`). The daemon's `SkillInfo` is Hydra's plus `configDir` (the Claude profile a
// plugin skill belongs to); its toggle request needs that and the skill file, which Hydra's `{ provider, id, enabled }` does not carry,
// so the bridge looks the skill up in a fresh scan.
import type { ProviderId, SkillInfo, SkillScanResult, SkillTogglePayload } from '@shared/types'
import type { BridgeContext, MethodGroup } from './context'

export const SKILL_METHODS = ['scanSkills', 'toggleSkill'] as const

type DaemonSkill = SkillInfo & { configDir?: string }
interface DaemonScan {
  claude: DaemonSkill[]
  codex: DaemonSkill[]
  opencode: DaemonSkill[]
}

export function mapSkillScan(scan: DaemonScan, now: () => Date = () => new Date()): SkillScanResult {
  return { claude: scan.claude ?? [], codex: scan.codex ?? [], opencode: scan.opencode ?? [], scannedAt: now().toISOString() }
}

/**
 * The daemon requests that carry out a Hydra toggle. Claude plugin skills toggle their plugin (`id` is the plugin key, once per profile that
 * has it); every other skill is a Codex-style `SKILL.md` that is renamed (`id` is the directory name). Claude's own user skills
 * (`group: "user"`) have no enabled switch in Claude Code, so there is nothing to send for them.
 */
export function togglePayloads(scan: DaemonScan, toggle: SkillTogglePayload): Array<Record<string, unknown>> {
  if (toggle.provider === 'claude') {
    const dirs = [...new Set(scan.claude.filter((s) => s.group === toggle.id && s.group !== 'user').map((s) => s.configDir ?? ''))]
    return dirs.map((configDir) => ({ provider: 'claude', enabled: toggle.enabled, group: toggle.id, configDir }))
  }
  const list = (toggle.provider === 'codex' ? scan.codex : scan.opencode).filter((s) => s.id === toggle.id)
  return list.slice(0, 1).map((s) => ({ provider: toggle.provider as ProviderId, enabled: toggle.enabled, path: s.path }))
}

export function createSkillMethods(ctx: BridgeContext): MethodGroup {
  const scan = (): Promise<DaemonScan> => ctx.call<DaemonScan>('scan_skills', undefined, 'skills')
  return {
    methods: {
      scanSkills: (async () => mapSkillScan(await scan())) as never,
      toggleSkill: (async (toggle: SkillTogglePayload) => {
        try {
          const payloads = togglePayloads(await scan(), toggle)
          if (payloads.length === 0) return { success: false }
          for (const payload of payloads) await ctx.call('toggle_skill', payload, 'ok')
          return { success: true }
        } catch (err) {
          console.warn('[bridge] toggleSkill failed', err)
          return { success: false }
        }
      }) as never
    }
  }
}
