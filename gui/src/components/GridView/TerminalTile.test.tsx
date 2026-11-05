// @vitest-environment jsdom

import { cleanup, fireEvent, render, screen } from '@testing-library/react'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { TerminalTile } from './TerminalTile'
import type { AgentState } from '@shared/types'

vi.mock('../Terminal/TerminalPane', () => ({
  TerminalPane: () => <div>terminal-pane</div>
}))

const agent: AgentState = {
  id: 'agent-1',
  name: 'Tile Agent',
  projectDir: '/tmp/project',
  provider: 'claude',
  model: 'sonnet',
  yolo: false,
  isManager: false,
  sessionId: null,
  initialPrompt: '',
  createdAt: '2026-01-01T00:00:00.000Z',
  status: 'running',
  pid: 123,
  restartCount: 0,
  startedAt: '2026-01-01T00:00:00.000Z',
  endedAt: null,
  lastActivityAt: '2026-01-01T00:00:00.000Z',
  workMode: 'local',
  worktreePath: null,
  worktreeBranch: null
}

function renderTile() {
  const props = {
    agent,
    rawOutput: '',
    isExpanded: false,
    onToggleExpand: vi.fn(),
    onOpenInChat: vi.fn(),
    onTerminalData: vi.fn(),
    onTerminalResize: vi.fn(),
    onStartOrRestart: vi.fn(),
    onStopAgent: vi.fn(),
    onRemove: vi.fn(),
    hidden: false
  }
  render(<TerminalTile {...props} />)
  return props
}

describe('TerminalTile', () => {
  afterEach(cleanup)

  it('opens the agent in the Chat view when the header is clicked', () => {
    const props = renderTile()
    fireEvent.click(screen.getByText('Tile Agent'))
    expect(props.onOpenInChat).toHaveBeenCalledTimes(1)
    expect(props.onToggleExpand).not.toHaveBeenCalled()
  })

  it('expands from its own button without opening the chat', () => {
    const props = renderTile()
    fireEvent.click(screen.getByTitle('Expand'))
    expect(props.onToggleExpand).toHaveBeenCalledTimes(1)
    expect(props.onOpenInChat).not.toHaveBeenCalled()
  })

  it('does not open the chat from the header action buttons', () => {
    const props = renderTile()
    fireEvent.click(screen.getByTitle('Stop'))
    fireEvent.click(screen.getByTitle('Remove'))
    expect(props.onStopAgent).toHaveBeenCalledTimes(1)
    expect(props.onRemove).toHaveBeenCalledTimes(1)
    expect(props.onOpenInChat).not.toHaveBeenCalled()
  })
})
