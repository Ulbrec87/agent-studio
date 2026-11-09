// @vitest-environment jsdom
import { afterEach, describe, expect, it } from 'vitest'
import { combineFileContents, mapGitBranches, mapGitLog, mapGitStatus, pathsToStage, type ProtoGitStatus } from './gitApi'
import { makeBridge } from './testKit'
import type { DaemonTransport } from './daemonApi'

const file = (path: string, f: Partial<{ staged: boolean; unstaged: boolean; untracked: boolean; conflicted: boolean }>): ProtoGitStatus['files'][number] => ({
  path, origPath: null, kind: 'M', index: null, worktree: null, staged: false, unstaged: false, untracked: false, conflicted: false, ...f
})
const STATUS: ProtoGitStatus = {
  isRepo: true, branch: 'main', detached: false, head: null, upstream: null, ahead: 2, behind: 1,
  files: [
    file('a.txt', { staged: true }),
    file('b.txt', { unstaged: true }),
    file('c.txt', { staged: true, unstaged: true }),
    file('d.txt', { untracked: true }),
    file('e.txt', { conflicted: true })
  ]
}

let transport: DaemonTransport | null = null
afterEach(() => transport?.dispose())
async function setup(configure?: Parameters<typeof makeBridge>[0]) {
  const made = await makeBridge(configure)
  transport = made.transport
  return made
}

describe('git mappers', () => {
  it('status becomes three path lists; a folder that is no repository is empty', () => {
    expect(mapGitStatus(STATUS)).toEqual({
      branch: 'main', ahead: 2, behind: 1, staged: ['a.txt', 'c.txt'], modified: ['b.txt', 'c.txt', 'e.txt'], untracked: ['d.txt']
    })
    expect(mapGitStatus({ ...STATUS, isRepo: false, branch: 'x' })).toEqual({ branch: '', ahead: 0, behind: 0, modified: [], staged: [], untracked: [] })
    expect(mapGitStatus({ ...STATUS, detached: true, branch: '' }).branch).toBe('HEAD')
  })

  it('log uses the 8 character hash, branches are local only, stage-all skips fully staged files', () => {
    const commit = { hash: 'a'.repeat(40), shortHash: 'aaaaaaaa', message: 'm', author: 'me', date: '2026-01-01T00:00:00+00:00' }
    expect(mapGitLog([commit])).toEqual([{ hash: 'aaaaaaaa', message: 'm', author: 'me', date: commit.date }])
    const b = (name: string, isRemote: boolean) => ({ name, isCurrent: false, isRemote, upstream: null, aheadOfUpstream: 0, behindUpstream: 0 })
    expect(mapGitBranches([b('main', false), b('origin/main', true)]).map((x) => x.name)).toEqual(['main'])
    expect(pathsToStage(STATUS)).toEqual(['b.txt', 'c.txt', 'd.txt', 'e.txt'])
  })

  it('file contents are HEAD versus the work tree', () => {
    const head = { original: 'old', modified: 'indexed', language: 'rust', binary: false }
    const work = { original: 'indexed', modified: 'new', language: 'rust', binary: false }
    expect(combineFileContents(head, work)).toEqual({ original: 'old', modified: 'new', language: 'rust', binary: false })
    expect(combineFileContents(head, { ...work, binary: true }).binary).toBe(true)
  })
})

