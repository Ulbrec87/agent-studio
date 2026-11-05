// @vitest-environment jsdom

import { renderHook, act, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { useAgents } from './useAgents'
import type {
  AgentOutputPayload,
  AgentState,
  AgentStatusPayload,
  CreateAgentPayload
} from '@shared/types'

function makeAgent(id: string, overrides: Partial<AgentState> = {}): AgentState {
  return {
    id,
    name: 'Agent',
    projectDir: '/tmp/project',
    provider: 'claude',
    model: 'sonnet',
    yolo: false,
    isManager: false,
    sessionId: null,
    initialPrompt: '',
    createdAt: '2026-01-01T00:00:00.000Z',
    status: 'idle',
    pid: null,
    restartCount: 0,
    startedAt: null,
    endedAt: null,
    lastActivityAt: '2026-01-01T00:00:00.000Z',
    workMode: 'local',
    worktreePath: null,
    worktreeBranch: null,
    ...overrides
  }
}

describe('useAgents', () => {
  let outputHandler: ((payload: AgentOutputPayload) => void) | null
  let statusHandler: ((payload: AgentStatusPayload) => void) | null
  let restartedHandler: ((payload: { agentId: string }) => void) | null

  const listAgents = vi.fn<() => Promise<AgentState[]>>()
  const getAgentBuffer = vi.fn<(agentId: string) => Promise<string[]>>()
  const createAgent = vi.fn<(payload: CreateAgentPayload) => Promise<AgentState>>()
  const restartAgent = vi.fn<(agentId: string) => Promise<AgentState | null>>()

  beforeEach(() => {
    outputHandler = null
    statusHandler = null
    restartedHandler = null
    vi.clearAllMocks()

    listAgents.mockResolvedValue([])
    getAgentBuffer.mockResolvedValue([])
    createAgent.mockResolvedValue(makeAgent('new-agent', { status: 'running' }))
    restartAgent.mockResolvedValue(makeAgent('agent-1', { status: 'running', restartCount: 1 }))

    window.hydra = {
      ...window.hydra,
      listAgents,
      getAgentBuffer,
      createAgent,
      restartAgent,
      killAgent: vi.fn().mockResolvedValue(true),
      removeAgent: vi.fn().mockResolvedValue(true),
      toggleYolo: vi.fn().mockResolvedValue(null),
      renameAgent: vi.fn().mockResolvedValue(null),
      setAgentModel: vi.fn().mockResolvedValue(null),
      sendInput: vi.fn(),
      sendRawInput: vi.fn(),
      resizeAgent: vi.fn(),
      broadcast: vi.fn().mockResolvedValue([]),
      onAgentOutput: vi.fn().mockImplementation((cb: (payload: AgentOutputPayload) => void) => {
        outputHandler = cb
        return () => undefined
      }),
      onAgentStatus: vi.fn().mockImplementation((cb: (payload: AgentStatusPayload) => void) => {
        statusHandler = cb
        return () => undefined
      }),
      onAgentRestarted: vi.fn().mockImplementation((cb: (payload: { agentId: string }) => void) => {
        restartedHandler = cb
        return () => undefined
      })
    } as typeof window.hydra
  })

  it('preserves output that arrives before restart resolves', async () => {
    listAgents.mockResolvedValue([makeAgent('agent-1', { status: 'idle' })])
    getAgentBuffer.mockResolvedValue([])

    let resolveRestart: ((state: AgentState) => void) | null = null
    restartAgent.mockImplementation(
      () =>
        new Promise<AgentState>((resolve) => {
          resolveRestart = resolve
        })
    )

    const { result } = renderHook(() => useAgents('agent-1'))

    await waitFor(() => {
      expect(result.current.agentList.length).toBe(1)
    })

    let restartPromise: Promise<void> | null = null
    act(() => {
      restartPromise = result.current.restartAgent('agent-1')
    })

    act(() => {
      outputHandler?.({ agentId: 'agent-1', data: 'boot output' })
      statusHandler?.({ agentId: 'agent-1', status: 'running', sessionId: null })
    })

    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 25))
    })

    await act(async () => {
      resolveRestart?.(makeAgent('agent-1', { status: 'running', restartCount: 1 }))
      await restartPromise
    })

    await waitFor(() => {
      expect(result.current.agents.get('agent-1')?.rawOutput).toContain('boot output')
    })
  })

  it('backfills terminal output after create from daemon buffer', async () => {
    const created = makeAgent('created-1', { status: 'running' })
    createAgent.mockResolvedValue(created)
    getAgentBuffer.mockImplementation(async (agentId: string) =>
      agentId === created.id ? ['line one\nline two'] : []
    )

    const { result } = renderHook(() => useAgents())

    await act(async () => {
      await result.current.createAgent({
        name: 'Created',
        projectDir: '/tmp/project',
        provider: 'claude',
        model: 'sonnet',
        yolo: false,
        initialPrompt: '',
        resumeSessionId: null
      })
    })

    expect(result.current.agents.get(created.id)?.rawOutput).toBe('line one\nline two')
  })

  it('preserves live output that arrives before create buffer backfill resolves', async () => {
    const created = makeAgent('created-1', { status: 'running', provider: 'codex', model: 'gpt-5.4' })
    createAgent.mockResolvedValue(created)

    let resolveBuffer: ((value: string[]) => void) | null = null
    getAgentBuffer.mockImplementation(
      () =>
        new Promise<string[]>((resolve) => {
          resolveBuffer = resolve
        })
    )

    const { result } = renderHook(() => useAgents())

    let createPromise: Promise<AgentState> | null = null
    act(() => {
      createPromise = result.current.createAgent({
        name: 'Created',
        projectDir: '/tmp/project',
        provider: 'codex',
        model: 'gpt-5.4',
        yolo: false,
        initialPrompt: '',
        resumeSessionId: null
      })
    })

    act(() => {
      outputHandler?.({ agentId: created.id, data: 'live codex frame' })
    })

    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 25))
    })

    await act(async () => {
      resolveBuffer?.(['older snapshot'])
      await createPromise
    })

    expect(result.current.agents.get(created.id)?.rawOutput).toBe('live codex frame')
  })

  // G2b oddity (a): a restart started elsewhere (CLI, TUI, MCP, YOLO/model switch) left the old session on screen under the new one,
  // because only the UI's own restart button cleared the output.
  it('drops the old screen when the daemon announces a restart and keeps what the new process writes', async () => {
    listAgents.mockResolvedValue([makeAgent('agent-1', { status: 'running' })])
    getAgentBuffer.mockResolvedValue(['old session text'])
    const { result } = renderHook(() => useAgents('agent-1'))
    await waitFor(() => expect(result.current.agents.get('agent-1')?.rawOutput).toBe('old session text'))

    // Output that is still queued (not flushed yet) belongs to the old process too.
    act(() => outputHandler?.({ agentId: 'agent-1', data: ' + old tail' }))
    act(() => restartedHandler?.({ agentId: 'agent-1' }))
    expect(result.current.agents.get('agent-1')?.rawOutput).toBe('')

    act(() => outputHandler?.({ agentId: 'agent-1', data: 'new session text' }))
    await waitFor(() => expect(result.current.agents.get('agent-1')?.rawOutput).toBe('new session text'))
    // An unknown agent is ignored.
    act(() => restartedHandler?.({ agentId: 'ghost' }))
    expect(result.current.agents.get('agent-1')?.rawOutput).toBe('new session text')
  })

  // G2b oddity (b): output (and a daemon-side `/rename ...` echo) of one agent must never show up in another agent's terminal.
  it('keeps the output of different agents apart, also across restarts and buffer reloads', async () => {
    listAgents.mockResolvedValue([makeAgent('a', { status: 'running' }), makeAgent('b', { status: 'running' })])
    getAgentBuffer.mockImplementation(async (id: string) => [`buffer of ${id}`])
    const { result } = renderHook(() => useAgents('a'))
    await waitFor(() => expect(result.current.agentList.length).toBe(2))
    act(() => {
      outputHandler?.({ agentId: 'a', data: '\n/rename alpha' })
      outputHandler?.({ agentId: 'b', data: '\n/rename beta' })
    })
    act(() => restartedHandler?.({ agentId: 'b' }))
    act(() => outputHandler?.({ agentId: 'b', data: 'beta again' }))
    await waitFor(() => expect(result.current.agents.get('b')?.rawOutput).toBe('beta again'))
    const a = result.current.agents.get('a')?.rawOutput ?? ''
    expect(a).toContain('/rename alpha')
    expect(a).not.toContain('beta')
    expect(result.current.agents.get('b')?.rawOutput).not.toContain('alpha')
  })
})
