// @vitest-environment jsdom

import { cleanup, render, screen } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { UsageChip } from './UsageChip'
import { NOW, profile, usageState, win } from './fixtures'

function renderChip(state: ReturnType<typeof usageState> | null) {
  return render(<UsageChip state={state} open={false} onClick={() => {}} />)
}

describe('UsageChip', () => {
  beforeEach(() => {
    vi.useFakeTimers({ toFake: ['Date'] })
    vi.setSystemTime(NOW)
  })
  afterEach(() => {
    cleanup()
    vi.useRealTimers()
  })

  it('shows the highest session and weekly across accounts', () => {
    renderChip(usageState())
    expect(screen.getByTestId('chip-5h').textContent).toContain('29%')
    expect(screen.getByTestId('chip-wk').textContent).toContain('26%')
  })

  it('has an accessible name and popup semantics', () => {
    renderChip(usageState())
    const button = screen.getByRole('button')
    expect(button.getAttribute('aria-label')).toBe('Usage and limits: session 29% used, weekly 26% used')
    expect(button.getAttribute('aria-haspopup')).toBe('dialog')
    expect(button.getAttribute('aria-expanded')).toBe('false')
  })

  it('colours each meter by severity and flags a high one on the button', () => {
    const state = usageState({
      claude: [profile('default', { windows: [win('default', 'session', 85, null), win('default', 'weekly', 55, null)] })]
    })
    renderChip(state)
    expect(screen.getByTestId('chip-5h').getAttribute('data-severity')).toBe('high')
    expect(screen.getByTestId('chip-wk').getAttribute('data-severity')).toBe('warn')
    expect(screen.getByRole('button').getAttribute('data-high')).toBe('true')
  })

  it('is green and not flagged when low', () => {
    renderChip(usageState())
    expect(screen.getByTestId('chip-5h').getAttribute('data-severity')).toBe('ok')
    expect(screen.getByRole('button').getAttribute('data-high')).toBe('false')
  })

  it('is a plain Usage button with no windows', () => {
    renderChip(usageState({ claude: [profile('work', { status: 'expired', windows: [], fetchedAt: null })] }))
    const button = screen.getByRole('button')
    expect(button.textContent).toBe('Usage')
    expect(button.getAttribute('aria-label')).toBe('Usage and limits')
    expect(screen.queryByTestId('chip-5h')).toBeNull()
  })

  it('is a plain Usage button before the first state', () => {
    renderChip(null)
    expect(screen.getByRole('button').textContent).toBe('Usage')
  })

  it('dims values older than 15 minutes', () => {
    const state = usageState({
      claude: [profile('default', { fetchedAt: NOW - 20 * 60_000 })]
    })
    renderChip(state)
    expect(screen.getByTestId('chip-5h').getAttribute('data-stale')).toBe('true')
  })

  it('does not dim fresh values', () => {
    renderChip(usageState())
    expect(screen.getByTestId('chip-5h').getAttribute('data-stale')).toBe('false')
  })
})
