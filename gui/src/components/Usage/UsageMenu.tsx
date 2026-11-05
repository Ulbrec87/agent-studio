import { useCallback, useRef, useState } from 'react'
import type { UsageState } from '@shared/usage/types'
import { UsageChip } from './UsageChip'
import { UsagePopover, type CloseReason } from './UsagePopover'
import styles from './UsageMenu.module.css'

interface UsageMenuProps {
  state: UsageState | null
  onOpenReport: () => void
}

/** The header's usage chip and the popover it opens; puts focus back on the chip when closed by keyboard. */
export function UsageMenu({ state, onOpenReport }: UsageMenuProps) {
  const [open, setOpen] = useState(false)
  const chipRef = useRef<HTMLButtonElement>(null)

  const close = useCallback((reason: CloseReason) => {
    setOpen(false)
    // After a click elsewhere the focus belongs to whatever was clicked.
    if (reason !== 'outside') chipRef.current?.focus()
  }, [])

  return (
    <div className={styles.menu}>
      <UsageChip ref={chipRef} state={state} open={open} onClick={() => setOpen((o) => !o)} />
      {open && <UsagePopover state={state} anchorRef={chipRef} onClose={close} onOpenReport={onOpenReport} />}
    </div>
  )
}
