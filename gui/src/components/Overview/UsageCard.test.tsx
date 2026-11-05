// @vitest-environment jsdom

import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type { LimitWindow, UsagePeriod, UsageState } from '@shared/usage/types'
import { NOW, profile, summaryOf, usageState, win } from '../Usage/fixtures'
import { UsageCard } from './UsageCard'

const getUsageSummary = vi.fn(async (period: UsagePeriod, _projectKey?: string) => summaryOf(period))
const setUsageVisible = vi.fn(async () => {})

function renderCard(state: UsageState | null = usageState(), projectDir = '', onOpenReport = vi.fn()) {
  render(<UsageCard state={state} projectDir={projectDir} onOpenReport={onOpenReport} />)
  return { onOpenReport }
}

describe('UsageCard', () => {
  beforeEach(() => {
    vi.useFakeTimers({ toFake: ['Date'] })
    vi.setSystemTime(NOW)
    localStorage.clear()
    getUsageSummary.mockClear()
    getUsageSummary.mockImplementation(async (period) => summaryOf(period))
    setUsageVisible.mockClear()
    ;(window as unknown as { hydra: unknown }).hydra = { getUsageSummary, setUsageVisible }
  })
  afterEach(() => {
    cleanup()
    vi.useRealTimers()
  })

  it('renders a tile per window with account labels, and a tile for the expired account', async () => {
    renderCard()
    await screen.findByText(/API-equivalent/)
    expect(screen.getByText('Claude default · session (5h)')).toBeTruthy()
    expect(screen.getByText('Claude alternative · weekly')).toBeTruthy()
    expect(screen.getByText('29%')).toBeTruthy()
    expect(screen.getByText('resets in 1h 48m')).toBeTruthy()
    expect(screen.getByText('Claude work')).toBeTruthy()
    expect(screen.getByText('Sign-in expired')).toBeTruthy()
    expect(screen.getByText('updated 1 min ago')).toBeTruthy()
  })

  it('names no account with a single Claude account, and marks Codex as estimated', async () => {
    const codexWindow: LimitWindow = { ...win('codex', 'weekly', 40, NOW + 86_400_000), provider: 'codex' }
    renderCard(
      usageState({
        claude: [profile('default')],
        codex: { plan: null, windows: [codexWindow], at: NOW - 3 * 3_600_000 }
      } as Partial<UsageState>)
    )
    await screen.findByText(/API-equivalent/)
    expect(screen.getByText('Claude · session (5h)')).toBeTruthy()
    expect(screen.getByText('Codex · weekly')).toBeTruthy()
    expect(screen.getByText('ESTIMATED · 3 h')).toBeTruthy()
  })

  it('says so when there are no plan limits', async () => {
    renderCard(usageState({ claude: [] }))
    expect(screen.getByText('No plan limits found.')).toBeTruthy()
  })

  it('caps the tiles at eight', async () => {
    const many = ['a', 'b', 'c', 'd', 'e'].map((n) => profile(n))
    renderCard(usageState({ claude: many }))
    await screen.findByText(/API-equivalent/)
    expect(screen.getAllByRole('progressbar')).toHaveLength(8)
    expect(screen.getByText('+2 more in the header popover')).toBeTruthy()
  })

  it('shows the week by default and refetches when the period changes', async () => {
    renderCard()
    await screen.findByText(/API-equivalent/)
    expect(getUsageSummary).toHaveBeenCalledWith('week')
    expect(screen.getByText('Tokens · this week')).toBeTruthy()
    expect(screen.getByText(/▲ 12% vs last week/)).toBeTruthy()
    fireEvent.click(screen.getByRole('button', { name: 'Month' }))
    await waitFor(() => expect(getUsageSummary).toHaveBeenCalledWith('month'))
    expect(screen.getByText('Tokens · this month')).toBeTruthy()
    expect(localStorage.getItem('agentStudio.usage.cardPeriod')).toBe('month')
  })

  it('passes the project to the summary and notes that limits are per account', async () => {
    renderCard(usageState(), '/work/alpha')
    await waitFor(() => expect(getUsageSummary).toHaveBeenCalledWith('week', '/work/alpha'))
    expect(screen.getByText('Tokens · this week · alpha')).toBeTruthy()
    expect(screen.getByText('Limits are per account, not per project')).toBeTruthy()
  })

  it('has no project note for all projects', async () => {
    renderCard()
    await screen.findByText(/API-equivalent/)
    expect(screen.queryByText('Limits are per account, not per project')).toBeNull()
  })

  it('shows a skeleton while the state is null and fetches nothing', () => {
    renderCard(null)
    expect(screen.getByTestId('usage-skeleton')).toBeTruthy()
    expect(getUsageSummary).not.toHaveBeenCalled()
  })

  it('opens the detailed report', async () => {
    const { onOpenReport } = renderCard()
    fireEvent.click(within(screen.getByRole('region', { name: 'Usage & limits' })).getByRole('button', { name: 'Detailed report' }))
    expect(onOpenReport).toHaveBeenCalledTimes(1)
  })

  it('tells the service it is visible', () => {
    renderCard()
    expect(setUsageVisible).toHaveBeenCalled()
  })
})
