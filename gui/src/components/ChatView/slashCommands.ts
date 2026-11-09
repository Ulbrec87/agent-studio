import type { ProviderId } from '@shared/types'

/**
 * Built-in slash commands of each agent CLI, offered by the chat box's `/` menu. Agent Studio extension: Hydra only offered
 * `/clear` (+ `/rename` for Claude, `/model` for Codex). Static lists of the CLIs' own commands; the text is typed into the
 * agent's terminal as is, so a command a given CLI version lacks just shows its own "unknown command" message.
 * Custom commands (`~/.claude/commands`, `.claude/commands`) are not listed (would need a daemon scan).
 */
export interface ProviderCommand {
  name: string
  description: string
  /** Extra words for the filter. */
  keywords?: string
}

const CLAUDE: ProviderCommand[] = [
  { name: 'clear', description: 'Clear the conversation history', keywords: 'reset context' },
  { name: 'compact', description: 'Compact the conversation, optionally with focus instructions', keywords: 'summarize context' },
  { name: 'rename', description: 'Rename the current Claude session', keywords: 'title session' },
  { name: 'model', description: 'Switch the Claude model', keywords: 'opus sonnet haiku' },
  { name: 'context', description: 'Show how the context window is used', keywords: 'tokens' },
  { name: 'cost', description: 'Show token usage and cost of this session', keywords: 'usage tokens' },
  { name: 'usage', description: 'Show plan usage limits', keywords: 'limits quota' },
  { name: 'status', description: 'Show version, model, account and connectivity' },
  { name: 'resume', description: 'Resume a previous conversation', keywords: 'session history' },
  { name: 'rewind', description: 'Rewind the conversation and/or code to an earlier point', keywords: 'undo checkpoint' },
  { name: 'review', description: 'Ask for a code review', keywords: 'pr diff' },
  { name: 'init', description: 'Create a CLAUDE.md guide for this project', keywords: 'claude.md setup' },
  { name: 'memory', description: 'Edit CLAUDE.md memory files' },
  { name: 'agents', description: 'Manage custom subagents', keywords: 'subagent' },
  { name: 'mcp', description: 'Manage MCP server connections', keywords: 'servers tools' },
  { name: 'permissions', description: 'View or update tool permissions', keywords: 'allow deny' },
  { name: 'hooks', description: 'Manage hook configurations' },
  { name: 'config', description: 'Open the settings', keywords: 'settings preferences' },
  { name: 'add-dir', description: 'Add another working directory', keywords: 'folder' },
  { name: 'export', description: 'Export the conversation to a file or the clipboard' },
  { name: 'todos', description: 'List the current todo items', keywords: 'tasks' },
  { name: 'doctor', description: 'Check the health of the Claude Code installation' },
  { name: 'help', description: 'Show help and the available commands' }
]

const CODEX: ProviderCommand[] = [
  { name: 'model', description: 'Choose the model and reasoning effort', keywords: 'switch codex' },
  { name: 'approvals', description: 'Choose what Codex may do without approval', keywords: 'permissions sandbox' },
  { name: 'review', description: 'Review the current changes', keywords: 'diff code review' },
  { name: 'new', description: 'Start a new chat in this session', keywords: 'reset' },
  { name: 'compact', description: 'Summarize the conversation to free context', keywords: 'context' },
  { name: 'clear', description: 'Clear the conversation context', keywords: 'reset' },
  { name: 'diff', description: 'Show the git diff, including untracked files', keywords: 'changes' },
  { name: 'mention', description: 'Mention a file', keywords: 'file attach' },
  { name: 'init', description: 'Create an AGENTS.md with instructions for Codex', keywords: 'agents.md setup' },
  { name: 'status', description: 'Show the session configuration and token usage', keywords: 'usage' },
  { name: 'mcp', description: 'List the configured MCP tools', keywords: 'servers tools' },
  { name: 'logout', description: 'Log out of Codex' },
  { name: 'quit', description: 'Exit Codex', keywords: 'exit' }
]

const OPENCODE: ProviderCommand[] = [
  { name: 'new', description: 'Start a new session', keywords: 'clear reset' },
  { name: 'sessions', description: 'List and switch sessions', keywords: 'resume history' },
  { name: 'models', description: 'List and switch models', keywords: 'model' },
  { name: 'compact', description: 'Summarize the session to free context', keywords: 'context' },
  { name: 'undo', description: 'Undo the last message and its file changes' },
  { name: 'redo', description: 'Redo an undone message' },
  { name: 'share', description: 'Share the current session', keywords: 'link' },
  { name: 'unshare', description: 'Stop sharing the current session' },
  { name: 'init', description: 'Create or update AGENTS.md', keywords: 'agents.md setup' },
  { name: 'export', description: 'Export the conversation to Markdown' },
  { name: 'details', description: 'Toggle tool execution details' },
  { name: 'themes', description: 'List the themes' },
  { name: 'help', description: 'Show the help' }
]

export function providerCommands(provider: ProviderId): ProviderCommand[] {
  switch (provider) {
    case 'claude':
      return CLAUDE
    case 'codex':
      return CODEX
    case 'opencode':
      return OPENCODE
    default:
      return []
  }
}
