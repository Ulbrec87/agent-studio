import { describe, expect, it } from 'vitest'
import { importDir } from './hydraImportApi'
import { makeBridge } from './testKit'

const PREVIEW = {
  dir: '/h/Hydra',
  configChanges: [{ key: 'theme', current: 'dark', incoming: 'midnight' }],
  agentsTotal: 3,
  agentsNew: 2,
  warnings: []
}

describe('Hydra import', () => {
  it('a blank folder means the default location', () => {
    expect([importDir(undefined), importDir(''), importDir('   '), importDir(' /x ')]).toEqual([null, null, null, '/x'])
  })

  it('previews and applies through the daemon requests, with the custom folder', async () => {
    const { raw, api } = await makeBridge((r) => {
      r.handlers.set('preview_hydra_import', () => ({ type: 'hydra_preview', payload: PREVIEW }))
      r.handlers.set('apply_hydra_import', () => ({ type: 'hydra_report', payload: { configKeysApplied: 1, agentsAdded: [], warnings: [] } }))
    })
    expect(await api.previewHydraImport()).toEqual(PREVIEW)
    expect(raw.of('preview_hydra_import')[0].payload).toEqual({ dir: null })
    expect((await api.applyHydraImport('/custom/Hydra')).configKeysApplied).toBe(1)
    expect(raw.of('apply_hydra_import')[0].payload).toEqual({ dir: '/custom/Hydra' })
  })

  it('a folder without Hydra data rejects with the daemon message', async () => {
    const { api } = await makeBridge((r) => {
      r.handlers.set('preview_hydra_import', () => {
        throw new Error('no Hydra data in /x')
      })
    })
    await expect(api.previewHydraImport('/x')).rejects.toThrow(/no Hydra data/)
  })
})
