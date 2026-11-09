import { ViewSwitcher } from './ViewSwitcher'
import { OpenInButton } from './OpenInButton'
import { Tooltip } from '../Tooltip'
import { UsageMenu } from '../Usage/UsageMenu'
import type { UsageState } from '@shared/usage/types'
import type { ViewMode, EditorId } from '@shared/types'
import styles from './Header.module.css'

interface HeaderProps {
  viewMode: ViewMode
  onSelectViewMode: (mode: ViewMode) => void
  needsYouCount?: number
  onOpenNewAgent: () => void
  globalYolo: boolean
  onToggleGlobalYolo: () => void
  onOpenSettings: () => void
  onOpenHeadless: () => void
  usage: UsageState | null
  onOpenUsage: () => void
  onOpenUpdates: () => void
  showUpdateAction: boolean
  updateReadyToInstall: boolean
  updateAvailable: boolean
  selectedProjectName?: string
  projectDir: string | null
  defaultEditor: EditorId
  onSetDefaultEditor: (id: EditorId) => void
}

export function Header({
  viewMode,
  onSelectViewMode,
  needsYouCount,
  onOpenNewAgent,
  globalYolo: _globalYolo,
  onToggleGlobalYolo: _onToggleGlobalYolo,
  onOpenSettings,
  onOpenHeadless: _onOpenHeadless,
  usage,
  onOpenUsage,
  onOpenUpdates,
  showUpdateAction,
  updateReadyToInstall,
  updateAvailable,
  selectedProjectName,
  projectDir,
  defaultEditor,
  onSetDefaultEditor
}: HeaderProps) {
  const isMac = navigator.platform.toLowerCase().includes('mac')
  return (
    <header className={`${styles.header} ${isMac ? styles.macHeader : ''} titlebar-drag`} data-tauri-drag-region>
      <div className={styles.left} data-tauri-drag-region>
        <div className={styles.logoMark} data-tauri-drag-region>
          <AgentStudioLogo />
        </div>
        <span className={styles.logo} data-tauri-drag-region>AGENT STUDIO</span>
        {viewMode === 'grid' && selectedProjectName && (
          <>
            <button
              className={`${styles.projectAddBtn} titlebar-no-drag`}
              onClick={onOpenNewAgent}
              title={`New agent in ${selectedProjectName}`}
            >
              <PlusIcon />
            </button>
            <span className={styles.projectName} data-tauri-drag-region>{selectedProjectName}</span>
          </>
        )}
      </div>

      <div className={`${styles.center} titlebar-no-drag`}>
        <ViewSwitcher viewMode={viewMode} onSelect={onSelectViewMode} needsYouCount={needsYouCount} />
      </div>

      <div className={`${styles.right} titlebar-no-drag`}>
        <OpenInButton
          projectDir={projectDir}
          defaultEditor={defaultEditor}
          onSetDefaultEditor={onSetDefaultEditor}
        />
        {showUpdateAction && (
          <Tooltip content={updateReadyToInstall ? 'Update ready to install' : 'Check for app updates'}>
            <button
              className={`${styles.updateBtn} ${updateReadyToInstall ? styles.updateReady : updateAvailable ? styles.updateAvailable : ''}`}
              onClick={onOpenUpdates}
            >
              {updateReadyToInstall ? 'Install Update' : updateAvailable ? 'Update Available' : 'Updates'}
            </button>
          </Tooltip>
        )}
        {/* Headless button hidden — functionality preserved, just not exposed in header for now */}
        <UsageMenu state={usage} onOpenReport={onOpenUsage} />
        <Tooltip content="Settings">
          <button className={styles.settingsBtn} onClick={onOpenSettings}>
            <SettingsIcon />
          </button>
        </Tooltip>
      </div>
    </header>
  )
}

function PlusIcon() {
  return (
    <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
      <line x1="12" y1="5" x2="12" y2="19" />
      <line x1="5" y1="12" x2="19" y2="12" />
    </svg>
  )
}

function AgentStudioLogo() {
  return (
    <svg width="22" height="22" viewBox="0 0 28 28" fill="none">
      <path d="M5.5 23 L14 4 L22.5 23" stroke="currentColor" strokeWidth="3" strokeLinecap="round" strokeLinejoin="round" />
      <circle cx="14" cy="16.5" r="2" fill="currentColor" />
    </svg>
  )
}

function SettingsIcon() {
  return (
    <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
      <path d="M12.22 2h-.44a2 2 0 0 0-2 2v.18a2 2 0 0 1-1 1.73l-.43.25a2 2 0 0 1-2 0l-.15-.08a2 2 0 0 0-2.73.73l-.22.38a2 2 0 0 0 .73 2.73l.15.1a2 2 0 0 1 1 1.72v.51a2 2 0 0 1-1 1.74l-.15.09a2 2 0 0 0-.73 2.73l.22.38a2 2 0 0 0 2.73.73l.15-.08a2 2 0 0 1 2 0l.43.25a2 2 0 0 1 1 1.73V20a2 2 0 0 0 2 2h.44a2 2 0 0 0 2-2v-.18a2 2 0 0 1 1-1.73l.43-.25a2 2 0 0 1 2 0l.15.08a2 2 0 0 0 2.73-.73l.22-.39a2 2 0 0 0-.73-2.73l-.15-.08a2 2 0 0 1-1-1.74v-.5a2 2 0 0 1 1-1.74l.15-.09a2 2 0 0 0 .73-2.73l-.22-.38a2 2 0 0 0-2.73-.73l-.15.08a2 2 0 0 1-2 0l-.43-.25a2 2 0 0 1-1-1.73V4a2 2 0 0 0-2-2z" />
      <circle cx="12" cy="12" r="3" />
    </svg>
  )
}
