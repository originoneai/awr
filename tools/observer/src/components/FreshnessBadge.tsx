import { Space, Tag, Typography } from 'antd';
import { useTranslation } from 'react-i18next';
import { PLANE_STATES, type PlaneState } from '../lib/contract';
import { formatUnixMs } from '../lib/format';
import type { PlaneFreshness, TeamFreshness } from '../lib/types';

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
  const { t, i18n } = useTranslation();
  const status = planeStatus(plane);
  let detail = '—';
  if (status === 'failed' && plane?.error_code) {
    detail = String(plane.error_code);
  } else if ((status === 'measured' || status === 'stale') && plane?.at_unix_ms != null) {
    detail = formatUnixMs(plane.at_unix_ms, i18n.language);
  }
  return (
    <span data-testid={`freshness-${planeKey}`}>
      <Typography.Text type="secondary" style={{ marginRight: 4 }}>
        {t(`freshness.plane.${planeKey}`)}
      </Typography.Text>
      <Tag color={STATE_COLORS[status]} style={{ marginInlineEnd: 4 }}>
        {t(`freshness.state.${status}`)}
      </Tag>
      <Typography.Text type="secondary" style={{ fontSize: 12 }}>
        {detail}
      </Typography.Text>
    </span>
  );
}

/**
 * The three freshness planes (report received / authoritative / page read),
 * each exactly one of the contract's four states. Vocabulary and states come
 * from observation-contract.js; this component never invents a measurement.
 */
export function FreshnessBadge({ freshness }: { freshness?: TeamFreshness }) {
  return (
    <Space size={[16, 4]} wrap data-testid="freshness-badge">
      {PLANE_KEYS.map((key) => (
        <PlaneView key={key} planeKey={key} plane={freshness?.[key]} />
      ))}
    </Space>
  );
}
