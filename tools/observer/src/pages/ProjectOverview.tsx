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
import { ToneBadge } from '../components/ToneBadge';
import { contractGroups } from '../lib/contract-groups';
import { deriveWorkState, groupWorks, moreLinesText, rawObservationText } from '../core/observation';
import { readStateOf } from '../lib/read-state';
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
  WorkContractValue,
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
      <span
        data-reporter-text={state.next.kind === 'reported' ? '' : undefined}
        style={{ display: '-webkit-box', WebkitLineClamp: 3, WebkitBoxOrient: 'vertical', overflow: 'hidden' }}
      >
        {state.next.text}
      </span>
    </Typography.Text>
  );
}

function LastReport({ work, query }: { work: TeamWork; query?: SnapshotQuery }) {
  const state = useWorkState(work, query);
  return <LastReportText report={state.lastReport} testId={`last-report-${work.key}`} />;
}

/** The goals and acceptance the works on this page have declared, merged; only reads already made. */
function GoalsBlock({ works, snapshots }: { project: string; works: TeamWork[]; snapshots: SnapshotQuery[] }) {
  const { t } = useTranslation();
  const GROUP_LIMIT = 5;
  const read = works.map((work, index) => {
    const query = snapshots[index];
    const contract = query?.data?.result.data?.visible_contract;
    return {
      title: work.title,
      // A read that did not happen contributes nothing; it is counted, never guessed from.
      state: readStateOf(query),
      goals: contract?.goals as WorkContractValue[] | undefined,
      acceptance: contract?.acceptance as WorkContractValue[] | undefined,
    };
  });
  const finished = read.filter((entry) => entry.state === 'ready');
  const groups = contractGroups(finished);
  const loading = read.filter((entry) => entry.state === 'loading').length;
  const failed = read.filter((entry) => entry.state === 'failed').length;
  const shown = groups.slice(0, GROUP_LIMIT);
  const more = groups.length - shown.length;

  return (
    <section data-testid="project-goals">
      <Typography.Title level={5}>{t('projectOverview.goals')}</Typography.Title>
      {/* Reading is not failing and it is not "nothing recorded": while no read has come
          back the block says so; "none record agreements" needs a finished read to stand on. */}
      {read.length > 0 && finished.length === 0 && loading > 0
        ? <Typography.Text type="secondary" data-testid="project-goals-loading">{t('common.loading')}</Typography.Text>
        : read.length > 0 && finished.length === 0
          ? <Typography.Text type="secondary" data-testid="project-goals-failed">{t('projectOverview.goalsFailed')}</Typography.Text>
          : shown.length === 0
            ? <Typography.Text type="secondary">{t('projectOverview.goalsEmpty')}</Typography.Text>
            : (
              <Flex vertical gap={12}>
                {shown.map((group, index) => (
                  <div key={index} data-testid="project-goals-group">
                    <Typography.Text type="secondary" style={{ fontSize: 12 }}>
                      {t('projectOverview.goalsOf', { titles: group.titles.join(t('knowledge.titleSeparator')) })}
                    </Typography.Text>
                    {group.goals?.length ? (
                      <div>
                        <Typography.Text strong style={{ fontSize: 12 }}>{t('workbench.detail.goals')}</Typography.Text>
                        <ul style={{ margin: '2px 0', paddingLeft: 20 }}>
                          {group.goals.map((goal, goalIndex) => (
                            <li key={goalIndex}>{rawObservationText(goal).text}</li>
                          ))}
                        </ul>
                      </div>
                    ) : null}
                    {group.acceptance?.length ? (
                      <div>
                        <Typography.Text strong style={{ fontSize: 12 }}>{t('workbench.detail.acceptance')}</Typography.Text>
                        <ul style={{ margin: '2px 0', paddingLeft: 20 }}>
                          {/* No checkbox: nothing in this data says whether an acceptance item is met;
                              that is the outcome page's question, and a box here would invent it. */}
                          {group.acceptance.map((item, itemIndex) => (
                            <li key={itemIndex}>{rawObservationText(item).text}</li>
                          ))}
                        </ul>
                      </div>
                    ) : null}
                  </div>
                ))}
                {more > 0 ? (
                  <Typography.Text type="secondary" data-testid="project-goals-more">
                    {t('projectOverview.goalsMoreGroups', { count: more })}
                  </Typography.Text>
                ) : null}
              </Flex>
            )}
      {loading + failed > 0 ? (
        <Typography.Text type="warning" data-testid="project-goals-unread">
          {t('projectOverview.goalsUnread', { count: loading + failed })}
        </Typography.Text>
      ) : null}
    </section>
  );
}

