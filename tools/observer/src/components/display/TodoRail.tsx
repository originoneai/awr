import { ArrowRight, Terminal } from 'lucide-react';
import type { ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import { useCompactViewport } from '../../lib/viewport';
import { DisplayIcon } from './DisplayIcon';
import { EmptyState } from './EmptyState';
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
export function TodoRail({ title, items, visibleLimit = 3, onViewAll, empty }: TodoRailProps) {
  const { t } = useTranslation();
  const compactViewport = useCompactViewport();
  const visible = items.slice(0, visibleLimit);
  const remaining = Math.max(items.length - visibleLimit, 0);

  return (
    <aside className="obs-todo-rail" aria-label={t('display.todo.groupLabel')} data-compact={compactViewport || undefined}>
      <div className="obs-todo-head">
        <h2>{title}</h2>
        <span className="obs-todo-count">{items.length}</span>
        {onViewAll === undefined ? null : (
          <button type="button" className="obs-todo-all" onClick={onViewAll}>
            {t('display.todo.viewAll')}
            <DisplayIcon icon={ArrowRight} />
          </button>
        )}
      </div>
      {items.length === 0 ? (
        empty ?? <EmptyState />
      ) : (
        <>
          {visible.map((item, index) => {
            const shared = { 'data-open': item.onOpen !== undefined || undefined } as const;
            const onClick = item.onOpen;
            if (index === 0) {
              const first = (
                <>
                  <div className="obs-todo-first-meta">
                    <span className="obs-tag">{item.kind}</span>
                    {item.from ? <span>{item.from}</span> : null}
                    {item.age ? <span className="obs-todo-age">{item.age}</span> : null}
                  </div>
                  <div className="obs-todo-title">{item.title}</div>
                  {item.detail ? <div className="obs-todo-detail">{item.detail}</div> : null}
                  <div className="obs-todo-acts">
                    <span className="obs-todo-agent">
                      <DisplayIcon icon={Terminal} />
                      {t('display.todo.handleInAgent')}
                    </span>
                    <span className="obs-todo-view">
                      {t('display.todo.viewItem')}
                      <DisplayIcon icon={ArrowRight} />
                    </span>
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
              <DisplayIcon icon={ArrowRight} />
            </button>
          ) : null}
        </>
      )}
    </aside>
  );
}
