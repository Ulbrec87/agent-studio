// @vitest-environment jsdom

import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { DEFAULT_USAGE_SETTINGS, type UsageSettings } from '@shared/usage/settings'
import { DEFAULT_CONFIG } from '@shared/types'
import { SettingsPanel } from '../Settings/SettingsPanel'
import { UsageCard } from '../Overview/UsageCard'
import { UsageChip } from './UsageChip'
import { UsageMenu } from './UsageMenu'
import { UsageSettingsProvider } from './UsageSettingsContext'
import { NOW, profile, summaryOf, usageState, win } from './fixtures'
import type { UsageState } from '@shared/usage/types'

const settings = (over: Partial<UsageSettings>): UsageSettings => ({ ...DEFAULT_USAGE_SETTINGS, ...over })

const codexState = (): UsageState =>
  usageState({
    codex: {
      at: NOW - 60_000,
      plan: 'Plus',
      windows: [
        { provider: 'codex', account: 'default', id: 'session', kind: 'session', label: '5h', usedPercent: 61, resetsAt: NOW + 3_600_000 },
        { provider: 'codex', account: 'default', id: 'weekly', kind: 'weekly', label: 'Weekly', usedPercent: 47, resetsAt: NOW + 86_400_000 }
      ]
    } as UsageState['codex']
  })

beforeEach(() => {
  vi.useFakeTimers({ toFake: ['Date'] })
  vi.setSystemTime(NOW)
  localStorage.clear()
  ;(window as unknown as { hydra: unknown }).hydra = {
    getUsageSummary: vi.fn(async (p) => summaryOf(p)),
    refreshUsage: vi.fn(async () => usageState()),
    setUsageVisible: vi.fn(async () => {}),
    getConfigPath: vi.fn(async () => '/x/config.json')
  }
})
afterEach(() => {
  cleanup()
  vi.useRealTimers()
})

describe('chip provider', () => {
  const chip = (provider: UsageSettings['chipProvider']) =>
    render(
      <UsageSettingsProvider settings={settings({ chipProvider: provider })}>
        <UsageChip state={codexState()} open={false} onClick={() => {}} />
      </UsageSettingsProvider>
    )

  it('auto shows the most used across providers', () => {
    chip('auto')
    expect(screen.getByTestId('chip-5h').textContent).toContain('61%')
  })

  it('claude shows only Claude windows', () => {
    chip('claude')
    expect(screen.getByTestId('chip-5h').textContent).toContain('29%')
    expect(screen.getByTestId('chip-wk').textContent).toContain('26%')
  })

  it('codex shows Codex windows', () => {
    chip('codex')
    expect(screen.getByTestId('chip-5h').textContent).toContain('61%')
    expect(screen.getByTestId('chip-wk').textContent).toContain('47%')
  })

  it('falls back to auto when the provider has no windows', () => {
    render(
      <UsageSettingsProvider settings={settings({ chipProvider: 'codex' })}>
        <UsageChip state={usageState()} open={false} onClick={() => {}} />
      </UsageSettingsProvider>
    )
    expect(screen.getByTestId('chip-5h').textContent).toContain('29%')
  })
})

describe('cost visibility', () => {
  it('hides the cost in the popover when showCost is off', async () => {
    render(
      <UsageSettingsProvider settings={settings({ showCost: false })}>
        <UsageMenu state={usageState()} onOpenReport={() => {}} />
      </UsageSettingsProvider>
    )
    fireEvent.click(screen.getByRole('button', { name: /Usage and limits/ }))
    const d = screen.getByRole('dialog', { name: 'Usage and limits' })
    await within(d).findAllByText('48.2M')
    expect(within(d).queryByText(/API-equivalent/)).toBeNull()
    expect(within(d).queryByText(/\$31/)).toBeNull()
  })

  it('shows it by default', async () => {
    render(<UsageMenu state={usageState()} onOpenReport={() => {}} />)
    fireEvent.click(screen.getByRole('button', { name: /Usage and limits/ }))
    expect(await screen.findByText(/API-equivalent/)).toBeTruthy()
  })

  it('hides the cost on the Overview card', async () => {
    render(
      <UsageSettingsProvider settings={settings({ showCost: false })}>
        <UsageCard state={usageState()} projectDir="" />
      </UsageSettingsProvider>
    )
    await screen.findAllByText('48.2M')
    expect(screen.queryByText(/API-equivalent/)).toBeNull()
  })
})

