// @vitest-environment jsdom
import { render, waitFor } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'

// The stand-in editor hands its mount callback a fake Monaco and records the registered commands.
const commands: Array<() => void> = []
vi.mock('@monaco-editor/react', () => ({
  Editor: (props: { onMount: (editor: unknown, monaco: unknown) => void }) => {
    const monaco = { KeyMod: { CtrlCmd: 2048 }, KeyCode: { KeyS: 49, KeyE: 35 }, editor: { defineTheme: vi.fn(), setTheme: vi.fn() } }
    const editor = { addCommand: (_key: number, run: () => void) => commands.push(run), focus: vi.fn() }
    if (commands.length === 0) props.onMount(editor, monaco)
    return null
  },
  loader: { config: vi.fn() }
}))
vi.mock('../../lib/monacoSetup', () => ({}))
vi.mock('../../lib/monacoThemes', () => ({ THEME_MAP: { dark: { name: 'dark', data: {} } } }))

import { CodeEditor } from './CodeEditor'

describe('CodeEditor', () => {
  it('Ctrl+S calls the latest onSave, not the one from the first render', async () => {
    commands.length = 0
    const first = vi.fn()
    const second = vi.fn()
    const view = render(<CodeEditor filePath="a.md" content="old" theme="dark" onChange={vi.fn()} onSave={first} />)
    await waitFor(() => expect(commands.length).toBe(2))
    view.rerender(<CodeEditor filePath="a.md" content="new" theme="dark" onChange={vi.fn()} onSave={second} />)
    commands[0]() // the Ctrl+S command registered at mount
    expect(second).toHaveBeenCalledTimes(1)
    expect(first).not.toHaveBeenCalled()
  })
})
