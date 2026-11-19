import { ChevronRight } from 'lucide-react';
import type { ReactNode } from 'react';
import { DisplayIcon } from './DisplayIcon';
import { EmptyState } from './EmptyState';
import './display.css';

export interface RowListItem {
  key: string;
  title: ReactNode;
  meta?: ReactNode;
  leading?: ReactNode;
  trailing?: ReactNode;
  selected?: boolean;
  onOpen?: () => void;
}

export interface RowListProps {
  items: readonly RowListItem[];
  empty?: ReactNode;
  flush?: boolean;
}

/** Uniform list rows. Only rows with a read destination render an arrow or a button. */
export function RowList({ items, empty, flush = false }: RowListProps) {
  if (items.length === 0) return <>{empty ?? <EmptyState />}</>;
  return (
    <ul className="obs-row-list" data-flush={flush || undefined}>
      {items.map((item) => {
        const content = (
          <>
            {item.leading === undefined ? null : <span className="obs-row-leading">{item.leading}</span>}
            <span className="obs-row-body">
              <span className="obs-row-title">{item.title}</span>
              {item.meta === undefined ? null : <span className="obs-row-meta">{item.meta}</span>}
            </span>
            {item.trailing === undefined ? null : <span className="obs-row-trailing">{item.trailing}</span>}
            {item.onOpen ? <DisplayIcon icon={ChevronRight} /> : null}
          </>
        );
        return (
          <li key={item.key} className="obs-row-item">
            {item.onOpen ? (
              <button
                type="button"
                className="obs-row-control"
                aria-current={item.selected || undefined}
                onClick={item.onOpen}
              >
                {content}
              </button>
            ) : (
              <div className="obs-row-control" aria-current={item.selected || undefined}>{content}</div>
            )}
          </li>
        );
      })}
    </ul>
  );
}
