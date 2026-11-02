import { Space, Tag, Typography } from 'antd';
import { useTranslation } from 'react-i18next';
import { PLANE_STATES, type PlaneState } from '../lib/contract';
import { freshnessWord } from '../lib/freshness';
import { formatUnixMs } from '../lib/format';
import type { PlaneFreshness, TeamFreshness } from '../lib/types';
import { TechnicalDetails } from './TechnicalDetails';

const PLANE_KEYS = ['report_received', 'authoritative', 'page_read'] as const;

const STATE_COLORS: Record<PlaneState, string> = {
  measured: 'success',
  stale: 'warning',
  failed: 'error',
  not_measured: 'default',
};

function planeStatus(plane: PlaneFreshness | undefined): PlaneState {
  const status = plane?.status;
  return (PLANE_STATES as readonly string[]).includes(status ?? '')
    ? (status as PlaneState)
    : 'not_measured';
}

function PlaneView({ planeKey, plane }: { planeKey: (typeof PLANE_KEYS)[number]; plane?: PlaneFreshness }) {
  const { t } = useTranslation();
  const status = planeStatus(plane);
  return (
    <span data-testid={`freshness-${planeKey}`}>
      <Tag color={STATE_COLORS[status]} style={{ marginInlineEnd: 4 }}>
        {t('freshness.statusLabel', {
          plane: t(`freshness.display.${planeKey}`),
          status: freshnessWord(t, status),
        })}
      </Tag>
    </span>
  );
}

/**
 * The three freshness planes keep their contractual states. Their engineering
 * names and raw replies stay in disclosure; the default names only the latest read.
 */
export function FreshnessBadge({ freshness }: { freshness?: TeamFreshness }) {
  const { t, i18n } = useTranslation();
  const latestAt = PLANE_KEYS.reduce<number | null>((latest, key) => {
    const plane = freshness?.[key];
    const status = planeStatus(plane);
    const at = plane?.at_unix_ms;
    if ((status === 'measured' || status === 'stale') && typeof at === 'number' && Number.isFinite(at)) {
      return latest == null ? at : Math.max(latest, at);
    }
    return latest;
  }, null);
  const time = latestAt == null ? freshnessWord(t, 'not_measured') : formatUnixMs(latestAt, i18n.language);
  return (
    <Space size={[16, 4]} wrap data-testid="freshness-badge">
      <Typography.Text type="secondary">{t('freshness.recentRead', { time })}</Typography.Text>
      {PLANE_KEYS.map((key) => (
        <PlaneView key={key} planeKey={key} plane={freshness?.[key]} />
      ))}
      <TechnicalDetails items={PLANE_KEYS.map((key) => ({
        label: t(`freshness.plane.${key}`),
        value: freshness?.[key] ?? null,
      }))} />
    </Space>
  );
}
