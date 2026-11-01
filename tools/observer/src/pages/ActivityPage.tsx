import { useInfiniteQuery, useQuery } from '@tanstack/react-query';
import { Alert, Button, Card, Col, Descriptions, Row, Select, Space, Table, Tabs, Typography } from 'antd';
import { useMemo } from 'react';
import { useTranslation } from 'react-i18next';
import { Link, useSearchParams } from 'react-router';
import { ObservationEmpty, ObservationForbidden, PartialFailure } from '../components/states';
import { activityListQueryOptions, capabilitiesQueryOptions, type ActivityKind } from '../lib/activity';
import { isAuthError } from '../lib/api';
import { formatUnixMs } from '../lib/format';
import { projectsQueryOptions } from '../lib/queries';
import { useRevocation } from '../lib/revocation';
import type { TeamActivityItem } from '../lib/types';

type ActivityTab = ActivityKind | 'access';

function raw(value: unknown): string | null {
  if (value == null) return null;
  if (typeof value === 'string') return value;
  if (typeof value === 'number' || typeof value === 'boolean') return String(value);
  return JSON.stringify(value) ?? null;
}

function tabOf(value: string | null): ActivityTab {
  return value === 'requests' || value === 'access' ? value : 'development';
}

function limitOf(value: string | null) {
  if (!value || !/^[1-9]\d*$/.test(value)) return 50;
  const parsed = Number(value);
  return Number.isSafeInteger(parsed) ? Math.min(parsed, 100) : 50;
}

