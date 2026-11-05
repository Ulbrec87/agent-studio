// @vitest-environment jsdom

import { cleanup, fireEvent, render, screen } from '@testing-library/react'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { ProjectTree } from './ProjectTree'
import type { ClaudeProfile, ProjectGroup } from '@shared/types'

const group: ProjectGroup = { projectDir: '/var/www/test/app', projectName: 'app', agents: [] }
const profiles: ClaudeProfile[] = [
  { configDir: '/home/me/.claude', label: 'default', isDefault: true },
  { configDir: '/home/me/.claude-work', label: 'work', isDefault: false }
]

function renderTree(props: Partial<Parameters<typeof ProjectTree>[0]> = {}) {
  return render(
    <ProjectTree
      group={group}
      selectedAgentId={null}
      onSelectAgent={vi.fn()}
      onNewAgentForProject={vi.fn()}
      onRenameAgent={vi.fn()}
      onRemoveAgent={vi.fn()}
      onStopAgent={vi.fn()}
      expanded={false}
      onToggleExpanded={vi.fn()}
      claudeProfiles={profiles}
      {...props}
    />
  )
}

describe('ProjectTree Claude profile picker', () => {
  afterEach(() => cleanup())

  it('shows the profile inherited from a parent folder', () => {
    renderTree({ claudeProfileByProject: { '/var/www/test': '/home/me/.claude-work' }, onSetClaudeProfile: vi.fn() })

    expect(screen.getByText('work')).toBeTruthy()
    const select = screen.getByLabelText('Claude profile for app') as HTMLSelectElement
    expect(select.value).toBe('__inherit__')
  })

  it('maps the project when a profile is picked and clears it when set back to inherit', () => {
    const onSetClaudeProfile = vi.fn()
    renderTree({ claudeProfileByProject: { [group.projectDir]: '' }, onSetClaudeProfile })

    const select = screen.getByLabelText('Claude profile for app')
    fireEvent.change(select, { target: { value: '/home/me/.claude-work' } })
    expect(onSetClaudeProfile).toHaveBeenLastCalledWith(group.projectDir, '/home/me/.claude-work')

    fireEvent.change(select, { target: { value: '__inherit__' } })
    expect(onSetClaudeProfile).toHaveBeenLastCalledWith(group.projectDir, null)
  })

  it('hides the picker when there is only one profile and nothing mapped', () => {
    renderTree({ claudeProfiles: [profiles[0]], onSetClaudeProfile: vi.fn() })
    expect(screen.queryByLabelText('Claude profile for app')).toBeNull()
  })
})
