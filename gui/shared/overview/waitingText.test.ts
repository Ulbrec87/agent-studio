import { describe, expect, it } from 'vitest'
import { lastAskFromTail } from './waitingText'

const DIR = '/work/proj'
let n = 0
const asst = (blocks: unknown[], extra: Record<string, unknown> = {}): string =>
  JSON.stringify({ type: 'assistant', message: { id: `m${++n}`, content: blocks }, ...extra })
const use = (name: string, input: unknown, id = `t${++n}`): unknown => ({ type: 'tool_use', id, name, input })
const result = (id: string): string =>
  JSON.stringify({ type: 'user', message: { content: [{ type: 'tool_result', tool_use_id: id }] } })
const prompt = (text: string): string => JSON.stringify({ type: 'user', message: { content: text } })
const ask = (...lines: string[]): string | null => lastAskFromTail(lines.join('\n'), true, DIR)

describe('lastAskFromTail', () => {
  it('describes each tool kind', () => {
    expect(ask(asst([use('Bash', { command: '\n  npm test -- --run\nls' })]))).toBe('Run: npm test -- --run')
    expect(ask(asst([use('Edit', { file_path: '/work/proj/src/a.ts' })]))).toBe('Edit: src/a.ts')
    expect(ask(asst([use('MultiEdit', { file_path: '/work/proj/b.ts' })]))).toBe('Edit: b.ts')
    expect(ask(asst([use('NotebookEdit', { notebook_path: '/work/proj/n.ipynb' })]))).toBe('Edit: n.ipynb')
    expect(ask(asst([use('Write', { file_path: '/elsewhere/x.md' })]))).toBe('Write: /elsewhere/x.md')
    expect(ask(asst([use('Read', { file_path: '/work/proj/r.md' })]))).toBe('Read: r.md')
    expect(ask(asst([use('WebFetch', { url: 'https://x.dev' })]))).toBe('Fetch: https://x.dev')
    expect(ask(asst([use('WebSearch', { query: 'vitest mock' })]))).toBe('Search: vitest mock')
    expect(ask(asst([use('ExitPlanMode', { plan: 'p' })]))).toBe('Approve the plan')
    expect(ask(asst([use('mcp__x__do', {})]))).toBe('Use: mcp__x__do')
  })

  it('uses the first question and counts the rest', () => {
    expect(ask(asst([use('AskUserQuestion', { questions: [{ question: 'Which DB?' }] })]))).toBe('Which DB?')
    expect(
      ask(asst([use('AskUserQuestion', { questions: [{ question: 'A?' }, { question: 'B?' }, { question: 'C?' }] })]))
    ).toBe('A? (+2 more)')
    expect(ask(asst([use('AskUserQuestion', { questions: [] })]))).toBeNull()
  })

  it('gives nothing for subagent tools or missing input', () => {
    expect(ask(asst([use('Task', { prompt: 'x' })]))).toBeNull()
    expect(ask(asst([use('Agent', {})]))).toBeNull()
    expect(ask(asst([use('Bash', {})]))).toBeNull()
  })

  it('gives nothing once the tool has a result', () => {
    expect(ask(asst([use('Bash', { command: 'ls' }, 'x1')]), result('x1'))).toBeNull()
  })

  it('ends the wait on a later prompt or assistant reply', () => {
    expect(ask(asst([use('Bash', { command: 'ls' })]), prompt('never mind'))).toBeNull()
    expect(ask(asst([use('Bash', { command: 'ls' })]), asst([{ type: 'text', text: 'ok' }]))).toBeNull()
  })

  it('ignores tool results and meta user lines as prompts', () => {
    const meta = JSON.stringify({ type: 'user', isMeta: true, message: { content: 'note' } })
    expect(ask(asst([use('Bash', { command: 'ls' }, 'x2')]), result('other'), meta)).toBe('Run: ls')
  })

  it('skips sidechain lines', () => {
    expect(
      ask(asst([use('Bash', { command: 'ls' })]), asst([use('Bash', { command: 'rm -rf x' })], { isSidechain: true }))
    ).toBe('Run: ls')
    expect(ask(asst([use('Bash', { command: 'ls' })]), asst([{ type: 'text', text: 'hi' }], { isSidechain: true }))).toBe(
      'Run: ls'
    )
  })

  it('takes the first still-waiting tool of the newest message', () => {
    const line = (block: unknown): string =>
      JSON.stringify({ type: 'assistant', message: { id: 'same', content: [block] } })
    const a = line(use('Bash', { command: 'first' }, 'a'))
    const b = line(use('Bash', { command: 'second' }, 'b'))
    expect(ask(a, b)).toBe('Run: first')
    expect(ask(a, b, result('a'))).toBe('Run: second')
    expect(ask(a, b, result('b'))).toBe('Run: first')
    expect(ask(a, b, result('a'), result('b'))).toBeNull()
  })

  it('looks only at the newest tool use when lines carry no message id', () => {
    const noId = (block: unknown): string => JSON.stringify({ type: 'assistant', message: { content: [block] } })
    expect(ask(noId(use('Bash', { command: 'one' }, 'a')), noId(use('Bash', { command: 'two' }, 'b')))).toBe('Run: two')
    expect(ask(noId(use('Bash', { command: 'one' }, 'a')), noId({ type: 'text', text: 'x' }))).toBeNull()
  })

  it('drops the first line of a partial tail, keeps it for a whole file', () => {
    const line = asst([use('Bash', { command: 'ls' })])
    expect(lastAskFromTail(line, false, DIR)).toBeNull()
    expect(lastAskFromTail(line, true, DIR)).toBe('Run: ls')
    expect(lastAskFromTail(`ial": "cut off"}\n${line}`, false, DIR)).toBe('Run: ls')
  })

  it('skips garbage lines and never throws', () => {
    expect(ask('not json', '[1,2]', 'null', asst([use('Bash', { command: 'ls' })]), '{"type":')).toBe('Run: ls')
    expect(ask('', '{}')).toBeNull()
    expect(lastAskFromTail('', true, DIR)).toBeNull()
    expect(ask(JSON.stringify({ type: 'assistant', message: { content: 'str' } }))).toBeNull()
  })

  it('puts it on one line and cuts to 200 characters', () => {
    const text = ask(asst([use('AskUserQuestion', { questions: [{ question: 'a\tb\u202e  c\n' + 'x'.repeat(300) }] })]))
    expect(text).not.toBeNull()
    expect(Array.from(text as string)).toHaveLength(200)
    expect(text?.endsWith('…')).toBe(true)
    expect(text?.startsWith('a b c x')).toBe(true)
  })
})
