import { Typography } from 'antd';
import type { WorkStateResult } from '../core/observation';

/** One work's last report: the time never wraps, a stale note sits under it. */
export function LastReportText({ report, testId }: { report: WorkStateResult['lastReport']; testId: string }) {
  return (
    <span data-testid={testId} style={{ whiteSpace: 'nowrap' }}>
      {report.timeText ?? report.text}
      {report.staleText ? <Typography.Text type="secondary" style={{ display: 'block' }}>{report.staleText}</Typography.Text> : null}
    </span>
  );
}
