import { useQuery } from '@tanstack/react-query';
import {
  Card,
  Col,
  Flex,
  Pagination,
  Row,
  Segmented,
  Select,
  Space,
  Spin,
  Table,
  Tooltip,
  Typography,
  type TableProps,
} from 'antd';
import type { KeyboardEvent } from 'react';
import { useTranslation } from 'react-i18next';
import { Link, useSearchParams } from 'react-router';
import { ObservationEmpty, ObservationForbidden, PartialFailure } from '../components/states';
import { ObservationValue } from '../components/ObservationValue';
import { WorkDetailPanel } from '../components/WorkDetailPanel';
import { isAuthError } from '../lib/api';
import { overviewQueryOptions } from '../lib/queries';
import { useRevocation } from '../lib/revocation';
import type { TeamOverviewResponse, TeamWork } from '../lib/types';

const PAGE_SIZE = 60;

type DisplayRow =
  | { kind: 'group'; key: string; id: string; title: string }
  | { kind: 'work'; key: string; work: TeamWork };

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

export function WorkPlanPage({ project }: { project: string }) {
  const { t } = useTranslation();
  const { revoked } = useRevocation();
  const [searchParams, setSearchParams] = useSearchParams();
  const overview = useQuery(overviewQueryOptions(project, revoked));
  const data = overview.data;
  const stream = searchParams.get('stream');
  const filteredWorks = (data?.works ?? []).filter((work) => !stream || work.workstream_id === stream);
  const grouped = groupedWorks(filteredWorks, data?.workstreams ?? []);
  const page = Math.min(
    validPage(searchParams.get('page')),
    Math.max(1, Math.ceil(grouped.works.length / PAGE_SIZE)),
  );
  const visibleWorks = grouped.works.slice((page - 1) * PAGE_SIZE, page * PAGE_SIZE);
  const selectedWorkId = searchParams.get('work');
  const selectedWork = (data?.works ?? []).find((work) => work.key === selectedWorkId);

  if (revoked || isAuthError(overview.error)) return <ObservationForbidden />;

  function updateParams(update: (next: URLSearchParams) => void) {
    const next = new URLSearchParams(searchParams);
    update(next);
    setSearchParams(next);
  }

  function selectPage(nextPage: number) {
    updateParams((next) => {
      if (nextPage === 1) next.delete('page');
      else next.set('page', String(nextPage));
      next.delete('work');
    });
  }

  function selectWorkstream(nextStream: string) {
    updateParams((next) => {
      if (nextStream === 'all') next.delete('stream');
      else next.set('stream', nextStream);
      next.delete('page');
      next.delete('work');
    });
  }

  function selectWork(work: TeamWork) {
    updateParams((next) => next.set('work', work.key));
  }

  const displayRows: DisplayRow[] = [];
  let currentWorkstream: string | null = null;
  for (const work of visibleWorks) {
    if (work.workstream_id !== currentWorkstream) {
      currentWorkstream = work.workstream_id;
      displayRows.push({
        kind: 'group',
        key: `group-${currentWorkstream}`,
        id: currentWorkstream,
        title: grouped.title(currentWorkstream),
      });
    }
    displayRows.push({ kind: 'work', key: work.key, work });
  }

  const columns: TableProps<DisplayRow>['columns'] = [
    {
      title: t('plan.columns.task'),
      key: 'task',
      width: 280,
      onCell: (row) => ({ colSpan: row.kind === 'group' ? 3 : 1 }),
      render: (_, row) => row.kind === 'group'
        ? <Typography.Text strong data-testid={`workstream-group-${row.id}`}>{row.title}</Typography.Text>
        : <Link to={taskPath(project, row.work.key)}>{row.work.title}</Link>,
    },
    {
      title: t('plan.columns.status'),
      key: 'status',
      width: 160,
      onCell: (row) => ({ colSpan: row.kind === 'group' ? 0 : 1 }),
      render: (_, row) => row.kind === 'work' ? (
        <ObservationValue testId={`plan-status-${row.work.key}`} value={row.work.status} field="state" />
      ) : null,
    },
    {
      title: t('plan.columns.owner'),
      key: 'owner',
      width: 160,
      onCell: (row) => ({ colSpan: row.kind === 'group' ? 0 : 1 }),
      render: (_, row) => row.kind === 'work' ? (
        <span data-testid={`plan-owner-${row.work.key}`}>
          {rawText(row.work.owner_person ?? row.work.agent) ?? t('common.notProvided')}
        </span>
      ) : null,
    },
  ];

  const table = (
    <Flex vertical gap={16}>
      <Table<DisplayRow>
        data-testid="plan-progress-table"
        rowKey="key"
        columns={columns}
        dataSource={displayRows}
        tableLayout="fixed"
        scroll={{ x: 600 }}
        pagination={false}
        onRow={(row) => row.kind === 'group' ? {} : {
          tabIndex: 0,
          onClick: () => selectWork(row.work),
          onKeyDown: (event: KeyboardEvent<HTMLTableRowElement>) => {
            if (event.key === 'Enter' || event.key === ' ') {
              event.preventDefault();
              selectWork(row.work);
            }
          },
        }}
      />
      <div data-testid="plan-pagination">
        <Pagination
          current={page}
          pageSize={PAGE_SIZE}
          total={grouped.works.length}
          showSizeChanger={false}
          onChange={selectPage}
          style={{ display: 'flex', justifyContent: 'flex-end' }}
        />
      </div>
    </Flex>
  );

  let content = <Spin />;
  if (overview.error) {
    content = <PartialFailure errorCode={(overview.error as { code?: string }).code}><div /></PartialFailure>;
  } else if (data) {
    const empty = data.coverage.scope.status === 'succeeded'
      && data.coverage.scope.enumerated === 0
      && data.coverage.failed === 0;
    content = empty
      ? <ObservationEmpty readAtUnixMs={data.freshness.page_read.at_unix_ms} />
      : table;
    const failedStream = data.coverage.streams.find((part) => part.status === 'failed');
    if (data.coverage.failed > 0 || failedStream) {
      content = (
        <PartialFailure
          errorCode={failedStream?.error_code}
          confirmedAtUnixMs={failedStream?.read_at_unix_ms}
        >
          {content}
        </PartialFailure>
      );
    }
  }

  return (
    <Flex vertical gap={16} data-testid="work-plan-page" style={{ minWidth: 0 }}>
      <Flex justify="space-between" align="center" wrap gap={12}>
        <Typography.Title level={5} style={{ margin: 0 }}>{t('plan.title')}</Typography.Title>
        <Space wrap>
          <Tooltip title={t('shell.later')}>
            <Segmented
              value="list"
              options={[
                { label: t('plan.list'), value: 'list' },
                { label: t('plan.board'), value: 'board', disabled: true },
              ]}
            />
          </Tooltip>
          <Select
            aria-label={t('plan.filter')}
            value={stream ?? 'all'}
            style={{ minWidth: 180 }}
            onChange={selectWorkstream}
            options={[
              { value: 'all', label: t('plan.allWorkstreams') },
              ...(data?.workstreams ?? []).map((workstream) => ({
                value: workstream.id,
                label: rawText(workstream.title) ?? rawText(workstream.external_key) ?? workstream.id,
              })),
            ]}
          />
        </Space>
      </Flex>
      <Row gutter={[16, 16]}>
        <Col xs={24} lg={16} style={{ minWidth: 0 }}>
          <Flex vertical gap={16}>
            <Card style={{ minWidth: 0 }}>{content}</Card>
            <Card data-testid="plan-candidates-placeholder" size="small">
              <Typography.Text strong>{t('state.placeholder.title')}</Typography.Text>
              <Typography.Paragraph type="secondary" style={{ marginBottom: 0 }}>
                {t('state.placeholder.description')}
              </Typography.Paragraph>
            </Card>
          </Flex>
        </Col>
        <Col xs={24} lg={8} style={{ minWidth: 0 }}>
          <Flex vertical gap={16}>
            <WorkDetailPanel
              project={project}
              work={selectedWork}
              reviewRoundId={searchParams.get('review_round_id')}
            />
            <Card data-testid="plan-source-set-placeholder" size="small">
              <Typography.Text strong>{t('state.placeholder.title')}</Typography.Text>
              <Typography.Paragraph type="secondary" style={{ marginBottom: 0 }}>
                {t('state.placeholder.description')}
              </Typography.Paragraph>
            </Card>
          </Flex>
        </Col>
      </Row>
    </Flex>
  );
}
