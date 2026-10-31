import { useQueries, useQuery, type UseQueryResult } from '@tanstack/react-query';
import {
  Alert,
  Card,
  Col,
  Flex,
  List,
  Row,
  Space,
  Spin,
  Table,
  Tabs,
  Tag,
  Typography,
  type TableProps,
} from 'antd';
import type { TFunction } from 'i18next';
import { Link as LinkIcon } from 'lucide-react';
import { useEffect, useState, type KeyboardEvent, type ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import { Link, useSearchParams } from 'react-router';
import { FreshnessBadge } from '../components/FreshnessBadge';
import { NotReportedYet, ObservationEmpty, ObservationForbidden, PartialFailure } from '../components/states';
import { isAuthError } from '../lib/api';
import { formatUnixMs } from '../lib/format';
import {
  projectsQueryOptions,
  workInboxQueryOptions,
  workNextQueryOptions,
  workSnapshotQueryOptions,
} from '../lib/queries';
import { useRevocation } from '../lib/revocation';
import { useScope } from '../lib/scope';
import type {
  TeamReadResponse,
  WorkContractValue,
  WorkNavigation,
  WorkQuery,
  WorkSnapshotData,
} from '../lib/types';

const PAGE_SIZE = 60;
const TAB_KEYS = ['in-progress', 'needs-me', 'participate', 'delivered'] as const;
const NAVIGATIONS: readonly WorkNavigation[] = [
  'resume',
  'recovery_required',
  'handoff_required',
  'held',
  'waiting_external',
  'waiting_dependency',
  'observe',
  'prepare',
];

type TabKey = (typeof TAB_KEYS)[number];

interface WorkbenchRow {
  key: string;
  title: string;
  work_id: string;
  workstream_id: string;
  state?: 'claimed' | null;
  navigation?: string;
  next_query: WorkQuery;
}

type SnapshotQuery = UseQueryResult<TeamReadResponse<WorkSnapshotData>>;

function validTab(value: string | null): TabKey {
  return (TAB_KEYS as readonly string[]).includes(value ?? '') ? value as TabKey : 'in-progress';
}

function taskPath(project: string, workId: string) {
  return `/projects/${encodeURIComponent(project)}?tab=plan&work=${encodeURIComponent(workId)}`;
}

function navigationText(navigation: string | undefined, t: TFunction) {
  if (!navigation) return null;
  return NAVIGATIONS.includes(navigation as WorkNavigation)
    ? t(`workbench.navigation.${navigation as WorkNavigation}`)
    : navigation;
}

function nextActionText(query: WorkQuery, t: TFunction) {
  if (query.op === 'work.observe') return t('workbench.nextAction.observe');
  if (query.op === 'work.prepare') return t('workbench.nextAction.prepare');
  return t('workbench.nextAction.advisory', { op: query.op });
}

function LastReport({ workId, query }: { workId: string; query?: SnapshotQuery }) {
  const { t, i18n } = useTranslation();
  const observation = query?.data?.result.data.observation;
  const lastActivity = observation?.last_activity_at_unix_ms ?? observation?.reported_at;
  let text = t('common.loading');
  if (query?.isError) text = t('freshness.state.not_measured');
  else if (query && !query.isPending) {
    text = lastActivity == null
      ? t('freshness.state.not_measured')
      : formatUnixMs(lastActivity, i18n.language);
  }
  return <span data-testid={`last-report-${workId}`}>{text}</span>;
}

function DetailAbsent() {
  const { t } = useTranslation();
  return (
    <Typography.Text data-testid="detail-absent" type="secondary">
      {t('workbench.detail.absent')}
    </Typography.Text>
  );
}

function ContractValues({ values }: { values: WorkContractValue[] }) {
  const { t } = useTranslation();
  if (values.length === 0) {
    return <Typography.Text type="secondary">{t('workbench.detail.none')}</Typography.Text>;
  }
  return (
    <List
      size="small"
      dataSource={values}
      renderItem={(value) => (
        <List.Item>
          <Typography.Text>
            {typeof value === 'object' && value !== null ? JSON.stringify(value) : String(value)}
          </Typography.Text>
        </List.Item>
      )}
    />
  );
}

function DetailSection({ title, children }: { title: string; children: ReactNode }) {
  return (
    <section>
      <Typography.Title level={4}>{title}</Typography.Title>
      {children}
    </section>
  );
}

function WorkDetail({
  project,
  row,
  snapshot,
}: {
  project: string;
  row?: WorkbenchRow;
  snapshot?: SnapshotQuery;
}) {
  const { t } = useTranslation();
  if (!row) {
    return (
      <Card data-testid="workbench-detail">
        <Typography.Text type="secondary">{t('workbench.detail.select')}</Typography.Text>
      </Card>
    );
  }

  const data = snapshot?.data?.result.data;
  const contract = data?.visible_contract ?? data?.published_contract;
  const goals = contract?.goals;
  const acceptance = contract?.acceptance;
  const dependencies = contract?.required_dependencies;
  const progress = data?.observation?.progress;
  const guidance = data?.guidance?.action;
  const nextStep = data?.next_step ?? guidance?.note ?? guidance?.op;

  return (
    <Card data-testid="workbench-detail">
      <Flex vertical gap={20}>
        <div>
          <Typography.Title level={3} style={{ marginTop: 0 }}>{row.title}</Typography.Title>
          <Space wrap>
            {row.state !== undefined
              ? <Tag color={row.state === 'claimed' ? 'processing' : 'default'}>
                  {t(row.state === 'claimed' ? 'workbench.state.claimed' : 'workbench.state.unclaimed')}
                </Tag>
              : <Tag>{t('freshness.state.not_measured')}</Tag>}
            {navigationText(row.navigation, t) ? <Tag>{navigationText(row.navigation, t)}</Tag> : null}
          </Space>
        </div>
        {snapshot?.isPending ? <Spin /> : (
          <>
            <DetailSection title={t('workbench.detail.goalsAcceptance')}>
              {goals || acceptance ? (
                <Flex vertical gap={12}>
                  {goals ? (
                    <div>
                      <Typography.Text strong>{t('workbench.detail.goals')}</Typography.Text>
                      <ContractValues values={goals} />
                    </div>
                  ) : null}
                  {acceptance ? (
                    <div>
                      <Typography.Text strong>{t('workbench.detail.acceptance')}</Typography.Text>
                      <ContractValues values={acceptance} />
                    </div>
                  ) : null}
                </Flex>
              ) : <DetailAbsent />}
            </DetailSection>
            <DetailSection title={t('workbench.detail.currentOutcome')}>
              {progress
                ? <pre className="workbench-progress">{JSON.stringify(progress, null, 2)}</pre>
                : data?.observation ? <NotReportedYet /> : <DetailAbsent />}
            </DetailSection>
            <DetailSection title={t('workbench.detail.dependencies')}>
              {dependencies ? <ContractValues values={dependencies} /> : <DetailAbsent />}
            </DetailSection>
            <DetailSection title={t('workbench.detail.nextStep')}>
              {nextStep ? <Typography.Paragraph>{nextStep}</Typography.Paragraph> : <DetailAbsent />}
            </DetailSection>
          </>
        )}
        <Link to={taskPath(project, row.work_id)}>{t('workbench.detail.openFull')}</Link>
      </Flex>
    </Card>
  );
}

function LaterSlice({ testId }: { testId?: string }) {
  const { t } = useTranslation();
  return (
    <div data-testid={testId}>
      <Typography.Text strong>{t('state.placeholder.title')}</Typography.Text>
      <Typography.Paragraph type="secondary" style={{ marginBottom: 0 }}>
        {t('state.placeholder.description')}
      </Typography.Paragraph>
    </div>
  );
}

export function WorkbenchPage() {
  const { t } = useTranslation();
  const { projectKey } = useScope();
  const { revoked } = useRevocation();
  const [searchParams, setSearchParams] = useSearchParams();
  const activeTab = validTab(searchParams.get('tab'));
  const [page, setPage] = useState(1);
  const projects = useQuery(projectsQueryOptions(revoked));
  const effectiveProject = projectKey ?? projects.data?.projects?.[0]?.key ?? null;
  const workNext = useQuery(workNextQueryOptions(effectiveProject, revoked));
  const workInbox = useQuery(workInboxQueryOptions(effectiveProject, activeTab === 'needs-me', revoked));

  useEffect(() => setPage(1), [activeTab, effectiveProject]);

  const nextData = workNext.data?.result.data;
  const inboxData = workInbox.data?.result.data;
  const sourcedTab = activeTab === 'in-progress' || activeTab === 'needs-me';
  const activeQuery = activeTab === 'needs-me' ? workInbox : workNext;
  const rows: WorkbenchRow[] = activeTab === 'needs-me'
    ? (inboxData?.items ?? []).map((item) => ({
        key: item.item_key,
        title: item.title,
        work_id: item.work_id,
        workstream_id: item.workstream_id,
        navigation: item.guidance?.code,
        next_query: item.next_query,
      }))
    : (nextData?.items ?? []).map((item) => ({ ...item, key: item.work_id }));
  const visibleRows = sourcedTab ? rows.slice((page - 1) * PAGE_SIZE, page * PAGE_SIZE) : [];
  const snapshots = useQueries({
    queries: visibleRows.map((row) =>
      workSnapshotQueryOptions(effectiveProject, row.work_id, row.workstream_id, revoked)),
  });
  const selectedWorkId = searchParams.get('work');
  const selectedIndex = visibleRows.findIndex((row) => row.work_id === selectedWorkId);
  const effectiveSelectedIndex = selectedIndex >= 0 ? selectedIndex : 0;
  const selectedRow = visibleRows[effectiveSelectedIndex];
  const selectedSnapshot = snapshots[effectiveSelectedIndex];
  const pageReadAt = activeQuery.data?.freshness.page_read.at_unix_ms;

  if (revoked || isAuthError(projects.error) || isAuthError(workNext.error) || isAuthError(workInbox.error)) {
    return <ObservationForbidden />;
  }

  function selectRow(row: WorkbenchRow) {
    const next = new URLSearchParams(searchParams);
    next.set('work', row.work_id);
    setSearchParams(next);
  }

  function selectTab(tab: string) {
    const next = new URLSearchParams(searchParams);
    next.set('tab', tab);
    next.delete('work');
    setSearchParams(next);
  }

  const columns: TableProps<WorkbenchRow>['columns'] = [
    {
      title: t('workbench.columns.task'),
      key: 'task',
      render: (_, row) => (
        <Flex vertical gap={4}>
          {effectiveProject ? (
            <Link to={taskPath(effectiveProject, row.work_id)} onClick={(event) => event.stopPropagation()}>
              {row.title}
            </Link>
          ) : row.title}
          <Typography.Text type="secondary">{row.work_id}</Typography.Text>
        </Flex>
      ),
    },
    {
      title: t('workbench.columns.progress'),
      key: 'progress',
      render: (_, row) => (
        <Space wrap>
          {row.state !== undefined
            ? <Tag color={row.state === 'claimed' ? 'processing' : 'default'}>
                {t(row.state === 'claimed' ? 'workbench.state.claimed' : 'workbench.state.unclaimed')}
              </Tag>
            : <Tag>{t('freshness.state.not_measured')}</Tag>}
          {navigationText(row.navigation, t) ? <Tag>{navigationText(row.navigation, t)}</Tag> : null}
        </Space>
      ),
    },
    {
      title: t('workbench.columns.next'),
      key: 'next',
      render: (_, row) => <Typography.Text>{nextActionText(row.next_query, t)}</Typography.Text>,
    },
    {
      title: t('workbench.columns.lastReport'),
      key: 'lastReport',
      render: (_, row, index) => <LastReport workId={row.work_id} query={snapshots[index]} />,
    },
  ];

  const table = visibleRows.length > 0 ? (
    <Table<WorkbenchRow>
      data-testid="workbench-table"
      rowKey="key"
      columns={columns}
      dataSource={visibleRows}
      scroll={{ x: 760 }}
      pagination={{
        current: page,
        pageSize: PAGE_SIZE,
        total: rows.length,
        showSizeChanger: false,
        onChange: setPage,
      }}
      onRow={(row) => ({
        tabIndex: 0,
        onClick: () => selectRow(row),
        onKeyDown: (event: KeyboardEvent<HTMLTableRowElement>) => {
          if (event.key === 'Enter' || event.key === ' ') {
            event.preventDefault();
            selectRow(row);
          }
        },
      })}
    />
  ) : null;

  const allSnapshotsFailed = visibleRows.length > 0
    && snapshots.length === visibleRows.length
    && snapshots.every((snapshot) => snapshot.isError);
  const firstSnapshotError = snapshots.find((snapshot) => snapshot.isError)?.error as { code?: string } | undefined;

  let sourceContent: ReactNode = <Spin />;
  if (!sourcedTab) {
    sourceContent = <LaterSlice testId="workbench-tab-placeholder" />;
  } else if (activeQuery.error) {
    sourceContent = (
      <PartialFailure
        errorCode={(activeQuery.error as { code?: string }).code}
        confirmedAtUnixMs={pageReadAt}
      >
        {table ?? <div />}
      </PartialFailure>
    );
  } else if (activeQuery.data) {
    const empty = activeTab === 'needs-me'
      ? (inboxData?.items.length ?? 0) === 0
      : (nextData?.items.length ?? 0) === 0 && (nextData?.resume.length ?? 0) === 0;
    sourceContent = empty ? <ObservationEmpty readAtUnixMs={pageReadAt} /> : (
      <Flex vertical gap={16}>
        {activeTab === 'in-progress' && (nextData?.resume.length ?? 0) > 0 ? (
          <Card size="small" data-testid="workbench-resume" title={t('workbench.resume.title')}>
            <List
              size="small"
              dataSource={nextData?.resume ?? []}
              renderItem={(entry) => (
                <List.Item>
                  <Flex vertical>
                    {effectiveProject ? <Link to={taskPath(effectiveProject, entry.work_id)}>{entry.work_id}</Link> : entry.work_id}
                    <Typography.Text type="secondary">
                      {t('workbench.resume.session', { id: entry.session_id, version: entry.session_version })}
                    </Typography.Text>
                  </Flex>
                </List.Item>
              )}
            />
          </Card>
        ) : null}
        {allSnapshotsFailed ? (
          <PartialFailure errorCode={firstSnapshotError?.code} confirmedAtUnixMs={pageReadAt}>
            {table}
          </PartialFailure>
        ) : table}
        {(activeTab === 'needs-me' ? inboxData?.next_cursor : nextData?.next_cursor) != null
          ? <Alert type="info" showIcon message={t('workbench.moreUnread')} />
          : null}
      </Flex>
    );
  } else if (projects.error) {
    sourceContent = (
      <PartialFailure errorCode={(projects.error as { code?: string }).code}>
        <div />
      </PartialFailure>
    );
  }

  return (
    <div data-testid="page-workbench" style={{ width: '100%', minWidth: 0 }}>
      <Flex vertical gap={16}>
        <Flex justify="space-between" align="flex-start" wrap gap={12}>
          <div>
            <Typography.Title level={2} style={{ marginTop: 0, marginBottom: 4 }}>
              {t('workbench.title')}
            </Typography.Title>
            <Typography.Paragraph type="secondary" style={{ marginBottom: 0 }}>
              {t('workbench.subtitle')}
            </Typography.Paragraph>
          </div>
          <Space wrap>
            {nextData?.identity.role ? <Tag data-testid="workbench-role">{nextData.identity.role}</Tag> : null}
            <Link to="/agent-connect">
              <Space size={4}><LinkIcon size={14} aria-hidden />{t('nav.agentConnect')}</Space>
            </Link>
          </Space>
        </Flex>
        <FreshnessBadge freshness={activeQuery.data?.freshness} />
        <Tabs
          activeKey={activeTab}
          onChange={selectTab}
          items={[
            { key: 'in-progress', label: t('workbench.tabs.inProgress') },
            { key: 'needs-me', label: t('workbench.tabs.needsMe') },
            { key: 'participate', label: t('workbench.tabs.participate') },
            { key: 'delivered', label: t('workbench.tabs.delivered') },
          ]}
        />
        {sourcedTab ? (
          <Row gutter={[16, 16]}>
            <Col xs={24} lg={16} style={{ minWidth: 0 }}>
              <Flex vertical gap={16}>
                <Card style={{ minWidth: 0 }}>{sourceContent}</Card>
                <Card data-testid="workbench-agents-placeholder" title={t('workbench.agents')}>
                  <LaterSlice />
                </Card>
                <Card data-testid="workbench-handoffs-placeholder" title={t('workbench.handoffs')}>
                  <LaterSlice />
                </Card>
              </Flex>
            </Col>
            <Col xs={24} lg={8} style={{ minWidth: 0 }}>
              {effectiveProject ? (
                <WorkDetail project={effectiveProject} row={selectedRow} snapshot={selectedSnapshot} />
              ) : <WorkDetail project="" />}
            </Col>
          </Row>
        ) : <Card>{sourceContent}</Card>}
      </Flex>
    </div>
  );
}
