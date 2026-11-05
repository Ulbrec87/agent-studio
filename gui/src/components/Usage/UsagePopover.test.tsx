// @vitest-environment jsdom

import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type { UsagePeriod, UsageState } from '@shared/usage/types'
import { NOW, profile, summaryOf, usageState, win } from './fixtures'
import { UsageMenu } from './UsageMenu'

const getUsageSummary = vi.fn(async (period: UsagePeriod) => summaryOf(period))
const refreshUsage = vi.fn(async () => usageState())
const setUsageVisible = vi.fn(async () => {})

function open(state: UsageState | null = usageState(), onOpenReport = vi.fn()) {
  const view = render(<UsageMenu state={state} onOpenReport={onOpenReport} />)
  fireEvent.click(screen.getByRole('button', { name: /Usage and limits/ }))
  return { ...view, onOpenReport }
}

async function dialog(): Promise<HTMLElement> {
  const d = screen.getByRole('dialog', { name: 'Usage and limits' })
  await screen.findByText(/API-equivalent/)
  return d
}

describe('UsagePopover', () => {
  beforeEach(() => {
    vi.useFakeTimers({ toFake: ['Date'] })
    vi.setSystemTime(NOW)
    localStorage.clear()
    getUsageSummary.mockClear()
    getUsageSummary.mockImplementation(async (period) => summaryOf(period))
    refreshUsage.mockClear()
    setUsageVisible.mockClear()
    ;(window as unknown as { hydra: unknown }).hydra = { getUsageSummary, refreshUsage, setUsageVisible }
  })
  afterEach(() => {
    cleanup()
    vi.useRealTimers()
  })

  it('renders accounts, labels and the expired state', async () => {
    open()
    const d = await dialog()
    expect(within(d).getByText('Pro · default')).toBeTruthy()
    expect(within(d).getByText('Pro · alternative')).toBeTruthy()
    expect(within(d).getByText('work')).toBeTruthy()
    expect(within(d).getAllByText('live')).toHaveLength(2)
    expect(within(d).getByText('29% used')).toBeTruthy()
    expect(within(d).getByText('resets in 1h 48m')).toBeTruthy()
    expect(within(d).getByText('Sign-in expired')).toBeTruthy()
    expect(within(d).getByText('Start a Claude agent in this profile to refresh the sign-in.')).toBeTruthy()
  })

  it('shows last values dimmed under an expired sign-in', async () => {
    const expired = profile('default', { status: 'expired', fetchedAt: NOW - 2 * 3_600_000 })
    open(usageState({ claude: [expired] }))
    const d = await dialog()
    expect(within(d).getByText('Showing the last values from 2 h ago.')).toBeTruthy()
    expect(within(d).getByText('29% used').closest('[data-dimmed]')?.getAttribute('data-dimmed')).toBe('true')
  })

  it('explains the other problem states', async () => {
    open(
      usageState({
        claude: [
          profile('a', { status: 'keychain-denied', windows: [], fetchedAt: null }),
          profile('b', { status: 'rate-limited', windows: [], fetchedAt: null }),
          profile('c', { status: 'unavailable', windows: [], fetchedAt: null })
        ]
      })
    )
    const d = await dialog()
    expect(within(d).getByText('Agent Studio could not read this sign-in from the Keychain.')).toBeTruthy()
    expect(within(d).getByText('Limits unavailable — will retry shortly.')).toBeTruthy()
    expect(within(d).getByText(/No plan limits for this sign-in/)).toBeTruthy()
  })

  it('hides not-signed-in accounts while another is signed in', async () => {
    open(usageState({ claude: [profile('default'), profile('old', { status: 'not-signed-in', windows: [] })] }))
    const d = await dialog()
    expect(within(d).queryByText('Not signed in.')).toBeNull()
  })

  it('Low filter: a quiet line when nothing is near its limit', async () => {
    open()
    const d = await dialog()
    fireEvent.click(within(d).getByRole('button', { name: 'Low' }))
    expect(within(d).getByText('No plan window is near its limit.')).toBeTruthy()
    expect(localStorage.getItem('agentStudio.usage.limitsFilter')).toBe('low')
  })

  it('Low filter: shows the window that is near its limit, with nothing else', async () => {
    const state = usageState({
      claude: [profile('default', { windows: [win('default', 'session', 85, NOW + 600_000), win('default', 'weekly', 40, null)] })]
    })
    open(state)
    const d = await dialog()
    fireEvent.click(within(d).getByRole('button', { name: 'Low' }))
    expect(within(d).getByText('85% used')).toBeTruthy()
    expect(within(d).queryByText('40% used')).toBeNull()
  })

  it('remembers the filter', async () => {
    localStorage.setItem('agentStudio.usage.limitsFilter', 'low')
    open()
    const d = await dialog()
    expect(within(d).getByRole('button', { name: 'Low' }).getAttribute('aria-pressed')).toBe('true')
  })

  it('Escape closes the popover and returns focus to the chip', async () => {
    open()
    const d = await dialog()
    await waitFor(() => expect(document.activeElement).toBe(d))
    fireEvent.keyDown(document, { key: 'Escape' })
    expect(screen.queryByRole('dialog')).toBeNull()
    expect(document.activeElement).toBe(screen.getByRole('button', { name: /Usage and limits/ }))
  })

  it('an outside click closes it', async () => {
    open()
    await dialog()
    fireEvent.mouseDown(document.body)
    expect(screen.queryByRole('dialog')).toBeNull()
  })

  it('switching the period refetches', async () => {
    open()
    const d = await dialog()
    expect(getUsageSummary).toHaveBeenCalledWith('today')
    fireEvent.click(within(d).getByRole('button', { name: 'Week' }))
    await waitFor(() => expect(getUsageSummary).toHaveBeenCalledWith('week'))
    expect(localStorage.getItem('agentStudio.usage.period')).toBe('week')
  })

  it('refetches when dataVersion changes while open', async () => {
    const { rerender } = open()
    await dialog()
    const calls = getUsageSummary.mock.calls.length
    rerender(<UsageMenu state={usageState({ dataVersion: 2 })} onOpenReport={() => {}} />)
    await waitFor(() => expect(getUsageSummary.mock.calls.length).toBe(calls + 1))
  })

  it('shows the hero, change and top model', async () => {
    open()
    const d = await dialog()
    expect(within(d).getByTitle('48,200,000').textContent).toBe('48.2M')
    expect(within(d).getByText(/~\$31\.40 API-equivalent/)).toBeTruthy()
    expect(within(d).getByText('▲ 12% tokens vs yesterday · 1,284 messages')).toBeTruthy()
    expect(within(d).getByText('Top model: opus-5-5 · 31.2M')).toBeTruthy()
    expect(within(d).getByText('no usage')).toBeTruthy()
  })

  it('prefixes a partial cost with ≥', async () => {
    getUsageSummary.mockImplementation(async (period) => summaryOf(period, { costPartial: true }))
    open()
    const d = await dialog()
    expect(within(d).getByText(/≥\$31\.40 API-equivalent/)).toBeTruthy()
  })

  it('omits the change when there is no previous period', async () => {
    getUsageSummary.mockImplementation(async (period) => summaryOf(period, { tokensChangePct: null }))
    open()
    const d = await dialog()
    expect(within(d).getByText('1,284 messages')).toBeTruthy()
  })

  it('shows the indexing banner above whatever data exists', async () => {
    const state = usageState({
      indexing: { indexing: true, progress: { filesDone: 1, filesTotal: 3, bytesDone: 40, bytesTotal: 100 }, lastScanAt: null }
    })
    open(state)
    const d = await dialog()
    expect(within(d).getByText('Indexing usage… 40%')).toBeTruthy()
    expect(within(d).getByTitle('48,200,000')).toBeTruthy()
  })

  it('refresh calls the service', async () => {
    open()
    const d = await dialog()
    await act(async () => {
      fireEvent.click(within(d).getByRole('button', { name: 'Refresh usage' }))
    })
    expect(refreshUsage).toHaveBeenCalledTimes(1)
  })

  it('tells the service it is visible', async () => {
    open()
    await dialog()
    expect(setUsageVisible).toHaveBeenCalledTimes(1)
  })

  it('Detailed report closes the popover and opens the report', async () => {
    const { onOpenReport } = open()
    const d = await dialog()
    fireEvent.click(within(d).getByRole('button', { name: 'Detailed report' }))
    expect(onOpenReport).toHaveBeenCalledTimes(1)
    expect(screen.queryByRole('dialog')).toBeNull()
  })

  it('shows a Codex group with the estimated pill', async () => {
    const state = usageState({
      claude: [profile('default')],
      codex: { at: NOW - 6 * 60_000, account: 'default', plan: 'Plus', windows: [{ ...win('default', 'session', 10, NOW + 60_000), provider: 'codex' }] }
    })
    open(state)
    const d = await dialog()
    expect(within(d).getByText('ESTIMATED')).toBeTruthy()
    expect(within(d).getByText(/from session log · 6 min/)).toBeTruthy()
  })
})
