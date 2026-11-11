// @vitest-environment jsdom
import { cleanup, render, screen } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { useEffect } from 'react'
import { fixtureAgents } from '../../bridge/fixtures'
import { ChatView } from './ChatView'

const lifecycle = vi.hoisted(() => ({ mount: vi.fn(), unmount: vi.fn() }))
vi.mock('../Terminal/TerminalPane', () => ({
  TerminalPane: () => {
    useEffect(() => { lifecycle.mount(); return () => lifecycle.unmount() }, [])
    return <div data-testid="agent-terminal" />
  }
}))
vi.mock('../EditorPanel', () => ({ EditorPanel: () => <div>Editor contents</div> }))
vi.mock('./InputBar', () => ({ InputBar: () => <div>Composer</div> }))
vi.mock('./DiffStats', () => ({ DiffStats: () => null }))
vi.mock('../Terminal/FreeTerminalPanel', () => ({ FreeTerminalPanel: () => null }))

describe('Chat workspace layout', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    window.hydra = { ...window.hydra, getGitStatus: vi.fn().mockResolvedValue({ branch: 'main' }) }
  })
  afterEach(cleanup)

  it('keeps the terminal mounted when the editor opens, closes, or changes color theme', () => {
    const props = {
      agent: fixtureAgents()[0], rawOutput: 'Existing terminal output',
      onSendInput: vi.fn(), onTerminalData: vi.fn(), onTerminalResize: vi.fn(),
      onRestartAgent: vi.fn(), onKillAgent: vi.fn()
    }
    const view = render(<ChatView {...props} editorOpen={false} />)
    const terminal = screen.getByTestId('agent-terminal')
    view.rerender(<ChatView {...props} editorOpen theme="modern-workspace-soft" />)
    expect(screen.getByText('Editor contents')).toBeTruthy()
    expect(screen.getByTestId('agent-terminal')).toBe(terminal)
    view.rerender(<ChatView {...props} editorOpen={false} theme="light" />)
    expect(screen.queryByText('Editor contents')).toBeNull()
    expect(screen.getByTestId('agent-terminal')).toBe(terminal)
    expect(lifecycle.mount).toHaveBeenCalledTimes(1)
    expect(lifecycle.unmount).not.toHaveBeenCalled()
  })
})
