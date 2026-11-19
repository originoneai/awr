import { Card } from 'antd';
import type { ReactNode } from 'react';
import './display.css';

export interface SectionProps {
  title: ReactNode;
  count?: number;
  extra?: ReactNode;
  children: ReactNode;
  padded?: boolean;
  className?: string;
}

/** The prototype's bordered section: title, quiet count, trailing area and content. */
export function Section({ title, count, extra, children, padded = false, className }: SectionProps) {
  return (
    <Card
      className={['obs-section', className].filter(Boolean).join(' ')}
      title={(
        <span className="obs-section-heading">
          <h2 className="obs-section-title">{title}</h2>
          {count === undefined ? null : <span className="obs-section-count">{count}</span>}
        </span>
      )}
      extra={extra}
    >
      <div className="obs-section-content" data-padded={padded || undefined}>
        {children}
      </div>
    </Card>
  );
}
