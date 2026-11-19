import { useTranslation } from 'react-i18next';
import './display.css';

export interface EmptyStateProps {
  title?: string;
  description?: string;
}

/** The prototype uses quiet text rather than a decorative empty-state illustration. */
export function EmptyState({ title, description }: EmptyStateProps) {
  const { t } = useTranslation();
  return (
    <div className="obs-empty" data-testid="obs-empty-state">
      <div>{title ?? t('display.empty.title')}</div>
      {description ? <div className="obs-empty-description">{description}</div> : null}
    </div>
  );
}
