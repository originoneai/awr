import { Alert } from 'antd';
import { Info, TriangleAlert } from 'lucide-react';
import type { ReactNode } from 'react';
import { DisplayIcon } from './DisplayIcon';
import './display.css';

export interface NoticeProps {
  children: ReactNode;
  title?: ReactNode;
  extra?: ReactNode;
  tone?: 'info' | 'warning';
}

/** A read-only explanatory strip; it intentionally has no business action slot. */
export function Notice({ children, title, extra, tone = 'info' }: NoticeProps) {
  return (
    <Alert
      className="obs-notice"
      data-tone={tone}
      type={tone === 'warning' ? 'warning' : 'info'}
      showIcon
      icon={<DisplayIcon icon={tone === 'warning' ? TriangleAlert : Info} />}
      title={(
        <div className="obs-notice-content">
          <div className="obs-notice-body">
            {title === undefined ? null : <strong className="obs-notice-title">{title}</strong>}
            {children}
          </div>
          {extra === undefined ? null : <div className="obs-notice-extra">{extra}</div>}
        </div>
      )}
    />
  );
}
