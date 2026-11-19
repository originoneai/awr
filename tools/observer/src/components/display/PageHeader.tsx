import { Grid, Typography } from 'antd';
import { ArrowLeft } from 'lucide-react';
import type { ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import { DisplayIcon } from './DisplayIcon';
import './display.css';

export interface PageHeaderProps {
  title: ReactNode;
  subtitle?: ReactNode;
  summary?: ReactNode;
  meta?: ReactNode;
  actions?: ReactNode;
  onBack?: () => void;
  backLabel?: string;
  variant?: 'compact' | 'detail';
}

/** Compact list headings and the taller detail-page hero share one composition. */
export function PageHeader({
  title,
  subtitle,
  summary,
  meta,
  actions,
  onBack,
  backLabel,
  variant = 'compact',
}: PageHeaderProps) {
  const { t } = useTranslation();
  const compactViewport = !(Grid.useBreakpoint().md ?? true);
  const header = (
    <header className="obs-page-header" data-variant={variant} data-compact={compactViewport || undefined}>
      <div className="obs-page-header-main">
        <div className="obs-page-title-line">
          <Typography.Title level={1} className="obs-page-title">{title}</Typography.Title>
          {subtitle === undefined ? null : <span className="obs-page-subtitle">{subtitle}</span>}
        </div>
        {summary === undefined ? null : <p className="obs-page-summary">{summary}</p>}
        {meta === undefined ? null : <div className="obs-page-meta">{meta}</div>}
      </div>
      {actions === undefined ? null : <div className="obs-page-actions">{actions}</div>}
    </header>
  );

  if (!onBack) return header;
  return (
    <div className="obs-page-with-back">
      <button type="button" className="obs-page-back" onClick={onBack}>
        <DisplayIcon icon={ArrowLeft} />
        {backLabel ?? t('display.pageHeader.back')}
      </button>
      {header}
    </div>
  );
}
