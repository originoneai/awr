import { useQuery } from '@tanstack/react-query';
import { Card, Space, Typography } from 'antd';
import { useTranslation } from 'react-i18next';
import { FreshnessBadge } from '../components/FreshnessBadge';
import { NotReportedYet, ObservationEmpty, ObservationForbidden, PartialFailure } from '../components/states';
import { isAuthError } from '../lib/api';
import { overviewQueryOptions, projectsQueryOptions } from '../lib/queries';
import { useScope } from '../lib/scope';

/**
 * Workbench placeholder: wires the real projects + overview reads so the
 * freshness planes and coverage summary are live; the record list itself is
 * the shared pending presentation until a later slice.
 */
export function WorkbenchPage() {
  const { t } = useTranslation();
  const { projectKey } = useScope();
  const projects = useQuery(projectsQueryOptions());
  const effectiveProject = projectKey ?? projects.data?.projects?.[0]?.key ?? null;
  const overview = useQuery(overviewQueryOptions(effectiveProject));

  if (overview.error && isAuthError(overview.error)) {
    return <ObservationForbidden />;
  }

  const data = overview.data;
  const failed = data?.coverage?.failed ?? 0;
  const pageReadAt = data?.freshness?.page_read?.at_unix_ms;

  return (
    <Space direction="vertical" size={16} style={{ width: '100%' }} data-testid="page-workbench">
      <Card>
        <Typography.Title level={5} style={{ marginTop: 0 }}>
          {t('freshness.title')}
        </Typography.Title>
        <FreshnessBadge freshness={data?.freshness} />
        {data?.coverage ? (
          <Typography.Paragraph type="secondary" style={{ marginTop: 12, marginBottom: 0 }}>
            {t('coverage.summary', {
              scope: data.coverage.scope.status,
              succeeded: data.coverage.succeeded,
              failed: data.coverage.failed,
              enumerated: data.coverage.works_enumerated,
              total: data.coverage.works_total ?? t('coverage.totalUnknown'),
            })}
          </Typography.Paragraph>
        ) : null}
      </Card>
      <Card>
        {overview.error ? (
          <PartialFailure errorCode={(overview.error as { code?: string }).code} confirmedAtUnixMs={pageReadAt}>
            <ObservationEmpty readAtUnixMs={pageReadAt} />
          </PartialFailure>
        ) : failed > 0 ? (
          <PartialFailure errorCode={data?.coverage.streams.find((s) => s.status === 'failed')?.error_code} confirmedAtUnixMs={pageReadAt}>
            <ObservationEmpty readAtUnixMs={pageReadAt} />
          </PartialFailure>
        ) : data?.freshness?.report_received?.status === 'not_measured' && (data?.works?.length ?? 0) === 0 ? (
          <Space direction="vertical" size={8} style={{ width: '100%' }}>
            <NotReportedYet />
            <ObservationEmpty readAtUnixMs={pageReadAt} />
          </Space>
        ) : (
          <ObservationEmpty readAtUnixMs={pageReadAt} />
        )}
      </Card>
    </Space>
  );
}
