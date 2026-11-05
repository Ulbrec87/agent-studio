// @vitest-environment jsdom

import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { DEFAULT_USAGE_SETTINGS } from '@shared/usage/settings'
import type { UsageReport, UsageReportDay, UsageReportProvider } from '@shared/usage/report'
import { emptyTokens } from '@shared/usage/types'
import { UsageSettingsProvider } from '../Usage/UsageSettingsContext'
import { UsageDashboard } from './UsageDashboard'

function day(date: string, over: Partial<UsageReportDay> = {}): UsageReportDay {
  return {
    date,
    tokens: emptyTokens(),
    totalTokens: 0,
    messages: 0,
    costUsd: 0,
    costPartial: false,
    models: [],
    projects: [],
    ...over
  }
}

function fixture(provider: UsageReportProvider = 'all'): UsageReport {
  const daily: UsageReportDay[] = []
  for (let i = 29; i >= 1; i--) {
    const d = new Date(2026, 9, 23 - i)
    const m = String(d.getMonth() + 1).padStart(2, '0')
    daily.push(day(`${d.getFullYear()}-${m}-${String(d.getDate()).padStart(2, '0')}`))
  }
  daily.push(
    day('2026-10-23', {
      tokens: { input: 1_200_000, output: 300_000, cacheWrite5m: 400_000, cacheWrite1h: 100_000, cacheRead: 9_000_000 },
      totalTokens: 11_000_000,
      messages: 40,
      costUsd: 12.5,
      costPartial: true,
      models: [
        {
          model: 'claude-opus-4-6',
          tokens: { input: 1_200_000, output: 300_000, cacheWrite5m: 400_000, cacheWrite1h: 100_000, cacheRead: 9_000_000 },
          totalTokens: 11_000_000,
          costUsd: 12.5
        }
      ],
      projects: [{ projectKey: '/home/u/code/hydra', label: 'hydra', totalTokens: 11_000_000, costUsd: 12.5 }]
    })
  )
  return { provider, daily, generatedAt: 0 }
}

/** Value of a summary card (its label is a span, unlike the table headers). */
function card(label: string): string | undefined {
  const span = screen.getAllByText(label).find((el) => el.tagName === 'SPAN')
  return span?.nextElementSibling?.textContent ?? undefined
}

const getUsageReport = vi.fn(async (_days: number, provider: UsageReportProvider) => fixture(provider))
const refreshUsage = vi.fn(async () => ({}))

function renderDashboard(showCost = true) {
  return render(
    <UsageSettingsProvider settings={{ ...DEFAULT_USAGE_SETTINGS, showCost }}>
      <UsageDashboard onClose={() => {}} />
    </UsageSettingsProvider>
  )
}

describe('UsageDashboard', () => {
  beforeEach(() => {
    getUsageReport.mockClear()
    getUsageReport.mockImplementation(async (_d, provider) => fixture(provider))
    refreshUsage.mockClear()
    ;(window as unknown as { hydra: unknown }).hydra = { getUsageReport, refreshUsage }
  })
  afterEach(cleanup)

  it('fetches 30 days for all providers and renders cards and tables', async () => {
    renderDashboard()
    await screen.findByText('Model Breakdown')
    expect(getUsageReport).toHaveBeenCalledWith(30, 'all')
    expect(card('Cost')).toBe('≥$12.50')
    expect(card('Total Tokens')).toBe('11M')
    expect(card('Input')).toBe('1.2M')
    expect(card('Output')).toBe('300k')
    expect(card('Cache Write')).toBe('500k')
    expect(card('Cache Read')).toBe('9M')
    expect(screen.getByText('Opus 4.6')).toBeTruthy()
    const project = screen.getByText('hydra')
    expect(project.getAttribute('title')).toBe('/home/u/code/hydra')
    expect(screen.getByText('Cost Trend')).toBeTruthy()
    expect(screen.queryByText('OpenCode')).toBeNull()
  })

  it('refetches with the chosen provider', async () => {
    renderDashboard()
    await screen.findByText('Model Breakdown')
    fireEvent.click(screen.getByRole('button', { name: 'Codex' }))
    await waitFor(() => expect(getUsageReport).toHaveBeenLastCalledWith(30, 'codex'))
    await screen.findByText('Model Breakdown')
    fireEvent.click(screen.getByRole('button', { name: 'Claude' }))
    await waitFor(() => expect(getUsageReport).toHaveBeenLastCalledWith(30, 'claude'))
  })

  it('refresh rescans then refetches', async () => {
    renderDashboard()
    await screen.findByText('Model Breakdown')
    fireEvent.click(screen.getByRole('button', { name: 'Refresh' }))
    await waitFor(() => expect(getUsageReport).toHaveBeenCalledTimes(2))
    expect(refreshUsage).toHaveBeenCalledTimes(1)
  })

  it('hides cost and trends tokens when showCost is off', async () => {
    renderDashboard(false)
    await screen.findByText('Model Breakdown')
    expect(screen.queryByText('Cost')).toBeNull()
    expect(screen.queryByText(/\$/)).toBeNull()
    expect(screen.queryByText('Cost Trend')).toBeNull()
    expect(screen.getByText('Token Trend')).toBeTruthy()
    const modelTable = screen.getByText('Model Breakdown').closest('section') as HTMLElement
    expect(within(modelTable).queryByText('Cost')).toBeNull()
  })

  it('shows the empty state when nothing was recorded', async () => {
    getUsageReport.mockImplementation(async (_d, provider) => ({
      ...fixture(provider),
      daily: fixture(provider).daily.map((d) => day(d.date))
    }))
    renderDashboard()
    await screen.findByText('No usage recorded in the last 30 days.')
    expect(screen.getByRole('button', { name: 'Codex' })).toBeTruthy()
  })
})
