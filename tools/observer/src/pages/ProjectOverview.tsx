import { useQueries, useQuery, type UseQueryResult } from '@tanstack/react-query';
import {
  Alert,
  Card,
  Col,
  Descriptions,
  Empty,
  Flex,
  List,
  Pagination,
  Row,
  Space,
  Spin,
  Table,
  Tag,
  Tooltip,
  Typography,
  type TableProps,
} from 'antd';
import type { ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import { Link, useSearchParams } from 'react-router';
import { FreshnessBadge } from '../components/FreshnessBadge';
import { ObservationEmpty, ObservationForbidden, PartialFailure } from '../components/states';
import { FieldLabel, ObservationValue } from '../components/ObservationValue';
import { isAuthError } from '../lib/api';
import { formatUnixMs } from '../lib/format';
import { activityQueryOptions, overviewQueryOptions, workSnapshotQueryOptions } from '../lib/queries';
import { useRevocation } from '../lib/revocation';
import type {
  TeamActivityItem,
  TeamActivityResponse,
  TeamOverviewResponse,
  TeamReadResponse,
  TeamWork,
  WorkSnapshotData,
} from '../lib/types';

const PAGE_SIZE = 60;

type SnapshotQuery = UseQueryResult<TeamReadResponse<WorkSnapshotData>>;
type ActivityQuery = UseQueryResult<TeamActivityResponse, unknown>;
type DisplayRow =
  | { kind: 'group'; key: string; id: string; title: string }
  | { kind: 'work'; key: string; work: TeamWork; index: number };

function rawText(value: unknown): string | null {
  if (value == null) return null;
  if (typeof value === 'string') return value;
  if (typeof value === 'number' || typeof value === 'boolean') return String(value);
  return JSON.stringify(value) ?? null;
}

function validPage(value: string | null) {
  if (!value || !/^[1-9]\d*$/.test(value)) return 1;
  const page = Number(value);
  return Number.isSafeInteger(page) ? page : 1;
}

function taskPath(project: string, workId: string) {
  return `/projects/${encodeURIComponent(project)}?tab=plan&work=${encodeURIComponent(workId)}`;
}

function groupedWorks(works: TeamWork[], workstreams: TeamOverviewResponse['workstreams']) {
  const groups = workstreams.map((workstream) => ({
    id: workstream.id,
    title: rawText(workstream.title) ?? rawText(workstream.external_key) ?? workstream.id,
    works: works.filter((work) => work.workstream_id === workstream.id),
  }));
  const known = new Set(workstreams.map((workstream) => workstream.id));
  const remaining = works.filter((work) => !known.has(work.workstream_id));
  return {
    works: [...groups.flatMap((group) => group.works), ...remaining],
    title: (id: string) => groups.find((group) => group.id === id)?.title ?? id,
  };
}

function NextStep({ work, query }: { work: TeamWork; query?: SnapshotQuery }) {
  const { t } = useTranslation();
  const data = query?.data?.result.data;
  const snapshotNext = data?.next_step ?? data?.guidance?.action?.op;
  const fullNote = data?.guidance?.action?.note;
  let text = t('common.loading');
  if (query?.isError) text = rawText(work.next_step) ?? t('common.notProvided');
  else if (query && !query.isPending) {
    text = rawText(snapshotNext ?? work.next_step) ?? t('common.notProvided');
  }
  // The compact op never truncates, so ellipsis.tooltip would never fire:
  // the full agent-facing note gets an always-on Tooltip instead.
  return (
    <Tooltip title={fullNote ?? undefined}>
      <Typography.Text data-testid={`project-next-${work.key}`} style={{ display: 'block' }}>
        {text}
      </Typography.Text>
    </Tooltip>
  );
}

function LastReport({ work, query }: { work: TeamWork; query?: SnapshotQuery }) {
  const { t, i18n } = useTranslation();
  const reportedAt = query?.data?.result.data.observation?.reporting?.client_reported_at_unix_ms;
  let text = t('common.loading');
  if (query?.isError) text = t('freshness.state.not_measured');
  else if (query && !query.isPending) {
    text = reportedAt == null ? t('freshness.state.not_measured') : formatUnixMs(reportedAt, i18n.language);
  }
  return <span data-testid={`last-report-${work.key}`}>{text}</span>;
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
    .map((part) => rawText(part.next_cursor))
    .filter((cursor): cursor is string => cursor !== null);
  const truncated = coverage.truncated
    || coverage.scope.truncated
    || coverage.streams.some((stream) => stream.truncated)
    || cursors.length > 0;

  return (
    <Card size="small" data-testid="project-coverage">
      <Flex vertical gap={16}>
        <Typography.Title level={5} style={{ margin: 0 }}>
          {t('projectOverview.coverage.title')}
        </Typography.Title>
        <FreshnessBadge freshness={data.freshness} />
        <Descriptions
          bordered
          size="small"
          column={{ xs: 1, sm: 2, lg: 4 }}
          items={[
            {
              key: 'scope',
              label: t('projectOverview.coverage.scope'),
              children: <Tag>{t(`coverage.scope.${coverage.scope.status}`)}</Tag>,
            },
            {
              key: 'enumerated',
              label: t('projectOverview.coverage.enumerated'),
              children: <span data-testid="coverage-enumerated">{coverage.scope.enumerated ?? coverage.works_enumerated}</span>,
            },
            {
              key: 'succeeded',
              label: t('projectOverview.coverage.succeeded'),
              children: coverage.succeeded,
            },
            {
              key: 'failed',
              label: t('projectOverview.coverage.failed'),
              children: coverage.failed,
            },
            {
              key: 'truncated',
              label: t('projectOverview.coverage.truncated'),
              children: t(coverage.truncated ? 'projectOverview.coverage.yes' : 'projectOverview.coverage.no'),
            },
            {
              key: 'cursor',
              label: t('projectOverview.coverage.nextCursor'),
              children: rawText(coverage.scope.next_cursor) ?? '—',
            },
            {
              key: 'works-enumerated',
              label: t('projectOverview.coverage.worksEnumerated'),
              children: coverage.works_enumerated,
            },
            {
              key: 'works-total',
              label: t('projectOverview.coverage.worksTotal'),
              children: <span data-testid="coverage-works-total">{coverage.works_total ?? t('coverage.totalUnknown')}</span>,
            },
          ]}
        />
        {failedStreams.map((stream) => (
          <div key={stream.workstream_id} data-testid={`coverage-stream-${stream.workstream_id}`}>
            <Space wrap>
              <Typography.Text strong>{stream.workstream_id}</Typography.Text>
              <Tag color="error">{t('coverage.scope.failed')}</Tag>
              {stream.error_code ? <Typography.Text code>{stream.error_code}</Typography.Text> : null}
              <Typography.Text type="secondary">
                {stream.read_at_unix_ms == null
                  ? t('freshness.state.not_measured')
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
            description={cursors.length > 0 ? cursors.join(' · ') : undefined}
          />
        ) : null}
      </Flex>
    </Card>
  );
}

function ActivityItems({ items }: { items: TeamActivityItem[] }) {
  const { i18n } = useTranslation();
  const latest = [...items]
    .sort((left, right) => right.created_at_unix_ms - left.created_at_unix_ms)
    .slice(0, 20);
  return (
    <List
      size="small"
      dataSource={latest}
      rowKey="id"
      renderItem={(item) => (
        <List.Item>
          <div data-testid={`activity-item-${item.id}`}>
            <Space wrap size={[12, 4]}>
              <Typography.Text code data-testid="activity-action">{item.action}</Typography.Text>
              <Typography.Text data-testid="activity-actor">{item.actor_id}</Typography.Text>
              <ObservationValue testId="activity-result" value={item.result} />
              <Typography.Text type="secondary" data-testid="activity-time">
                {formatUnixMs(item.created_at_unix_ms, i18n.language)}
              </Typography.Text>
            </Space>
          </div>
        </List.Item>
      )}
    />
  );
}

function RecentActivity({ query }: { query: ActivityQuery }) {
  const { t } = useTranslation();
  const data = query.data?.data;
  const errorCode = (query.error as { code?: string } | null)?.code;
  let content: ReactNode = <Spin />;
  if (errorCode === 'Unsupported') {
    content = <Empty data-testid="activity-unavailable" description={t('projectOverview.activity.unavailable')} />;
  } else if (query.error) {
    content = (
      <PartialFailure errorCode={errorCode}>
        {data?.items.length ? <ActivityItems items={data.items} /> : <div />}
      </PartialFailure>
    );
  } else if (data) {
    content = data.items.length === 0 ? <ObservationEmpty /> : <ActivityItems items={data.items} />;
  }

  return (
    <section data-testid="project-activity">
      <Typography.Title level={5}>{t('projectOverview.activity.title')}</Typography.Title>
      {data ? (
        <Typography.Paragraph type="secondary">
          <FieldLabel field="development_source" />: <ObservationValue value={data.development_source} />
          <br />
          <FieldLabel field="unknown_means" />: <ObservationValue value={data.unknown_means} />
        </Typography.Paragraph>
      ) : null}
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
  const grouped = groupedWorks(data?.works ?? [], data?.workstreams ?? []);
  const page = Math.min(
    validPage(searchParams.get('page')),
    Math.max(1, Math.ceil(grouped.works.length / PAGE_SIZE)),
  );
  const visibleWorks = grouped.works.slice((page - 1) * PAGE_SIZE, page * PAGE_SIZE);
  const snapshots = useQueries({
    queries: visibleWorks.map((work) => workSnapshotQueryOptions(project, work.key, work.workstream_id, revoked)),
  });

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
        title: grouped.title(currentWorkstream),
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
          {rawText(row.work.owner_person ?? row.work.agent) ?? t('common.notProvided')}
        </span>
      ) : null,
    },
    {
      title: t('projectOverview.columns.status'),
      key: 'status',
      width: 170,
      onCell: (row) => ({ colSpan: row.kind === 'group' ? 0 : 1 }),
      render: (_, row) => row.kind === 'work' ? (
        <ObservationValue testId={`project-status-${row.work.key}`} value={row.work.status} />
      ) : null,
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
      width: 160,
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
          <RecentActivity query={activity} />
        </Col>
        <Col xs={24} lg={8}>
          <LaterBlock title={t('projectOverview.dependencies')} testId="project-deps-placeholder" />
        </Col>
      </Row>
      {data ? <Coverage data={data} /> : null}
    </Flex>
  );
}
