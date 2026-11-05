// @vitest-environment jsdom

import { cleanup, render, screen } from '@testing-library/react'
import { afterEach, describe, expect, it } from 'vitest'
import { ViewSwitcher } from './ViewSwitcher'

describe('ViewSwitcher needs-you badge', () => {
  afterEach(cleanup)

  it('shows the count with an accessible label on the Overview tab', () => {
    render(<ViewSwitcher viewMode="grid" onSelect={() => undefined} needsYouCount={2} />)
    const tab = screen.getByRole('tab', { name: 'Overview, 2 agents need you' })
    expect(tab.textContent).toContain('2')
  })

  it('uses the singular for one agent', () => {
    render(<ViewSwitcher viewMode="chat" onSelect={() => undefined} needsYouCount={1} />)
    expect(screen.getByRole('tab', { name: 'Overview, 1 agent needs you' })).toBeTruthy()
  })

  it('is hidden at 0 and when not given', () => {
    const { rerender } = render(<ViewSwitcher viewMode="grid" onSelect={() => undefined} needsYouCount={0} />)
    expect(screen.getByRole('tab', { name: 'Overview' }).textContent).toBe('Overview')
    rerender(<ViewSwitcher viewMode="grid" onSelect={() => undefined} />)
    expect(screen.getByRole('tab', { name: 'Overview' }).textContent).toBe('Overview')
  })

  it('does not badge the other tabs', () => {
    render(<ViewSwitcher viewMode="grid" onSelect={() => undefined} needsYouCount={3} />)
    expect(screen.getByRole('tab', { name: 'Grid' }).textContent).toBe('Grid')
    expect(screen.getByRole('tab', { name: 'Chat' }).textContent).toBe('Chat')
  })
})
