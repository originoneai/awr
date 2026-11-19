import type { CSSProperties } from 'react';
import { useTranslation } from 'react-i18next';
import { useCompactViewport } from '../../lib/viewport';
import './display.css';

export interface CompletionCounts {
  verified: number;
  reported: number;
  inProgress: number;
  notStarted: number;
}

const SEGMENTS = [
  { key: 'verified', token: '--craft-completion-verified' },
  { key: 'reported', token: '--craft-completion-reported' },
  { key: 'inProgress', token: '--craft-completion-in-progress' },
  { key: 'notStarted', token: '--craft-completion-not-started' },
] as const;

export interface CompletionBarProps {
  counts: CompletionCounts;
  /** The prototype draws the quiet "核验过 N · 标了完成 N · 共 N" line under the bar. */
  showSummary?: boolean;
}

/**
 * The prototype's four-segment completion bar (.seg4): one segment per completion grade,
 * sized by its count, plus the quiet summary line. The counts come from the caller; this
 * component only lays them out, and the accessible label speaks every count out loud.
 */
export function CompletionBar({ counts, showSummary = true }: CompletionBarProps) {
  const { t } = useTranslation();
  const compactViewport = useCompactViewport();
  const total = counts.verified + counts.reported + counts.inProgress + counts.notStarted;
  return (
    <div className="obs-completion" data-compact={compactViewport || undefined}>
      <div
        className="obs-seg4"
        role="img"
        aria-label={t('display.completion.barLabel', {
          verified: counts.verified,
          reported: counts.reported,
          inProgress: counts.inProgress,
          notStarted: counts.notStarted,
        })}
      >
        {SEGMENTS.map((segment) => {
          const value: number = counts[segment.key];
          if (value <= 0) return null;
          return (
            <i
              key={segment.key}
              className="obs-seg4-part"
              data-seg={segment.key}
              style={{ flexGrow: value, '--obs-seg-color': `var(${segment.token})` } as CSSProperties}
            />
          );
        })}
      </div>
      {showSummary ? (
        <div className="obs-completion-summary">
          {t('display.completion.summary', {
            verified: counts.verified,
            reported: counts.reported,
            total,
          })}
        </div>
      ) : null}
    </div>
  );
}

export interface CompletionLegendProps {
  /** inline: the tab-bar legend (.legend); wrapped: the chart legend (.hlegend). */
  variant?: 'inline' | 'wrapped';
}

/** The four-word legend for the completion grades (.legend/.hlegend in the prototype). */
export function CompletionLegend({ variant = 'inline' }: CompletionLegendProps) {
  const { t } = useTranslation();
  const compactViewport = useCompactViewport();
  return (
    <div
      className="obs-legend"
      data-variant={variant === 'wrapped' ? variant : undefined}
      data-compact={compactViewport || undefined}
      aria-label={t('display.completion.legendLabel')}
    >
      {SEGMENTS.map((segment) => (
        <span key={segment.key}>
          <i
            className="obs-legend-swatch"
            data-seg={segment.key}
            style={{ '--obs-seg-color': `var(${segment.token})` } as CSSProperties}
          />
          {t(`display.completion.${segment.key}`)}
        </span>
      ))}
    </div>
  );
}
