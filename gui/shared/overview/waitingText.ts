// Adapted from agent-lookout (MIT, © 2026 Idris Olubisi):
//   src/collector/adapters/claude-code/transcript/lastAsk.ts, transcriptLines.ts, src/core/text.ts

/** The longest text kept of what a waiting session asks, in characters. */
export const MAX_WAITING_TEXT_LENGTH = 200

export type Json = Record<string, unknown>

/** One `tool_use` block of the session's own, in transcript order. */
interface ToolUse {
  id: string
  name: string
  input: Json
  /** `message.id` of the assistant message it is part of, when the line gives one. */
  messageId: string | null
}

export function isRecord(value: unknown): value is Json {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
}

export function nonEmpty(value: unknown): string | null {
  return typeof value === 'string' && value.trim() !== '' ? value : null
}

export function blocksOf(line: Json): Json[] {
  const message = line.message
  if (!isRecord(message) || !Array.isArray(message.content)) return []
  return message.content.filter(isRecord)
}

/** Parses the tail. The first line is cut short unless the tail is the whole file, so it is dropped then. */
export function linesOf(tail: string, wholeFile: boolean): Json[] {
  const pieces = tail.split('\n')
  if (!wholeFile) pieces.shift()
  const lines: Json[] = []
  for (const piece of pieces) {
    if (piece.trim() === '') continue
    try {
      const value: unknown = JSON.parse(piece)
      if (isRecord(value)) lines.push(value)
    } catch {
      // Not a readable line; the next one may be.
    }
  }
  return lines
}

/** Whether a `user` line is something the person said rather than a tool result or a meta note. */
function isPrompt(line: Json): boolean {
  if (line.isMeta === true) return false
  const message = isRecord(line.message) ? line.message : null
  if (message === null) return false
  if (typeof message.content === 'string') return message.content.trim() !== ''
  return blocksOf(line).some((block) => block.type === 'text')
}

/**
 * The tool use the session waits on, or null. One message can ask for several
 * tools; Claude Code asks in order, so the one waited on is the first without a
 * result. A line with no `message.id` cannot be tied to the same message, so then
 * only the newest tool use counts and any later assistant line means it moved on.
 */
function pendingToolUse(lines: readonly Json[]): ToolUse | null {
  const uses: ToolUse[] = []
  const answered = new Set<string>()
  let movedOn = false
  for (const line of lines) {
    if (line.isSidechain === true) continue
    const message = isRecord(line.message) ? line.message : null
    if (line.type === 'assistant') {
      const messageId = nonEmpty(message?.id)
      const newest = uses.at(-1)
      if (newest !== undefined && (messageId === null || messageId !== newest.messageId)) movedOn = true
      for (const block of blocksOf(line)) {
        if (block.type !== 'tool_use') continue
        const id = nonEmpty(block.id)
        const name = nonEmpty(block.name)
        if (id === null || name === null) continue
        uses.push({ id, name, input: isRecord(block.input) ? block.input : {}, messageId })
        movedOn = false
      }
    } else if (line.type === 'user') {
      for (const block of blocksOf(line)) {
        const id = block.type === 'tool_result' ? nonEmpty(block.tool_use_id) : null
        if (id !== null) answered.add(id)
      }
      if (uses.length > 0 && isPrompt(line)) movedOn = true
    }
  }
  const newest = uses.at(-1)
  if (newest === undefined || movedOn) return null
  if (newest.messageId === null) return answered.has(newest.id) ? null : newest
  return uses.find((use) => use.messageId === newest.messageId && !answered.has(use.id)) ?? null
}

/** A path relative to the project dir when inside it, whole otherwise. */
function shortPath(file: string, projectDir: string): string {
  const folder = projectDir.replace(/[/\\]+$/, '')
  if (folder === '') return file
  for (const separator of ['/', '\\']) {
    const prefix = `${folder}${separator}`
    if (file.startsWith(prefix) && file.length > prefix.length) return file.slice(prefix.length)
  }
  return file
}

const FILE_TOOLS: Readonly<Record<string, { word: string; field: string }>> = {
  Edit: { word: 'Edit', field: 'file_path' },
  MultiEdit: { word: 'Edit', field: 'file_path' },
  NotebookEdit: { word: 'Edit', field: 'notebook_path' },
  Write: { word: 'Write', field: 'file_path' },
  Read: { word: 'Read', field: 'file_path' }
}

/** What one tool use asks, in a few words; null for unknown input or a subagent start. */
function describeToolUse(name: string, input: Json, projectDir: string): string | null {
  if (name === 'AskUserQuestion') {
    const questions = Array.isArray(input.questions) ? input.questions : []
    const first: unknown = questions[0]
    const question = isRecord(first) ? nonEmpty(first.question) : null
    if (question === null) return null
    return questions.length > 1 ? `${question} (+${questions.length - 1} more)` : question
  }
  if (name === 'Bash' || name === 'PowerShell') {
    const command = nonEmpty(input.command)
    const line = command?.split(/\r?\n/).find((l) => l.trim() !== '')
    return line ? `Run: ${line.trim()}` : null
  }
  const fileTool = Object.hasOwn(FILE_TOOLS, name) ? FILE_TOOLS[name] : undefined
  if (fileTool) {
    const file = nonEmpty(input[fileTool.field])
    return file === null ? null : `${fileTool.word}: ${shortPath(file, projectDir)}`
  }
  if (name === 'WebFetch') {
    const url = nonEmpty(input.url)
    return url === null ? null : `Fetch: ${url}`
  }
  if (name === 'WebSearch') {
    const query = nonEmpty(input.query)
    return query === null ? null : `Search: ${query}`
  }
  if (name === 'ExitPlanMode') return 'Approve the plan'
  if (name === 'Task' || name === 'Agent') return null
  return `Use: ${name}`
}

// Control characters, line/paragraph separators and bidi reordering marks.
const NOT_ON_ONE_LINE = /[\u0000-\u001f\u007f-\u009f\u2028\u2029\u061c\u200e\u200f\u202a-\u202e\u2066-\u2069]/g

/** Makes text stand on one line and cuts it to 200 characters with an ellipsis. */
function oneLine(text: string): string | null {
  const flat = text.replace(NOT_ON_ONE_LINE, ' ').replace(/\s+/g, ' ').trim()
  const chars = Array.from(flat)
  const out = chars.length <= MAX_WAITING_TEXT_LENGTH
    ? flat
    : `${chars.slice(0, MAX_WAITING_TEXT_LENGTH - 1).join('').trimEnd()}…`
  return out || null
}

/**
 * What a waiting Claude Code session is asking, from the tail of its transcript.
 * `wholeFile` says the tail starts at the file start; otherwise its first
 * (possibly partial) line is dropped. Never throws; anything unexpected is null.
 */
export function lastAskFromTail(tail: string, wholeFile: boolean, projectDir: string): string | null {
  try {
    const pending = pendingToolUse(linesOf(tail, wholeFile))
    if (pending === null) return null
    const described = describeToolUse(pending.name, pending.input, projectDir)
    return described === null ? null : oneLine(described)
  } catch {
    return null
  }
}
