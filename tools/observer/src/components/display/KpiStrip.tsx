import { ArrowDown, ArrowUp, ChevronRight } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { useCompactViewport } from '../../lib/viewport';
import { DisplayIcon } from './DisplayIcon';
import './display.css';

export type KpiChangeTone = 'good' | 'warn' | 'bad';

export interface KpiChange {
  /** A short comparison sentence, always with its reference (prototype .kpi .c). */
  text: string;
  tone?: KpiChangeTone;
  trend?: 'up' | 'down';
}

export interface KpiMetric {
  key: string;
  label: string;
  /** The value is already formatted by the caller; the strip never computes numbers. */
  value: string;
  unit?: string;
  change?: KpiChange;
  /** The single cell the prototype marks as being about the viewer (.kpi.me). */
  emphasized?: boolean;
  /** Present when the cell opens the items behind the number (.kpi.go). */
  onOpen?: () => void;
}

export interface KpiStripProps {
  items: readonly KpiMetric[];
}

/**
 * The prototype's four-cell metric strip (.kpis/.kpi): label, large number with unit, and a
 * quiet change sentence with a good/warn/bad tone. Cells with `onOpen` are buttons that
 * reveal which items the number counts; the hover chevron announces that in the label row.
 */
export function KpiStrip({ items }: KpiStripProps) {
  const { t } = useTranslation();
  const compactViewport = useCompactViewport();

  return (
    <div className="obs-kpis" role="group" aria-label={t('display.kpi.groupLabel')} data-compact={compactViewport || undefined}>
      {items.map((metric) => {
        const body = (
          <>
            <span className="obs-kpi-label">
              {metric.label}
              {metric.onOpen === undefined ? null : (
                <span className="obs-kpi-hint"><DisplayIcon icon={ChevronRight} /></span>
              )}
            </span>
            <span className="obs-kpi-value">
              {metric.value}
              {metric.unit === undefined ? null : <small>{metric.unit}</small>}
            </span>
            {metric.change === undefined ? null : (
              <span className="obs-kpi-change" data-tone={metric.change.tone}>
                {metric.change.trend === undefined ? null : (
                  <span className="obs-kpi-trend">
                    <DisplayIcon icon={metric.change.trend === 'up' ? ArrowUp : ArrowDown} />
                  </span>
                )}
                {metric.change.text}
              </span>
            )}
          </>
        );
        const className = ['obs-kpi', metric.onOpen === undefined ? null : 'obs-kpi-open']
          .filter(Boolean)
          .join(' ');
        if (metric.onOpen === undefined) {
          return (
            <div key={metric.key} className={className} data-emphasized={metric.emphasized || undefined}>
              {body}
            </div>
          );
        }
        return (
          <button
            key={metric.key}
            type="button"
            className={className}
            data-emphasized={metric.emphasized || undefined}
            title={t('display.kpi.openTitle')}
            onClick={metric.onOpen}
          >
            {body}
          </button>
        );
      })}
    </div>
  );
}