describe('Low filter threshold', () => {
  const state = usageState({
    claude: [profile('default', { windows: [win('default', 'session', 65, NOW + 600_000), win('default', 'weekly', 40, null)] })]
  })
  const lowView = (s: UsageSettings) => {
    render(
      <UsageSettingsProvider settings={s}>
        <UsageMenu state={state} onOpenReport={() => {}} />
      </UsageSettingsProvider>
    )
    fireEvent.click(screen.getByRole('button', { name: /Usage and limits/ }))
    const d = screen.getByRole('dialog', { name: 'Usage and limits' })
    fireEvent.click(within(d).getByRole('button', { name: 'Low' }))
    return d
  }

  it('uses the default level without a custom threshold', () => {
    expect(within(lowView(DEFAULT_USAGE_SETTINGS)).getByText('No plan window is near its limit.')).toBeTruthy()
  })

  it('uses the configured session threshold', () => {
    expect(within(lowView(settings({ sessionAlertPercent: 60 }))).getByText('65% used')).toBeTruthy()
  })
})

describe('Settings › Usage', () => {
  function renderPanel(usage: UsageSettings = DEFAULT_USAGE_SETTINGS) {
    const onUpdate = vi.fn()
    render(
      <SettingsPanel
        config={{ ...DEFAULT_CONFIG, usage }}
        keybindings={[]}
        keybindingsPath=""
        onUpdate={onUpdate}
        onClose={() => {}}
        globalYolo={false}
        onToggleGlobalYolo={() => {}}
      />
    )
    fireEvent.click(screen.getByRole('button', { name: 'Usage' }))
    return onUpdate
  }

  it('renders with alerts off and the percent inputs disabled', async () => {
    renderPanel()
    await waitFor(() => expect(screen.getByLabelText('Session alert at (% used)')).toBeTruthy())
    expect((screen.getByLabelText('Notify me when a plan limit is nearly used') as HTMLInputElement).checked).toBe(false)
    expect((screen.getByLabelText('Session alert at (% used)') as HTMLInputElement).disabled).toBe(true)
    expect((screen.getByLabelText('Weekly alert at (% used)') as HTMLInputElement).disabled).toBe(true)
    expect(screen.getByText(/Costs are estimates at API prices/)).toBeTruthy()
  })

  it('saves the whole usage block on each change', () => {
    const onUpdate = renderPanel(settings({ alertsEnabled: true }))
    fireEvent.change(screen.getByLabelText('Session alert at (% used)'), { target: { value: '70' } })
    expect(onUpdate).toHaveBeenLastCalledWith({ usage: { ...DEFAULT_USAGE_SETTINGS, alertsEnabled: true, sessionAlertPercent: 70 } })
    fireEvent.change(screen.getByLabelText('Weekly alert at (% used)'), { target: { value: '20' } })
    expect(onUpdate).toHaveBeenLastCalledWith({ usage: expect.objectContaining({ weeklyAlertPercent: 50 }) })
    fireEvent.change(screen.getByLabelText('Header chip shows'), { target: { value: 'codex' } })
    expect(onUpdate).toHaveBeenLastCalledWith({ usage: expect.objectContaining({ chipProvider: 'codex' }) })
    fireEvent.click(screen.getByLabelText('Show estimated API-equivalent cost'))
    expect(onUpdate).toHaveBeenLastCalledWith({ usage: expect.objectContaining({ showCost: false }) })
  })

  it('turns alerts on', () => {
    const onUpdate = renderPanel()
    fireEvent.click(screen.getByLabelText('Notify me when a plan limit is nearly used'))
    expect(onUpdate).toHaveBeenCalledWith({ usage: { ...DEFAULT_USAGE_SETTINGS, alertsEnabled: true } })
  })
})
