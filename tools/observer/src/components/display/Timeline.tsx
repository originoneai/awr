import type { ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import { EmptyState } from './EmptyState';
import './display.css';

export interface TimelineItem {
  key: string;
  time: ReactNode;
  title: ReactNode;
  detail?: ReactNode;
  tone?: 'default' | 'current' | 'positive' | 'negative';
}

export interface TimelineProps {
  items: readonly TimelineItem[];
  empty?: ReactNode;
}

/** The prototype's compact chronology, with metadata above each event and its consequence. */
export function Timeline({ items, empty }: TimelineProps) {
  const { t } = useTranslation();
  if (items.length === 0) return <>{empty ?? <EmptyState />}</>;
  return (
    <ol className="obs-timeline" aria-label={t('display.timeline.label')}>
      {items.map((item) => (
        <li key={item.key} className="obs-timeline-item" data-tone={item.tone ?? 'default'}>
          <div className="obs-timeline-time">{item.time}</div>
          <div className="obs-timeline-title">{item.title}</div>
          {item.detail === undefined ? null : <div className="obs-timeline-detail">{item.detail}</div>}
        </li>
      ))}
    </ol>
  );
}
