import { useQuery, type UseQueryResult } from '@tanstack/react-query';
import {
  Alert,
  Card,
  Col,
  Empty,
  Flex,
  Pagination,
  Row,
  Space,
  Spin,
  Table,
  Tag,
  Typography,
  type TableProps,
} from 'antd';
import type { ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import { Link, useSearchParams } from 'react-router';
import { FreshnessBadge } from '../components/FreshnessBadge';
import { LastReportText } from '../components/LastReportText';
import { WorkProgress, useWorkState } from '../components/WorkProgress';
import { groupWorks, rawObservationText } from '../core/observation';
import { RecentChanges } from '../components/RecentChanges';
import { ObservationEmpty, ObservationForbidden, PartialFailure } from '../components/states';
import { TechnicalDetails } from '../components/TechnicalDetails';
import { pageTechnicalItems } from '../lib/activity-technical';
import { isAuthError } from '../lib/api';
import { freshnessWord } from '../lib/freshness';
import { formatUnixMs } from '../lib/format';
import { activityQueryOptions, overviewQueryOptions } from '../lib/queries';
import { useRevocation } from '../lib/revocation';
import { useWorkSnapshots, type SnapshotQuery } from '../lib/work-snapshots';
import type {
  TeamActivityItem,
  TeamActivityResponse,
  TeamOverviewResponse,
  TeamWork,
} from '../lib/types';

const PAGE_SIZE = 60;

type ActivityQuery = UseQueryResult<TeamActivityResponse, unknown>;
type DisplayRow =
  | { kind: 'group'; key: string; id: string; title: string }
  | { kind: 'work'; key: string; work: TeamWork; index: number };

function validPage(value: string | null) {
  if (!value || !/^[1-9]\d*$/.test(value)) return 1;
  const page = Number(value);
  return Number.isSafeInteger(page) ? page : 1;
}

function taskPath(project: string, workId: string) {
  return `/projects/${encodeURIComponent(project)}?tab=plan&work=${encodeURIComponent(workId)}`;
}

function NextStep({ work, query }: { work: TeamWork; query?: SnapshotQuery }) {
  const state = useWorkState(work, query);
  return (
    <Typography.Text
      data-testid={`project-next-${work.key}`}
      title={state.next.fullText ?? undefined}
      style={{ display: 'block' }}
    >
      {/* antd's Typography overwrites a line clamp set on itself, so the clamp sits on a plain span. */}
      <span style={{ display: '-webkit-box', WebkitLineClamp: 3, WebkitBoxOrient: 'vertical', overflow: 'hidden' }}>
        {state.next.text}
      </span>
    </Typography.Text>
  );
}

function LastReport({ work, query }: { work: TeamWork; query?: SnapshotQuery }) {
  const state = useWorkState(work, query);
  return <LastReportText report={state.lastReport} testId={`last-report-${work.key}`} />;
}

function LaterBlock({ title, testId }: { title: string; testId: string }) {
  const { t } = useTranslation();
  return (
    <section data-testid={testId}>
      <Typography.Title level={5}>{title}</Typography.Title>
      <Typography.Text strong>{t('state.placeholder.title')}</Typography.Text>
      <Typography.Paragraph type="secondary">
        {t('state.placeholder.description')}
      </Typography.Paragraph>
    </section>
  );
}

function Coverage({ data }: { data: TeamOverviewResponse }) {
  const { t, i18n } = useTranslation();
  const { coverage } = data;
  const failedStreams = coverage.streams.filter((stream) => stream.status === 'failed');
  const cursors = [coverage.scope, ...coverage.streams]
    .map((part) => rawObservationText(part.next_cursor).text)
    .filter((cursor): cursor is string => cursor !== null);
  const truncated = coverage.truncated
    || coverage.scope.truncated
    || coverage.streams.some((stream) => stream.truncated)
    || cursors.length > 0;
  const enumerated = coverage.works_enumerated;
  const total = coverage.works_total;
  const names = groupWorks(data.works, data.workstreams).titles;

  return (
    <Card size="small" data-testid="project-coverage">
      <Flex vertical gap={16}>
        <Typography.Title level={5} style={{ margin: 0 }}>
          {t('projectOverview.coverage.title')}
        </Typography.Title>
        <FreshnessBadge freshness={data.freshness} />
        <Typography.Text data-testid="coverage-summary">
          {total == null
            ? t('projectOverview.coverage.readUnknownTotal', { enumerated })
            : t('projectOverview.coverage.readOfTotal', { enumerated, total })}
        </Typography.Text>
        {failedStreams.length > 0 ? (
          <Typography.Text type="warning" data-testid="coverage-failed-summary">
            {t('projectOverview.coverage.failedSummary', { count: failedStreams.length })}
          </Typography.Text>
        ) : null}
        {failedStreams.map((stream) => (
          <div key={stream.workstream_id} data-testid={`coverage-stream-${stream.workstream_id}`}>
            <Space wrap>
              <Typography.Text strong>{(stream.workstream_id ? names[stream.workstream_id] : undefined) ?? t('projectOverview.coverage.someWorkstream')}</Typography.Text>
              <Tag color="error">{t('coverage.scope.failed')}</Tag>
              <Typography.Text type="secondary">
                {stream.read_at_unix_ms == null
                  ? freshnessWord(t, 'not_measured')
                  : formatUnixMs(stream.read_at_unix_ms, i18n.language)}
              </Typography.Text>
            </Space>
          </div>
        ))}
        {truncated ? (
          <Alert
            type="info"
            showIcon
            data-testid="project-coverage-unread"
            message={t('projectOverview.coverage.unread')}
          />
        ) : null}
        <TechnicalDetails
          items={[
            { label: 'coverage.scope.status', value: coverage.scope.status },
            { label: 'coverage.scope.enumerated', value: coverage.scope.enumerated },
            { label: 'coverage.succeeded', value: coverage.succeeded },
            { label: 'coverage.failed', value: coverage.failed },
            { label: 'coverage.truncated', value: coverage.truncated },
            { label: 'coverage.scope.next_cursor', value: coverage.scope.next_cursor },
            { label: 'coverage.works_enumerated', value: coverage.works_enumerated },
            { label: 'coverage.works_total', value: coverage.works_total, testId: 'coverage-works-total' },
            ...coverage.streams.map((stream) => ({ label: `coverage.streams.${stream.workstream_id}`, value: stream })),
          ]}
        />
      </Flex>
    </Card>
  );
}

function RecentActivity({ query, project }: { query: ActivityQuery; project: string }) {
  const { t } = useTranslation();
  const data = query.data?.data;
  const errorCode = (query.error as { code?: string } | null)?.code;
  const changes = (items: TeamActivityItem[]) => (
    <RecentChanges
      items={items}
      maxSteps={8}
      viewAllHref={`/activity?project=${encodeURIComponent(project)}`}
      testId="project-recent-changes"
      extraTechnical={pageTechnicalItems(query.data?.data)}
    />
  );
  let content: ReactNode = <Spin />;
  if (errorCode === 'Unsupported') {
    content = <Empty data-testid="activity-unavailable" description={t('projectOverview.activity.unavailable')} />;
  } else if (query.error) {
    content = (
      <PartialFailure errorCode={errorCode}>
        {data?.items.length ? changes(data.items) : <div />}
      </PartialFailure>
    );
  } else if (data) {
    content = data.items.length === 0 ? <ObservationEmpty /> : changes(data.items);
  }

  return (
    <section data-testid="project-activity">
      <Typography.Title level={5}>{t('projectOverview.activity.title')}</Typography.Title>
      {content}
      {data?.next_cursor != null ? (
        <Alert type="info" showIcon message={t('projectOverview.activity.earlier')} />
      ) : null}
    </section>
  );
}

export function ProjectOverview({ project }: { project: string }) {
  const { t } = useTranslation();
  const { revoked } = useRevocation();
  const [searchParams, setSearchParams] = useSearchParams();
  const overview = useQuery(overviewQueryOptions(project, revoked));
  const activity = useQuery(activityQueryOptions(project, revoked));
  const data = overview.data;
  const grouped = groupWorks(data?.works ?? [], data?.workstreams ?? []);
  const page = Math.min(
    validPage(searchParams.get('page')),
    Math.max(1, Math.ceil(grouped.works.length / PAGE_SIZE)),
  );
  const visibleWorks = grouped.works.slice((page - 1) * PAGE_SIZE, page * PAGE_SIZE);
  const snapshots = useWorkSnapshots(project, visibleWorks, revoked);

  if (revoked || isAuthError(overview.error) || isAuthError(activity.error)) {
    return <ObservationForbidden />;
  }

  function selectPage(nextPage: number) {
    const next = new URLSearchParams(searchParams);
    if (nextPage === 1) next.delete('page');
    else next.set('page', String(nextPage));
    next.delete('work');
    setSearchParams(next);
  }

  const displayRows: DisplayRow[] = [];
  let currentWorkstream: string | null = null;
  visibleWorks.forEach((work, index) => {
    if (work.workstream_id !== currentWorkstream) {
      currentWorkstream = work.workstream_id;
      displayRows.push({
        kind: 'group',
        key: `group-${currentWorkstream}`,
        id: currentWorkstream,
        title: (grouped.titles[currentWorkstream] ?? currentWorkstream),
      });
    }
    displayRows.push({ kind: 'work', key: work.key, work, index });
  });

  const columns: TableProps<DisplayRow>['columns'] = [
    {
      title: t('projectOverview.columns.work'),
      key: 'work',
      width: 220,
      onCell: (row) => ({ colSpan: row.kind === 'group' ? 5 : 1 }),
      render: (_, row) => row.kind === 'group'
        ? <Typography.Text strong data-testid={`workstream-group-${row.id}`}>{row.title}</Typography.Text>
        : <Link to={taskPath(project, row.work.key)}>{row.work.title}</Link>,
    },
    {
      title: t('projectOverview.columns.owner'),
      key: 'owner',
      width: 150,
      onCell: (row) => ({ colSpan: row.kind === 'group' ? 0 : 1 }),
      render: (_, row) => row.kind === 'work' ? (
        <span data-testid={`project-owner-${row.work.key}`}>
          {rawObservationText(row.work.owner_person ?? row.work.agent).text ?? t('common.notProvided')}
        </span>
      ) : null,
    },
    {
      title: t('projectOverview.columns.status'),
      key: 'status',
      width: 170,
      onCell: (row) => ({ colSpan: row.kind === 'group' ? 0 : 1 }),
      render: (_, row) => row.kind === 'work'
        ? <WorkProgress work={row.work} query={snapshots[row.index]} testId={`project-status-${row.work.key}`} />
        : null,
    },
    {
      title: t('projectOverview.columns.next'),
      key: 'next',
      width: 260,
      onCell: (row) => ({ colSpan: row.kind === 'group' ? 0 : 1 }),
      render: (_, row) => row.kind === 'work'
        ? <NextStep work={row.work} query={snapshots[row.index]} />
        : null,
    },
    {
      title: t('projectOverview.columns.lastReport'),
      key: 'lastReport',
      width: 220,
      onCell: (row) => ({ colSpan: row.kind === 'group' ? 0 : 1 }),
      render: (_, row) => row.kind === 'work'
        ? <LastReport work={row.work} query={snapshots[row.index]} />
        : null,
    },
  ];

  const table = (
    <Flex vertical gap={16}>
      <Table<DisplayRow>
        data-testid="project-progress-table"
        rowKey="key"
        columns={columns}
        dataSource={displayRows}
        tableLayout="fixed"
        scroll={{ x: 960 }}
        pagination={false}
      />
      <Pagination
        current={page}
        pageSize={PAGE_SIZE}
        total={grouped.works.length}
        showSizeChanger={false}
        onChange={selectPage}
        style={{ display: 'flex', justifyContent: 'flex-end' }}
      />
    </Flex>
  );

  let progress: ReactNode = <Spin />;
  if (overview.error) {
    progress = (
      <PartialFailure errorCode={(overview.error as { code?: string }).code}>
        <div />
      </PartialFailure>
    );
  } else if (data) {
    const empty = data.coverage.scope.status === 'succeeded'
      && data.coverage.scope.enumerated === 0
      && data.coverage.failed === 0;
    progress = empty
      ? <ObservationEmpty readAtUnixMs={data.freshness.page_read.at_unix_ms} />
      : table;
    const failedStream = data.coverage.streams.find((stream) => stream.status === 'failed');
    if (data.coverage.failed > 0 || failedStream) {
      progress = (
        <PartialFailure
          errorCode={failedStream?.error_code}
          confirmedAtUnixMs={failedStream?.read_at_unix_ms}
        >
          {progress}
        </PartialFailure>
      );
    } else if (visibleWorks.length > 0 && snapshots.every((snapshot) => snapshot.isError)) {
      progress = (
        <PartialFailure errorCode={(snapshots[0]?.error as { code?: string } | undefined)?.code}>
          {progress}
        </PartialFailure>
      );
    }
  }

  return (
    <Flex vertical gap={24} data-testid="project-overview" style={{ minWidth: 0 }}>
      <Row gutter={[16, 16]}>
        <Col xs={24} lg={12}>
          <LaterBlock title={t('projectOverview.goals')} testId="project-goals-placeholder" />
        </Col>
        <Col xs={24} lg={12}>
          <LaterBlock title={t('projectOverview.delivery')} testId="project-delivery-placeholder" />
        </Col>
      </Row>
      <section>
        <Typography.Title level={5}>{t('projectOverview.progress')}</Typography.Title>
        {progress}
      </section>
      <Row gutter={[16, 16]}>
        <Col xs={24} lg={16} style={{ minWidth: 0 }}>
          <RecentActivity query={activity} project={project} />
        </Col>
        <Col xs={24} lg={8}>
          <LaterBlock title={t('projectOverview.dependencies')} testId="project-deps-placeholder" />
        </Col>
      </Row>
      {data ? <Coverage data={data} /> : null}
    </Flex>
  );
}