/** What the reads on this page actually say about delivery: counts per progress word and the works that need attention — never a whole-project verdict. */
function DeliveryBlock({ project, works, snapshots, coverage }: {
  project: string;
  works: TeamWork[];
  snapshots: SnapshotQuery[];
  coverage: TeamOverviewResponse['coverage'] | undefined;
}) {
  const { t, i18n } = useTranslation();
  const entries = works.map((work, index) => {
    const query = snapshots[index];
    const readState = readStateOf(query);
    const state = deriveWorkState({
      language: i18n.language,
      readState,
      workNextStep: work.next_step,
      workStatus: work.status,
      snapshot: query?.data?.result.data,
    });
    return { work, readState, progress: state.progress };
  });
  // Only a read that happened counts; loading and failed works are named separately and
  // counted nowhere, least of all as "in progress".
  const counted = entries.filter((entry) => entry.readState === 'ready');
  const loading = entries.filter((entry) => entry.readState === 'loading').length;
  const failed = entries.filter((entry) => entry.readState === 'failed').length;
  const counts = new Map<string, { tone: string; works: TeamWork[] }>();
  for (const entry of counted) {
    const bucket = counts.get(entry.progress.text) ?? { tone: entry.progress.tone, works: [] };
    bucket.works.push(entry.work);
    counts.set(entry.progress.text, bucket);
  }
  const ATTENTION_LIMIT = 5;
  // The list exists to name the works a person must look at: the waiting words (warning)
  // and the blocked, failed, rejected ones (negative) — never the positive or neutral words.
  const attention = counted.filter((entry) => entry.progress.tone === 'warning' || entry.progress.tone === 'negative');
  const attentionShown = attention.slice(0, ATTENTION_LIMIT);
  const attentionMore = attention.length - attentionShown.length;
  // The project total is the coverage card's own number, never the length of this list:
  // a cut-short list would put a second, smaller total on the page.
  const total = coverage?.works_total ?? null;
  const scopeText = total == null
    ? t('projectOverview.deliveryScopeUnknownTotal', { page: works.length, read: counted.length })
    : t('projectOverview.deliveryScope', { page: works.length, read: counted.length, total });

  return (
    <section data-testid="project-delivery">
      <Typography.Title level={5}>{t('projectOverview.delivery')}</Typography.Title>
      {loading > 0 && counted.length === 0
        ? (
          <Typography.Text type="secondary" data-testid="project-delivery-loading">
            {t('common.loading')}
          </Typography.Text>
        )
        : counted.length === 0
          ? (
            <Typography.Text type="secondary" data-testid="project-delivery-failed">
              {t('projectOverview.deliveryFailed')}
            </Typography.Text>
          )
          : (
            <Flex vertical gap={12}>
              <Typography.Text type="secondary" data-testid="project-delivery-scope">
                {scopeText}
              </Typography.Text>
              <Space wrap size={[16, 8]} data-testid="project-delivery-counts">
                {[...counts.entries()].map(([text, bucket]) => (
                  <Space key={text} size={4}>
                    <ToneBadge tone={bucket.tone as 'positive' | 'warning' | 'negative' | 'neutral'} text={text} />
                    <Typography.Text type="secondary">× {bucket.works.length}</Typography.Text>
                  </Space>
                ))}
              </Space>
              {attention.length > 0 ? (
                <div data-testid="project-delivery-attention">
                  <Typography.Text strong>{t('projectOverview.deliveryAttention')}</Typography.Text>
                  <ul style={{ margin: '4px 0', paddingLeft: 20 }}>
                    {attentionShown.map(({ work, progress }) => (
                      // The link text carries the progress word, so its accessible name stays
                      // distinct from the same work's plain link in the table below.
                      <li key={work.key}>
                        <Link to={taskPath(project, work.key)} data-testid={`project-delivery-attention-${work.key}`}>
                          {work.title}
                          {t('knowledge.titleSeparator')}
                          {progress.text}
                        </Link>
                      </li>
                    ))}
                  </ul>
                  {attentionMore > 0 ? (
                    <Typography.Text type="secondary">{moreLinesText(attentionMore, i18n.language)}</Typography.Text>
                  ) : null}
                </div>
              ) : null}
              {loading + failed > 0 ? (
                <Typography.Text type="warning" data-testid="project-delivery-unread">
                  {t('projectOverview.deliveryUnread', { count: loading + failed })}
                </Typography.Text>
              ) : null}
            </Flex>
          )}
    </section>
  );
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
          <GoalsBlock project={project} works={visibleWorks} snapshots={snapshots} />
        </Col>
        <Col xs={24} lg={12}>
          <DeliveryBlock project={project} works={visibleWorks} snapshots={snapshots} coverage={data?.coverage} />
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