/** Bounded object activity, not the deferred full ops-audit/export surface. */
export function ActivityPage() {
  const { t, i18n } = useTranslation();
  const { revoked } = useRevocation();
  const [searchParams, setSearchParams] = useSearchParams();
  const projects = useQuery(projectsQueryOptions(revoked));
  const project = searchParams.get('project') ?? projects.data?.projects[0]?.key ?? '';
  const tab = tabOf(searchParams.get('kind'));
  const kind: ActivityKind = tab === 'requests' ? 'requests' : 'development';
  const limit = limitOf(searchParams.get('limit'));
  const capabilities = useQuery(capabilitiesQueryOptions(project, revoked));
  const identity = capabilities.data?.result.data.identity;
  const workFilter = searchParams.get('work') ?? undefined;
  const actorParam = searchParams.get('actor') ?? undefined;
  const projectAudit = identity?.can_read_project_audit === true;
  const actorFilter = actorParam && (projectAudit || actorParam === identity?.actor_id) ? actorParam : undefined;
  const activity = useInfiniteQuery(activityListQueryOptions(project, kind, {
    workId: workFilter, actorId: actorFilter, limit,
  }, revoked));
  const items = useMemo(() => {
    const unique = new Map<string, TeamActivityItem>();
    for (const page of activity.data?.pages ?? []) {
      for (const item of page.data.items) {
        if (!unique.has(item.id)) unique.set(item.id, item);
      }
    }
    return Array.from(unique.values());
  }, [activity.data]);
  const visibleItems = tab === 'access' ? items.filter((item) => item.action === 'access.changed') : items;
  const firstPage = activity.data?.pages[0]?.data;
  const workOptions = useMemo(() => Array.from(new Set(
    items.map((item) => item.work_id).filter((id): id is string => Boolean(id)),
  )).map((value) => ({ value, label: value })), [items]);
  const actorOptions = useMemo(() => {
    const values = projectAudit
      ? Array.from(new Set(items.map((item) => item.actor_id)))
      : identity?.actor_id ? [identity.actor_id] : [];
    return values.map((value) => ({ value, label: value }));
  }, [items, projectAudit, identity?.actor_id]);

  function updateFilter(key: 'work' | 'actor', value?: string) {
    const next = new URLSearchParams(searchParams);
    if (value) next.set(key, value);
    else next.delete(key);
    next.delete('event');
    setSearchParams(next);
  }

  const selectedEvent = visibleItems.find((item) => item.id === searchParams.get('event'));
  function selectEvent(id: string) {
    const next = new URLSearchParams(searchParams);
    next.set('event', id);
    setSearchParams(next);
  }

  if (revoked || isAuthError(projects.error) || isAuthError(activity.error) || isAuthError(capabilities.error)) {
    return <ObservationForbidden />;
  }
  if (projects.isPending || (project && activity.isPending)) return <Card>{t('common.loading')}</Card>;
  if (projects.isError) return <PartialFailure errorCode="ActivityProjectsError">{null}</PartialFailure>;
  if (activity.isError) return <PartialFailure errorCode="ActivityReadError">{null}</PartialFailure>;

  return (
    <Space direction="vertical" size={16} style={{ width: '100%' }} data-testid="page-activity">
      <Card size="small">
        <Space size={16} wrap>
          <Typography.Title level={4} style={{ margin: 0 }}>{t('activity.title')}</Typography.Title>
          <Typography.Text type="secondary" data-testid="activity-scope">scope: {raw(firstPage?.scope) ?? t('freshness.state.not_measured')}</Typography.Text>
          <Select
            aria-label={t('activity.limit')}
            data-testid="activity-limit"
            value={limit}
            style={{ width: 96 }}
            onChange={(value) => {
              const next = new URLSearchParams(searchParams);
              next.set('limit', String(value));
              next.delete('event');
              setSearchParams(next);
            }}
            options={[50, 100].map((value) => ({ value, label: String(value) }))}
          />
          <Select
            aria-label={t('activity.workFilter')}
            data-testid="activity-work-filter"
            value={workFilter}
            allowClear
            placeholder={t('activity.workFilter')}
            style={{ minWidth: 190, maxWidth: 320 }}
            onChange={(value) => updateFilter('work', value)}
            options={workOptions}
          />
          <Select
            aria-label={t('activity.actorFilter')}
            data-testid="activity-actor-filter"
            value={actorFilter}
            allowClear
            placeholder={t('activity.actorFilter')}
            style={{ minWidth: 220, maxWidth: 360 }}
            onChange={(value) => updateFilter('actor', value)}
            options={actorOptions}
          />
        </Space>
      </Card>
      <Card size="small">
        <Tabs
          activeKey={tab}
          onChange={(key) => {
            const next = new URLSearchParams(searchParams);
            next.set('kind', key);
            next.delete('event');
            setSearchParams(next);
          }}
          items={[
            { key: 'development', label: t('activity.development') },
            { key: 'requests', label: t('activity.requests') },
            { key: 'access', label: t('activity.access') },
          ]}
        />
        {tab === 'access' ? (
          <Typography.Text type="secondary" data-testid="activity-access-note">{t('activity.accessNote')}</Typography.Text>
        ) : null}
        {visibleItems.length === 0 ? <ObservationEmpty /> : (
          <Row gutter={[16, 16]}>
            <Col xs={24} xl={16}>
              <Table<TeamActivityItem>
                size="small"
                rowKey="id"
                pagination={false}
                scroll={{ x: 720 }}
                dataSource={visibleItems}
                onRow={(record) => ({
                  className: 'activity-row', 'data-testid': 'activity-row', onClick: () => selectEvent(record.id),
                } as { className: string; 'data-testid': string; onClick: () => void })}
                columns={[
                  { title: t('activity.time'), dataIndex: 'created_at_unix_ms', width: 160, render: (value) => formatUnixMs(value, i18n.language) },
                  {
                    title: t('activity.actor'), dataIndex: 'actor_id', width: 230,
                    render: (value) => projectAudit || value === identity?.actor_id ? (
                      <Button type="link" style={{ padding: 0 }} data-testid={`activity-actor-${value}`} onClick={() => updateFilter('actor', value)}>
                        <Typography.Text style={{ overflowWrap: 'anywhere' }}>{value}</Typography.Text>
                      </Button>
                    ) : <Typography.Text style={{ overflowWrap: 'anywhere' }}>{value}</Typography.Text>,
                  },
                  { title: t('activity.action'), dataIndex: 'action', width: 190 },
                  {
                    title: t('activity.work'), dataIndex: 'work_id', width: 170,
                    render: (value, record) => value ? (
                      <Link data-testid={`activity-work-link-${value}`} to={`/projects/${encodeURIComponent(project)}?tab=acceptance&work=${encodeURIComponent(value)}`}>
                        {value}
                      </Link>
                    ) : <span data-testid={`activity-work-empty-${record.id}`}>{t('freshness.state.not_measured')}</span>,
                  },
                  { title: t('activity.result'), dataIndex: 'result', width: 120, render: (value) => raw(value) ?? t('freshness.state.not_measured') },
                ]}
              />
            </Col>
            <Col xs={24} xl={8}>
              <Card size="small" title={t('activity.detail')}>
                {selectedEvent ? (
                  <Space direction="vertical" size={12} style={{ width: '100%' }} data-testid="activity-event-detail">
                    <Descriptions
                      size="small"
                      column={1}
                      items={([
                        ['id', 'activity-event-id', selectedEvent.id],
                        ['actor_id', 'activity-event-actor', selectedEvent.actor_id],
                        ['client_id', 'activity-event-client', selectedEvent.client_id],
                        ['credential_id', 'activity-event-credential', selectedEvent.credential_id],
                        ['action', 'activity-event-action', selectedEvent.action],
                        ['work_id', 'activity-event-work', selectedEvent.work_id],
                        ['result', 'activity-event-result', selectedEvent.result],
                        ['created_at_unix_ms', 'activity-event-time', formatUnixMs(selectedEvent.created_at_unix_ms, i18n.language)],
                      ] as const).map(([label, testId, field]) => ({
                        key: label,
                        label,
                        children: <span data-testid={testId}>{raw(field) ?? t('freshness.state.not_measured')}</span>,
                      }))}
                    />
                    <Typography.Text type="secondary" data-testid="activity-event-not-request">
                      {t('activity.auditRowId')}
                    </Typography.Text>
                    {selectedEvent.result === 'unknown' ? (
                      <Alert
                        type="warning"
                        showIcon
                        message={t('activity.unknownResult')}
                        description={<span data-testid="activity-unknown-result">{raw(firstPage?.unknown_means) ?? t('freshness.state.not_measured')}</span>}
                      />
                    ) : null}
                  </Space>
                ) : <Typography.Text type="secondary">{t('activity.selectEvent')}</Typography.Text>}
              </Card>
            </Col>
          </Row>
        )}
        {activity.hasNextPage ? (
          <Button
            data-testid="activity-load-earlier"
            loading={activity.isFetchingNextPage}
            onClick={() => activity.fetchNextPage()}
          >
            {t('activity.loadEarlier')}
          </Button>
        ) : null}
      </Card>
      <Card size="small">
        <Space direction="vertical" size={4}>
          <Typography.Text type="secondary" data-testid="activity-retention">
            request_retention: {raw(firstPage?.request_retention) ?? t('freshness.state.not_measured')}
            {firstPage?.request_retention_capacity != null ? ` (${firstPage.request_retention_capacity})` : ''}
          </Typography.Text>
          <Typography.Text type="secondary" data-testid="activity-development-source">
            development_source: {raw(firstPage?.development_source) ?? t('freshness.state.not_measured')}
          </Typography.Text>
          <Typography.Text type="secondary" data-testid="activity-collection-limits">
            chat_text_collected: {raw(firstPage?.chat_text_collected) ?? t('freshness.state.not_measured')}
            {' · '}tool_io_collected: {raw(firstPage?.tool_io_collected) ?? t('freshness.state.not_measured')}
          </Typography.Text>
          <Typography.Text type="secondary" data-testid="activity-unknown-means">
            unknown_means: {raw(firstPage?.unknown_means) ?? t('freshness.state.not_measured')}
          </Typography.Text>
        </Space>
      </Card>
    </Space>
  );
}
