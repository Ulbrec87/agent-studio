import type { ReactNode } from 'react'
import styles from './OverviewView.module.css'

interface CardProps {
  title: string
  /** Grid area name, see OverviewView.module.css. */
  area: 'usage' | 'needs' | 'lastHour' | 'sessions' | 'events' | 'timeline' | 'waits'
  /** Short text on the right of the title. */
  meta?: ReactNode
  /** Controls placed in the header, before the meta text. */
  actions?: ReactNode
  /** Makes the title a button, for example to open a chart. */
  onTitleClick?: (trigger: HTMLElement) => void
  titleExpanded?: boolean
  titleLabel?: string
  children: ReactNode
}

export function Card({ title, area, meta, actions, onTitleClick, titleExpanded, titleLabel, children }: CardProps) {
  return (
    <section className={`${styles.card} ${styles[area]}`} aria-label={title}>
      <header className={styles.cardHeader}>
        <h3 className={styles.cardTitle}>
          {onTitleClick ? (
            <button
              type="button"
              className={styles.cardTitleBtn}
              aria-expanded={titleExpanded ?? false}
              aria-label={titleLabel}
              onClick={(e) => onTitleClick(e.currentTarget)}
            >
              {title}
            </button>
          ) : (
            title
          )}
        </h3>
        {(actions || meta !== undefined) && (
          <div className={styles.cardHeaderEnd}>
            {actions}
            {meta !== undefined && <span className={styles.cardMeta}>{meta}</span>}
          </div>
        )}
      </header>
      <div className={styles.cardBody}>{children}</div>
    </section>
  )
}

export function EmptyState({ children }: { children: ReactNode }) {
  return <p className={styles.empty}>{children}</p>
}
