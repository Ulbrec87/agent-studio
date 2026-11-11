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
        <div className={styles.brand} data-tauri-drag-region>
          <span className={styles.logo} data-tauri-drag-region>Agent Studio</span>
          <span className={styles.subtitle} data-tauri-drag-region>Modern Workspace</span>
        </div>
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
          <button className={styles.settingsBtn} onClick={onOpenSettings} aria-label="Settings">
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
    <svg width="32" height="36" viewBox="0 0 32 36" fill="none" aria-hidden="true">
      <path d="M16 1 30 9 16 17 2 9Z" fill="currentColor" opacity=".85" />
      <path d="M2 11 14 18v15L2 26Z" fill="currentColor" opacity=".55" />
      <path d="m30 11-12 7v15l12-7Z" fill="currentColor" opacity=".7" />
      <path d="m16 7 7 4-7 4-7-4Zm-8 9 6 3.5v7L8 23Zm16 0-6 3.5v7l6-3.5Z" fill="var(--color-bg)" />
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
