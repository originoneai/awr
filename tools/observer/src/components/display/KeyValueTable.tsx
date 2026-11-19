import type { ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import { EmptyState } from './EmptyState';
import './display.css';

export interface KeyValueItem {
  key: string;
  label: ReactNode;
  value: ReactNode;
}

export interface KeyValueTableProps {
  items: readonly KeyValueItem[];
  empty?: ReactNode;
}

/** A wrapping two-column description list, matching the prototype's 96px label column. */
export function KeyValueTable({ items, empty }: KeyValueTableProps) {
  const { t } = useTranslation();
  if (items.length === 0) return <>{empty ?? <EmptyState />}</>;
  return (
    <dl className="obs-key-value" aria-label={t('display.keyValue.label')}>
      {items.map((item) => (
        <div className="obs-key-value-row" key={item.key}>
          <dt>{item.label}</dt>
          <dd>{item.value}</dd>
        </div>
      ))}
    </dl>
  );
}
