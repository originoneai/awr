import { CircleCheck, CircleX, Clock, Circle } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import type { LucideIcon } from 'lucide-react';
import { DisplayIcon } from './DisplayIcon';
import { EmptyState } from './EmptyState';
import './display.css';

/** The four states of one completion criterion (.acc data-s in the prototype). */
export type AcceptanceItemStatus = 'supported' | 'failed' | 'waiting' | 'notStarted';

const STATUS_ICONS: Record<AcceptanceItemStatus, LucideIcon> = {
  supported: CircleCheck,
  failed: CircleX,
  waiting: Clock,
  notStarted: Circle,
};

export interface AcceptanceItem {
  key: string;
  title: string;
  /** The fact that backs or blocks this criterion; shown as the quiet second line. */
  evidence?: string;
  status: AcceptanceItemStatus;
}

export interface AcceptanceListProps {
  items: readonly AcceptanceItem[];
}

/**
 * The prototype's acceptance list (.acc): one row per completion criterion with an icon, the
 * criterion, its evidence and the trailing status word. The word is always present, so the
 * state never rests on the icon's color alone.
 */
export function AcceptanceList({ items }: AcceptanceListProps) {
  const { t } = useTranslation();
  if (items.length === 0) return <EmptyState />;
  return (
    <ul className="obs-acceptance" aria-label={t('display.acceptance.groupLabel')}>
      {items.map((item) => (
        <li key={item.key} className="obs-acceptance-item" data-status={item.status}>
          <span className="obs-acceptance-icon"><DisplayIcon icon={STATUS_ICONS[item.status]} /></span>
          <div className="obs-acceptance-body">
            {item.title}
            {item.evidence ? <div className="obs-acceptance-evidence">{item.evidence}</div> : null}
          </div>
          <span className="obs-acceptance-word">{t(`display.acceptance.${item.status}`)}</span>
        </li>
      ))}
    </ul>
  );
}
