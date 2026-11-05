import { AgentItem } from './AgentItem'
import { EditorIcon } from '../shared/EditorIcon'
import type { ProjectGroup, EditorId, ClaudeProfile } from '@shared/types'
import { claudeProfileLabel, resolveClaudeProfile } from '@shared/claudeProfiles'
import { ClaudeProfileOptions } from '../shared/ClaudeProfileOptions'
import { getEditorLabel } from '../../lib/editorUtils'
import styles from './ProjectTree.module.css'

interface ProjectTreeProps {
  group: ProjectGroup
  selectedAgentId: string | null
  onSelectAgent: (agentId: string) => void
  onNewAgentForProject: (projectDir: string) => void
  onRenameAgent: (agentId: string, newName: string) => void
  onRemoveAgent: (agentId: string) => void
  onStopAgent: (agentId: string) => void
  defaultEditor?: EditorId
  expanded: boolean
  onToggleExpanded: () => void
  numberedAgentMap?: Map<string, number>
  claudeProfiles?: ClaudeProfile[]
  claudeProfileByProject?: Record<string, string>
  /** `null` clears this project's own mapping so it inherits again. */
  onSetClaudeProfile?: (projectDir: string, configDir: string | null) => void
}

const INHERIT_PROFILE = '__inherit__'

export function ProjectTree({ group, selectedAgentId, onSelectAgent, onNewAgentForProject, onRenameAgent, onRemoveAgent, onStopAgent, defaultEditor = 'vscode', expanded, onToggleExpanded, numberedAgentMap, claudeProfiles = [], claudeProfileByProject = {}, onSetClaudeProfile }: ProjectTreeProps) {
  const ownProfile = claudeProfileByProject[group.projectDir]
  const effectiveProfile = resolveClaudeProfile(group.projectDir, claudeProfileByProject)
  const showProfilePicker = !!onSetClaudeProfile && (claudeProfiles.length > 1 || effectiveProfile !== null)

  return (
    <div className={styles.group}>
      <div className={styles.groupRow}>
        <button
          className={styles.groupHeader}
          onClick={onToggleExpanded}
        >
          <span className={`${styles.chevron} ${expanded ? styles.expanded : ''}`}>
            <ChevronIcon />
          </span>
          <FolderIcon open={expanded} />
          <span className={styles.projectName}>{group.projectName}</span>
        </button>
        {showProfilePicker && (
          <label
            className={`${styles.profileChip} ${effectiveProfile === null ? styles.profileChipUnset : ''} ${ownProfile === undefined ? styles.profileChipInherited : ''}`}
            title={
              effectiveProfile === null
                ? 'Claude profile: not set (uses Agent Studio\'s environment). Click to choose.'
                : `Claude profile: ${claudeProfileLabel(effectiveProfile, claudeProfiles)}${ownProfile === undefined ? ' (inherited from a parent folder)' : ''}`
            }
          >
            <ProfileIcon />
            {effectiveProfile !== null && <span>{claudeProfileLabel(effectiveProfile, claudeProfiles)}</span>}
            <select
              className={styles.profileSelect}
              aria-label={`Claude profile for ${group.projectName}`}
              value={ownProfile ?? INHERIT_PROFILE}
              onChange={(e) => onSetClaudeProfile?.(group.projectDir, e.target.value === INHERIT_PROFILE ? null : e.target.value)}
            >
              <option value={INHERIT_PROFILE}>
                {effectiveProfile !== null && ownProfile === undefined ? 'Inherit from parent folder' : 'Not set'}
              </option>
              <ClaudeProfileOptions profiles={claudeProfiles} selected={ownProfile} />
            </select>
          </label>
        )}
        <button
          className={styles.addBtn}
          onClick={() => window.hydra.openInApp(defaultEditor, group.projectDir)}
          title={`Open in ${getEditorLabel(defaultEditor)}`}
        >
          <EditorIcon editor={defaultEditor} size={12} />
        </button>
        <button
          className={styles.addBtn}
          onClick={() => onNewAgentForProject(group.projectDir)}
          title={`New agent in ${group.projectName}`}
        >
          <PlusIcon />
        </button>
        <span className={styles.count}>{group.agents.length}</span>
      </div>

      {expanded && (
        <div className={styles.agentList}>
          {group.agents.map((agent) => (
            <AgentItem
              key={agent.id}
              agent={agent}
              isSelected={agent.id === selectedAgentId}
              onSelect={() => onSelectAgent(agent.id)}
              onRename={(newName) => onRenameAgent(agent.id, newName)}
              onRemove={() => onRemoveAgent(agent.id)}
              onStop={() => onStopAgent(agent.id)}
              hotkeyNumber={numberedAgentMap?.get(agent.id)}
            />
          ))}
        </div>
      )}
    </div>
  )
}

function ChevronIcon() {
  return (
    <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
      <polyline points="9 18 15 12 9 6" />
    </svg>
  )
}

function FolderIcon({ open }: { open: boolean }) {
  if (open) {
    return (
      <svg className={styles.folderIcon} width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round">
        <path d="M6 14l1.5-2.9A2 2 0 0 1 9.24 10H20a2 2 0 0 1 1.94 2.5l-1.54 6a2 2 0 0 1-1.95 1.5H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h3.9a2 2 0 0 1 1.69.9l.81 1.2a2 2 0 0 0 1.67.9H18a2 2 0 0 1 2 2v2" />
      </svg>
    )
  }
  return (
    <svg className={styles.folderIcon} width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round">
      <path d="M20 20a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.9a2 2 0 0 1-1.69-.9L9.6 3.9A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2h16Z" />
    </svg>
  )
}

function ProfileIcon() {
  return (
    <svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
      <circle cx="12" cy="8" r="4" />
      <path d="M4 21a8 8 0 0 1 16 0" />
    </svg>
  )
}

function PlusIcon() {
  return (
    <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round">
      <line x1="12" y1="5" x2="12" y2="19" />
      <line x1="5" y1="12" x2="19" y2="12" />
    </svg>
  )
}
