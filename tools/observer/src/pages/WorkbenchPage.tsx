import { useQuery } from '@tanstack/react-query';
import { Alert, Card, Space, Typography } from 'antd';
import { useTranslation } from 'react-i18next';
import { FreshnessBadge } from '../components/FreshnessBadge';
import { NotReportedYet, ObservationEmpty, ObservationForbidden, PartialFailure } from '../components/states';
import { isAuthError } from '../lib/api';
import { overviewQueryOptions, projectsQueryOptions } from '../lib/queries';
import { useRevocation } from '../lib/revocation';
import { useScope } from '../lib/scope';
import type { TeamOverviewResponse } from '../lib/types';

function CoverageSummary({ data }: { data: TeamOverviewResponse }) {
  const { t } = useTranslation();
  return (
    <Typography.Paragraph
      data-testid="coverage-summary"
      type="secondary"
      style={{ marginTop: 12, marginBottom: 0 }}
    >
      {t('coverage.summary', {
        scope: t(`coverage.scope.${data.coverage.scope.status}`),
        succeeded: data.coverage.succeeded,
        failed: data.coverage.failed,
        enumerated: data.coverage.works_enumerated,
        total: data.coverage.works_total ?? t('coverage.totalUnknown'),
      })}
    </Typography.Paragraph>
  );
}

function PendingWorkList({ data }: { data: TeamOverviewResponse }) {
  const { t } = useTranslation();
  return (
    <div data-testid="state-list-pending">
      <Alert
        type="info"
        showIcon
        title={t('state.listPending.title')}
        description={t('state.listPending.description')}
      />
      <CoverageSummary data={data} />
    </div>
  );
}

/** The overview is live; the work list itself remains an explicit later-slice view. */
export function WorkbenchPage() {
  const { t } = useTranslation();
  const { projectKey } = useScope();
  const { revoked } = useRevocation();
  const projects = useQuery(projectsQueryOptions(revoked));
  const effectiveProject = projectKey ?? projects.data?.projects?.[0]?.key ?? null;
  const overview = useQuery(overviewQueryOptions(effectiveProject, revoked));

  if (revoked || isAuthError(projects.error) || isAuthError(overview.error)) {
    return <ObservationForbidden />;
  }

  const data = overview.data;
  const failed = data?.coverage?.failed ?? 0;
  const pageReadAt = data?.freshness?.page_read?.at_unix_ms;
  const empty = data?.coverage.scope.status === 'succeeded'
    && data.coverage.works_enumerated === 0
    && failed === 0;

  let content = data ? <PendingWorkList data={data} /> : <ObservationEmpty readAtUnixMs={pageReadAt} />;
  if (data && empty) {
    content = data.freshness.report_received.status === 'not_measured' ? (
      <Space direction="vertical" size={8} style={{ width: '100%' }}>
        <NotReportedYet />
        <ObservationEmpty readAtUnixMs={pageReadAt} />
      </Space>
    ) : <ObservationEmpty readAtUnixMs={pageReadAt} />;
  }
  if (overview.error) {
    content = (
      <PartialFailure errorCode={(overview.error as { code?: string }).code} confirmedAtUnixMs={pageReadAt}>
        {content}
      </PartialFailure>
    );
  } else if (failed > 0 && data) {
    content = (
      <PartialFailure
        errorCode={data.coverage.streams.find((stream) => stream.status === 'failed')?.error_code}
        confirmedAtUnixMs={pageReadAt}
      >
        {content}
      </PartialFailure>
    );
  }

  return (
    <Space direction="vertical" size={16} style={{ width: '100%' }} data-testid="page-workbench">
      <Card>
        <Typography.Title level={5} style={{ marginTop: 0 }}>
          {t('freshness.title')}
        </Typography.Title>
        <FreshnessBadge freshness={data?.freshness} />
        {data && empty ? <CoverageSummary data={data} /> : null}
      </Card>
      <Card>{content}</Card>
    </Space>
  );
}
