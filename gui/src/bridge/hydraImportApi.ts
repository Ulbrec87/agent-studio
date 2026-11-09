// Import from Hydra (an Agent Studio extension, not in Hydra's preload): `PreviewHydraImport` shows what would change, `ApplyHydraImport`
// merges Hydra's config and agents into ours. `dir` overrides Hydra's default data folder (empty or missing = the default).
// Daemon errors (folder without Hydra data, unreadable files) reject with the daemon's message; the Settings section shows it.
import type { ImportPreview, ImportReport } from './generated'
import type { BridgeContext, MethodGroup } from './context'

export const HYDRA_IMPORT_METHODS = ['previewHydraImport', 'applyHydraImport'] as const

export type { ImportPreview, ImportReport }

/** The daemon wants `null` for "default folder"; a blank text field means the same. */
export function importDir(dir: string | undefined | null): string | null {
  const trimmed = dir?.trim()
  return trimmed ? trimmed : null
}

export function createHydraImportMethods(ctx: BridgeContext): MethodGroup {
  return {
    methods: {
      previewHydraImport: ((dir?: string) => ctx.call<ImportPreview>('preview_hydra_import', { dir: importDir(dir) }, 'hydra_preview')) as never,
      applyHydraImport: ((dir?: string) => ctx.call<ImportReport>('apply_hydra_import', { dir: importDir(dir) }, 'hydra_report')) as never
    }
  }
}