describe('git methods', () => {
  it('read methods map to the Git* requests', async () => {
    const { raw, api } = await setup((r) => {
      r.handlers.set('git_status', () => ({ type: 'git_status', payload: STATUS }))
      r.handlers.set('git_log', () => ({ type: 'git_log', payload: [] }))
      r.handlers.set('git_diff', () => ({ type: 'text', payload: 'diff --git' }))
      r.handlers.set('git_diff_stats', () => ({ type: 'git_diff_stats', payload: { files: 1, additions: 2, deletions: 3 } }))
    })
    expect((await api.getGitStatus('/p')).staged).toEqual(['a.txt', 'c.txt'])
    expect(await api.getGitLog('/p')).toEqual([])
    expect(await api.getGitDiff('/p', 'a.txt')).toBe('diff --git')
    await api.getGitDiff('/p')
    expect(await api.getGitDiffStats('/p')).toEqual({ files: 1, additions: 2, deletions: 3 })
    expect(raw.of('git_log')[0].payload).toEqual({ dir: '/p', limit: 20 })
    expect(raw.of('git_diff').map((r) => r.payload)).toEqual([
      { dir: '/p', file: 'a.txt', staged: false },
      { dir: '/p', staged: false }
    ])
  })

  it('commit with files commits just them; without files it stages everything first', async () => {
    const { raw, api } = await setup((r) => {
      r.handlers.set('git_status', () => ({ type: 'git_status', payload: STATUS }))
      r.handlers.set('git_stage', () => ({ type: 'ok' }))
      r.handlers.set('git_commit', () => ({ type: 'text', payload: 'abc1234' }))
    })
    expect(await api.gitCommit('/p', '  fix things  ', ['a.txt'])).toBe('abc1234')
    expect(raw.requests.map((r) => r.type)).toEqual(['git_commit'])
    expect(raw.of('git_commit')[0].payload).toEqual({ dir: '/p', message: 'fix things', files: ['a.txt'] })
    raw.requests.length = 0
    expect(await api.gitCommit('/p', 'all')).toBe('abc1234')
    expect(raw.requests.map((r) => r.type)).toEqual(['git_status', 'git_stage', 'git_commit'])
    expect(raw.of('git_stage')[0].payload).toEqual({ dir: '/p', files: ['b.txt', 'c.txt', 'd.txt', 'e.txt'] })
    expect(raw.of('git_commit')[0].payload).toEqual({ dir: '/p', message: 'all', files: [] })
  })

  it('commit without changes still reaches git (which reports "nothing to commit")', async () => {
    const { raw, api } = await setup((r) => {
      r.handlers.set('git_status', () => ({ type: 'git_status', payload: { ...STATUS, files: [] } }))
      r.handlers.set('git_commit', () => Promise.reject('nothing to commit, working tree clean'))
    })
    await expect(api.gitCommit('/p', 'x')).rejects.toThrow('nothing to commit')
    expect(raw.of('git_stage')).toHaveLength(0)
  })

  it('push, branches, checkout, create branch and worktrees', async () => {
    const { raw, api } = await setup((r) => {
      r.handlers.set('git_push', () => ({ type: 'text', payload: 'pushed' }))
      r.handlers.set('git_branches', () => ({ type: 'git_branches', payload: [{ name: 'main', isCurrent: true, isRemote: false, upstream: null, aheadOfUpstream: 0, behindUpstream: 0 }] }))
      r.handlers.set('git_checkout', () => ({ type: 'ok' }))
      r.handlers.set('git_create_branch', () => ({ type: 'ok' }))
      r.handlers.set('git_worktree_create', () => ({ type: 'git_worktree', payload: { path: '/p/.worktrees/feat-x', branch: 'feat/x' } }))
      r.handlers.set('git_worktree_remove', () => ({ type: 'ok' }))
    })
    expect(await api.gitPush('/p')).toBeUndefined()
    expect((await api.gitListBranches('/p'))[0].name).toBe('main')
    expect(await api.gitCheckout('/p', 'dev')).toBeUndefined()
    await api.gitCreateBranch('/p', 'feat/y')
    await api.gitCreateBranch('/p', 'feat/z', 'main')
    expect(raw.of('git_create_branch').map((r) => r.payload)).toEqual([{ dir: '/p', name: 'feat/y' }, { dir: '/p', name: 'feat/z', from: 'main' }])
    expect(await api.gitWorktreeCreate('/p', 'feat/x')).toEqual({ worktreePath: '/p/.worktrees/feat-x', branch: 'feat/x' })
    await api.gitWorktreeRemove('/p', '/p/.worktrees/feat-x', 'feat/x')
    expect(raw.of('git_worktree_remove')[0].payload).toEqual({ dir: '/p', path: '/p/.worktrees/feat-x', delete_branch: 'feat/x', force: true })
  })

  it('file contents ask both sides and PR calls pass the identifier as text', async () => {
    const { raw, api } = await setup((r) => {
      r.handlers.set('git_file_contents', (p) => {
        const staged = (p as { staged: boolean }).staged
        return { type: 'git_file_contents', payload: staged ? { original: 'HEAD', modified: 'INDEX', language: 'rust', binary: false } : { original: 'INDEX', modified: 'WORK', language: 'rust', binary: false } }
      })
      r.handlers.set('git_pr_fetch', () => Promise.reject('gh: no git remotes found'))
      r.handlers.set('git_pr_file_diff', () => ({ type: 'text', payload: 'patch' }))
    })
    expect(await api.gitFileContents('/p', 'a.rs')).toMatchObject({ original: 'HEAD', modified: 'WORK', language: 'rust' })
    expect(raw.of('git_file_contents').map((r) => (r.payload as { staged: boolean }).staged).sort()).toEqual([false, true])
    await expect(api.gitFetchPr('/p', '#12')).rejects.toThrow('no git remotes')
    expect(raw.of('git_pr_fetch')[0].payload).toEqual({ dir: '/p', pr: '#12' })
    expect(await api.gitPrFileDiff('/p', 12, 'a.rs')).toBe('patch')
    expect(raw.of('git_pr_file_diff')[0].payload).toEqual({ dir: '/p', pr: '12', path: 'a.rs' })
  })
})
