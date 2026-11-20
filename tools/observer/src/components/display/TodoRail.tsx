import { Fragment, type ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import { useCompactViewport } from '../../lib/viewport';
import { DisplayIcon } from './DisplayIcon';
import { EmptyState } from './EmptyState';
import { ArrowRightIcon, TerminalIcon } from '../icons/prototype-icons';
import './display.css';

export interface TodoItem {
  key: string;
  /** The kind chip, for example an answer or a decision; a short noun from the caller. */
  kind: string;
  title: string;
  /** Only the first card shows its description. */
  detail?: string;
  from?: string;
  age?: string;
  onOpen?: () => void;
}

export interface TodoRailProps {
  title: string;
  items: readonly TodoItem[];
  /** A null total is unknown and must not be replaced by the confirmed row count. */
  count?: number | null;
  /** Product-language explanation shown when coverage is incomplete. */
  coverageNote?: ReactNode;
  /** Opens the first item's local Agent handback without submitting a business action. */
  onFirstAgent?: () => void;
  /** Optional local handback, shown by the caller only after onFirstAgent is used. */
  firstAccessory?: ReactNode;
  /** How many cards stay visible before the "还有 N 件" line; the prototype keeps three. */
  visibleLimit?: number;
  onViewAll?: () => void;
  empty?: ReactNode;
}

/**
 * The prototype's waiting rail (.rail/.todo): the first item opens into a full card, the
 * rest collapse to one line, and anything past the limit becomes "还有 N 件". Every card
 * only opens a page; the handling itself stays in the person's own Agent.
 */
export function TodoRail({
  title, items, count, coverageNote, onFirstAgent, firstAccessory, visibleLimit = 3, onViewAll, empty,
}: TodoRailProps) {
  const { t } = useTranslation();
  const compactViewport = useCompactViewport();
  const resolvedCount = count === undefined ? items.length : count;
  const visible = items.slice(0, visibleLimit);
  const remaining = resolvedCount === null ? 0 : Math.max(resolvedCount - visibleLimit, 0);

  return (
    <aside className="obs-todo-rail" aria-label={t('display.todo.groupLabel')} data-compact={compactViewport || undefined}>
      <div className="obs-todo-head">
        <h2>{title}</h2>
        <span className="obs-todo-count" data-unknown={resolvedCount === null || undefined}>{resolvedCount === null ? '—' : resolvedCount}</span>
        {onViewAll === undefined ? null : (
          <button type="button" className="obs-todo-all" onClick={onViewAll}>
            {t('display.todo.viewAll')}
            <DisplayIcon icon={ArrowRightIcon} />
          </button>
        )}
      </div>
      {coverageNote === undefined ? null : <div className="obs-todo-coverage">{coverageNote}</div>}
      {items.length === 0 ? (
        empty ?? <EmptyState />
      ) : (
        <>
          {visible.map((item, index) => {
            const shared = { 'data-open': item.onOpen !== undefined || undefined } as const;
            const onClick = item.onOpen;
            if (index === 0) {
              const firstBody = (
                <>
                  <div className="obs-todo-first-meta">
                    <span className="obs-tag">{item.kind}</span>
                    {item.from ? <span>{item.from}</span> : null}
                    {item.age ? <span className="obs-todo-age">{item.age}</span> : null}
                  </div>
                  <div className="obs-todo-title">{item.title}</div>
                  {item.detail ? <div className="obs-todo-detail">{item.detail}</div> : null}
                </>
              );
              if (onFirstAgent) {
                return (
                  <Fragment key={item.key}>
                    {onClick === undefined ? (
                      <div className="obs-todo-card" {...shared}>
                        {firstBody}
                        <div className="obs-todo-acts">
                          <button type="button" className="obs-todo-agent obs-todo-agent-control" onClick={onFirstAgent}>
                            <DisplayIcon icon={TerminalIcon} />{t('display.todo.handleInAgent')}
                          </button>
                          <span className="obs-todo-view">{t('display.todo.viewItem')}<DisplayIcon icon={ArrowRightIcon} /></span>
                        </div>
                      </div>
                    ) : (
                      <button
                        type="button"
                        className="obs-todo-card"
                        {...shared}
                        onClick={onClick}
                        onKeyDown={(event) => {
                          if (event.key.toLocaleLowerCase() !== 'h') return;
                          event.preventDefault();
                          event.stopPropagation();
                          onFirstAgent();
                        }}
                        aria-keyshortcuts="h"
                        title={t('display.todo.handbackShortcut')}
                      >
                        {firstBody}
                        <div className="obs-todo-acts">
                          <span
                            className="obs-todo-agent obs-todo-agent-control"
                            onClick={(event) => {
                              event.stopPropagation();
                              onFirstAgent();
                            }}
                          >
                            <DisplayIcon icon={TerminalIcon} />{t('display.todo.handleInAgent')}
                          </span>
                          <span className="obs-todo-view">{t('display.todo.viewItem')}<DisplayIcon icon={ArrowRightIcon} /></span>
                        </div>
                      </button>
                    )}
                    {firstAccessory}
                  </Fragment>
                );
              }
              const first = (
                <>
                  {firstBody}
                  <div className="obs-todo-acts">
                    <span className="obs-todo-agent"><DisplayIcon icon={TerminalIcon} />{t('display.todo.handleInAgent')}</span>
                    <span className="obs-todo-view">{t('display.todo.viewItem')}<DisplayIcon icon={ArrowRightIcon} /></span>
                  </div>
                </>
              );
              return onClick === undefined ? (
                <div key={item.key} className="obs-todo-card" {...shared}>{first}</div>
              ) : (
                <button key={item.key} type="button" className="obs-todo-card" {...shared} onClick={onClick}>{first}</button>
              );
            }
            const min = (
              <>
                <span className="obs-tag">{item.kind}</span>
                <span className="obs-todo-min-title">{item.title}</span>
                {item.age ? <span className="obs-todo-age">{item.age}</span> : null}
              </>
            );
            return onClick === undefined ? (
              <div key={item.key} className="obs-todo-card obs-todo-min" {...shared}>{min}</div>
            ) : (
              <button key={item.key} type="button" className="obs-todo-card obs-todo-min" {...shared} onClick={onClick}>{min}</button>
            );
          })}
          {remaining > 0 && onViewAll !== undefined ? (
            <button type="button" className="obs-todo-more" onClick={onViewAll}>
              {t('display.todo.more', { remaining })}
              <DisplayIcon icon={ArrowRightIcon} />
            </button>
          ) : null}
        </>
      )}
    </aside>
  );
}
