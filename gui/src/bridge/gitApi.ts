// Git: Hydra's `GitService` methods on top of the daemon's `Git*` requests.
// Differences handled here: Hydra's status is three path lists (the daemon's is a file list with flags), `gitCommit`
// without files means "commit everything" (the daemon commits what is staged, so everything is staged first), the diff
// viewer wants HEAD versus the work tree (the daemon offers index/work tree and HEAD/index), and branches are local only.
import type { GitBranch, GitCommit, GitDiffStats, GitFileContents, GitPrDiff, GitStatus } from '@shared/types'
import type { BridgeContext, MethodGroup } from './context'
import { compact } from './mappers'
import type { GitCommit as ProtoGitCommit, GitStatus as ProtoGitStatus } from './generated'

export type { ProtoGitStatus }

interface ProtoGitFileContents extends GitFileContents {
  binary: boolean
}

/** Daemon status -> Hydra's `GitStatus` (a folder that is not a repository is an empty status, like Hydra). */
export function mapGitStatus(s: ProtoGitStatus): GitStatus {
  if (!s.isRepo) return { branch: '', ahead: 0, behind: 0, modified: [], staged: [], untracked: [] }
  return {
    branch: s.detached ? 'HEAD' : s.branch,
    ahead: s.ahead,
    behind: s.behind,
    staged: s.files.filter((f) => f.staged).map((f) => f.path),
    modified: s.files.filter((f) => (f.unstaged && !f.untracked) || (f.conflicted && !f.unstaged && !f.staged)).map((f) => f.path),
    untracked: s.files.filter((f) => f.untracked).map((f) => f.path)
  }
}

/** Hydra's `hash` is the first 8 characters. */
export function mapGitLog(commits: ProtoGitCommit[]): GitCommit[] {
  return commits.map((c) => ({ hash: c.shortHash || c.hash.slice(0, 8), message: c.message, author: c.author, date: c.date }))
}

/** Hydra lists local branches only (the panel has no notion of remote ones). */
export function mapGitBranches(branches: GitBranch[]): GitBranch[] {
  return branches.filter((b) => !b.isRemote)
}

/** The paths `git add -A` would stage: everything that is not fully staged already. */
export function pathsToStage(status: ProtoGitStatus): string[] {
  return status.files.filter((f) => f.unstaged || f.untracked || f.conflicted).map((f) => f.path)
}

/**
 * HEAD versus the work tree (Hydra's diff viewer). The daemon's `staged: true` side is HEAD/index and `staged: false` is
 * index/work tree, so the original comes from the first and the modified text from the second.
 */
export function combineFileContents(head: ProtoGitFileContents, work: ProtoGitFileContents): GitFileContents & { binary: boolean } {
  return { original: head.original, modified: work.modified, language: work.language || head.language, binary: head.binary || work.binary }
}

export function createGitMethods(ctx: BridgeContext): MethodGroup {
  const methods: MethodGroup['methods'] = {
    getGitStatus: (async (dir: string) => mapGitStatus(await ctx.call<ProtoGitStatus>('git_status', { dir }, 'git_status'))) as never,
    getGitLog: (async (dir: string, limit?: number) =>
      mapGitLog(await ctx.call<ProtoGitCommit[]>('git_log', { dir, limit: limit ?? 20 }, 'git_log'))) as never,
    getGitDiff: ((dir: string, file?: string) => ctx.call<string>('git_diff', compact({ dir, file, staged: false }), 'text')) as never,
    getGitDiffStats: ((dir: string) => ctx.call<GitDiffStats>('git_diff_stats', { dir }, 'git_diff_stats')) as never,
    gitCommit: (async (dir: string, message: string, files?: string[]) => {
      const text = message.trim()
      if (files && files.length > 0) return ctx.call<string>('git_commit', { dir, message: text, files }, 'text')
      // Hydra: `git add -A` then commit. The daemon commits the index, so stage the rest first.
      const paths = pathsToStage(await ctx.call<ProtoGitStatus>('git_status', { dir }, 'git_status'))
      if (paths.length > 0) await ctx.call('git_stage', { dir, files: paths }, 'ok')
      return ctx.call<string>('git_commit', { dir, message: text, files: [] }, 'text')
    }) as never,
    gitPush: (async (dir: string) => {
      await ctx.call('git_push', { dir }, 'text')
    }) as never,
    gitListBranches: (async (dir: string) => mapGitBranches(await ctx.call<GitBranch[]>('git_branches', { dir }, 'git_branches'))) as never,
    gitCheckout: (async (dir: string, branch: string) => {
      await ctx.call('git_checkout', { dir, branch }, 'ok')
    }) as never,
    gitCreateBranch: (async (dir: string, name: string, startPoint?: string) => {
      await ctx.call('git_create_branch', compact({ dir, name, from: startPoint || undefined }), 'ok')
    }) as never,
    gitWorktreeCreate: (async (dir: string, branch: string) => {
      const wt = await ctx.call<{ path: string }>('git_worktree_create', { dir, branch }, 'git_worktree')
      return { worktreePath: wt.path, branch }
    }) as never,
    gitWorktreeRemove: (async (dir: string, path: string, deleteBranch?: string) => {
      // Hydra removes with --force and deletes the branch with -D.
      await ctx.call('git_worktree_remove', compact({ dir, path, delete_branch: deleteBranch || undefined, force: true }), 'ok')
    }) as never,
    gitFileContents: (async (dir: string, path: string) => {
      const [head, work] = await Promise.all([
        ctx.call<ProtoGitFileContents>('git_file_contents', { dir, path, staged: true }, 'git_file_contents'),
        ctx.call<ProtoGitFileContents>('git_file_contents', { dir, path, staged: false }, 'git_file_contents')
      ])
      return combineFileContents(head, work)
    }) as never,
    gitFetchPr: ((dir: string, pr: string) => ctx.call<GitPrDiff>('git_pr_fetch', { dir, pr }, 'git_pr')) as never,
    gitPrFileDiff: ((dir: string, prNumber: number, path: string) =>
      ctx.call<string>('git_pr_file_diff', { dir, pr: String(prNumber), path }, 'text')) as never
  }
  return { methods }
}

export const GIT_METHODS = [
  'getGitStatus', 'getGitLog', 'getGitDiff', 'getGitDiffStats', 'gitCommit', 'gitPush', 'gitListBranches', 'gitCheckout',
  'gitCreateBranch', 'gitWorktreeCreate', 'gitWorktreeRemove', 'gitFileContents', 'gitFetchPr', 'gitPrFileDiff'
] as const
