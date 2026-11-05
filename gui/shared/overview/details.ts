// Adapted from agent-lookout (MIT, © 2026 Idris Olubisi):
//   src/collector/adapters/claude-code/transcript/lastSaid.ts, lastUsage.ts, src/core/text.ts

import type { AgentDetails } from './types'
import { blocksOf, isRecord, linesOf, nonEmpty, type Json } from './waitingText'

/** The longest last message kept, in characters (the end of the text is kept). */
export const MAX_LAST_MESSAGE_LENGTH = 2000

/** What goes between the parts of one message: a blank line. */
const PART_BREAK = '\n\n'

export type NothingSaid = 'nothing-yet' | 'too-far-back'

export type LastSaid = { text: string; startCut?: true } | { text: null; reason: NothingSaid }

/** The `text` blocks with anything in them of a line of the session's own assistant reply. */
function textsOf(line: Json): string[] {
  if (line.type !== 'assistant' || line.isSidechain === true || line.isMeta === true) return []
  const texts: string[] = []
  for (const block of blocksOf(line)) {
    const text = block.type === 'text' ? nonEmpty(block.text) : null
    if (text !== null) texts.push(text)
  }
  return texts
}

function messageIdOf(line: Json): string | null {
  return isRecord(line.message) ? nonEmpty(line.message.id) : null
}

/**
 * What the session last said: the `text` blocks of its newest assistant reply
 * (all lines sharing one `message.id`), joined with blank lines. Subagent and
 * meta lines, thinking, tool uses and results are never taken.
 */
export function lastSaidInTail(tail: string, fromStart: boolean): LastSaid {
  const lines = linesOf(tail, fromStart)
  const newest = lines.findLastIndex((line) => textsOf(line).length > 0)
  if (newest === -1) return { text: null, reason: fromStart ? 'nothing-yet' : 'too-far-back' }

  const parts = [textsOf(lines[newest])]
  const messageId = messageIdOf(lines[newest])
  if (messageId === null) return { text: parts[0].join(PART_BREAK) }
  let index = newest - 1
  for (; index >= 0; index -= 1) {
    const line = lines[index]
    if (line.isSidechain === true) continue
    if (line.type === 'user') break
    if (line.type !== 'assistant') continue
    if (messageIdOf(line) !== messageId) break
    parts.unshift(textsOf(line))
  }
  const text = parts.flat().join(PART_BREAK)
  // Nothing before the message stopped it, and the line dropped from the tail's start may be part of it.
  return index < 0 && !fromStart ? { text, startCut: true } : { text }
}

// Control characters except tab and line feed, line/paragraph separators and bidi reordering marks.
const UNSAFE_CHARS = new RegExp('[\\u0000-\\u0008\\u000b-\\u001f\\u007f-\\u009f\\u2028\\u2029\\u061c\\u200e\\u200f\\u202a-\\u202e\\u2066-\\u2069]', 'g')

/** Makes text fit to show: line breaks kept, unsafe characters dropped, the last 2000 characters kept. */
export function messageText(text: string, startCut = false): { text: string; truncated: boolean } {
  const clean = text.replace(/\r\n?/g, '\n').replace(UNSAFE_CHARS, '').trim()
  const chars = Array.from(clean)
  if (chars.length <= MAX_LAST_MESSAGE_LENGTH) return { text: clean, truncated: startCut }
  return { text: chars.slice(chars.length - MAX_LAST_MESSAGE_LENGTH).join('').trimStart(), truncated: true }
}

// --- Token counts of the newest reply ---------------------------------------------------------

const ASSISTANT_HINT = '"assistant"'
const USAGE_HINT = '"usage"'
const COMPACT_HINT = '"isCompactSummary"'
const SYNTHETIC_MODEL = '<synthetic>'

function isCount(value: unknown): value is number {
  return typeof value === 'number' && Number.isSafeInteger(value) && value >= 0
}

function parsed(piece: string): Json | null {
  try {
    const value: unknown = JSON.parse(piece)
    return isRecord(value) ? value : null
  } catch {
    return null
  }
}

function isOwnReply(line: Json): boolean {
  if (line.type !== 'assistant' || line.isSidechain === true || line.isApiErrorMessage === true) return false
  return !(isRecord(line.message) && line.message.model === SYNTHETIC_MODEL)
}

function isCompaction(line: Json): boolean {
  return line.type === 'user' && line.isCompactSummary === true && line.isSidechain !== true
}

/**
 * `input_tokens` is only the uncached part of the prompt; the whole prompt is
 * that plus cache creation plus cache reads. Anything not a whole number means
 * the counts are not believed.
 */
function countsOf(usage: unknown): NonNullable<AgentDetails['tokens']> | null {
  if (!isRecord(usage)) return null
  const uncached = usage.input_tokens
  const written = usage.cache_creation_input_tokens
  const read = usage.cache_read_input_tokens
  const output = usage.output_tokens
  if (!isCount(uncached) || !isCount(output)) return null
  if (written !== undefined && !isCount(written)) return null
  if (read !== undefined && !isCount(read)) return null
  const input = uncached + (written ?? 0) + (read ?? 0)
  if (!Number.isSafeInteger(input)) return null
  return read === undefined ? { input, output } : { input, cached: read, output }
}

/** Token counts of the session's newest reply, or null when there are none that could be right. */
export function lastUsageInTail(tail: string, fromStart: boolean): AgentDetails['tokens'] {
  const pieces = tail.split('\n')
  const first = fromStart ? 0 : 1
  for (let index = pieces.length - 1; index >= first; index -= 1) {
    const piece = pieces[index] ?? ''
    const named = piece.includes(COMPACT_HINT) || (piece.includes(ASSISTANT_HINT) && piece.includes(USAGE_HINT))
    if (!named) continue
    const line = parsed(piece)
    if (line === null) continue
    // Compacted since the newest reply: its counts are of a context no longer in use.
    if (isCompaction(line)) return null
    if (!isOwnReply(line) || !isRecord(line.message) || line.message.usage === undefined) continue
    return countsOf(line.message.usage)
  }
  return null
}

const NOTHING_REASON: Record<NothingSaid, string> = {
  'nothing-yet': 'Nothing said yet',
  'too-far-back': 'The last reply is further back than the part of the transcript that is read'
}

/** Details of a Claude session from the end of its transcript. Never throws. */
export function detailsFromTail(tail: string, fromStart: boolean): AgentDetails {
  try {
    const said = lastSaidInTail(tail, fromStart)
    return {
      lastMessage:
        said.text === null
          ? { unavailable: NOTHING_REASON[said.reason] }
          : messageText(said.text, said.startCut === true),
      tokens: lastUsageInTail(tail, fromStart)
    }
  } catch {
    return { lastMessage: { unavailable: 'The transcript could not be read' }, tokens: null }
  }
}
