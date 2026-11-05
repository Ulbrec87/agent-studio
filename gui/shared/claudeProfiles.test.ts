import { describe, expect, it } from 'vitest'
import { resolveClaudeProfile } from './claudeProfiles'

const map = {
  '/var/www/test': '/home/me/.claude-work',
  '/home/me/projects/': '/home/me/.claude-alternative',
  '/home/me/projects/personal': ''
}

describe('resolveClaudeProfile', () => {
  it('matches the project dir and its subfolders', () => {
    expect(resolveClaudeProfile('/var/www/test', map)).toBe('/home/me/.claude-work')
    expect(resolveClaudeProfile('/var/www/test/app/', map)).toBe('/home/me/.claude-work')
  })

  it('does not match sibling dirs sharing a name prefix', () => {
    expect(resolveClaudeProfile('/var/www/test-2', map)).toBeNull()
  })

  it('prefers the longest mapped path, including an explicit default', () => {
    expect(resolveClaudeProfile('/home/me/projects/client', map)).toBe('/home/me/.claude-alternative')
    expect(resolveClaudeProfile('/home/me/projects/personal/blog', map)).toBe('')
  })

  it('returns null without a mapping', () => {
    expect(resolveClaudeProfile('/tmp', map)).toBeNull()
    expect(resolveClaudeProfile('/tmp', undefined)).toBeNull()
  })
})
