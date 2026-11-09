// @vitest-environment jsdom

import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { HydraImportTab } from './HydraImportTab'

const PREVIEW = {
  dir: '/home/u/.config/Hydra',
  configChanges: [{ key: 'theme', current: 'dark', incoming: 'midnight' }],
  agentsTotal: 3,
  agentsNew: 2,
  warnings: ['1 agent(s) already exist or could not be read and will be skipped']
}

describe('HydraImportTab', () => {
  let preview: ReturnType<typeof vi.fn>
  let apply: ReturnType<typeof vi.fn>

  beforeEach(() => {
    preview = vi.fn().mockResolvedValue(PREVIEW)
    apply = vi.fn().mockResolvedValue({ configKeysApplied: 1, agentsAdded: [{}, {}], warnings: [] })
    window.hydra = {
      ...window.hydra,
      previewHydraImport: preview,
      applyHydraImport: apply,
      selectDirectory: vi.fn().mockResolvedValue('/picked/Hydra')
    } as never
  })

  afterEach(() => {
    cleanup()
    vi.restoreAllMocks()
  })

  it('previews, then applies only after the confirm button', async () => {
    render(<HydraImportTab />)
    fireEvent.click(screen.getByText('Preview import'))
    await waitFor(() => expect(screen.getByText(/2 new agents/)).toBeTruthy())
    expect(preview).toHaveBeenCalledWith('')
    expect(screen.getByText('midnight')).toBeTruthy()
    expect(screen.getByText(/already exist/)).toBeTruthy()
    expect(apply).not.toHaveBeenCalled()
    fireEvent.click(screen.getByText('Import now'))
    await waitFor(() => expect(screen.getByText('Import finished')).toBeTruthy())
    expect(apply).toHaveBeenCalledWith('')
    expect(screen.getByText(/2 agents added, 1 setting updated/)).toBeTruthy()
  })

  it('uses the browsed folder and forgets a stale preview when it changes', async () => {
    render(<HydraImportTab />)
    fireEvent.click(screen.getByText('Browse'))
    await waitFor(() => expect((screen.getByLabelText('Hydra data folder') as HTMLInputElement).value).toBe('/picked/Hydra'))
    fireEvent.click(screen.getByText('Preview import'))
    await waitFor(() => expect(screen.getByText('Import now')).toBeTruthy())
    expect(preview).toHaveBeenCalledWith('/picked/Hydra')
    fireEvent.change(screen.getByLabelText('Hydra data folder'), { target: { value: '/other' } })
    expect(screen.queryByText('Import now')).toBeNull()
  })

  it('shows the daemon error and offers no import when nothing differs', async () => {
    preview.mockRejectedValueOnce(new Error('no Hydra data in /x'))
    render(<HydraImportTab />)
    fireEvent.click(screen.getByText('Preview import'))
    await waitFor(() => expect(screen.getByRole('alert').textContent).toMatch(/no Hydra data/))
    preview.mockResolvedValueOnce({ ...PREVIEW, configChanges: [], agentsNew: 0, warnings: [] })
    fireEvent.click(screen.getByText('Preview import'))
    await waitFor(() => expect(screen.getByText(/Nothing to import/)).toBeTruthy())
    expect(screen.queryByText('Import now')).toBeNull()
  })
})
