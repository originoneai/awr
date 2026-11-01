import { useInfiniteQuery, useQuery } from '@tanstack/react-query';
import { Button, Card, Select, Space, Table, Tabs, Typography } from 'antd';
import { useMemo } from 'react';
import { useTranslation } from 'react-i18next';
import { useSearchParams } from 'react-router';
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
  const activity = useInfiniteQuery(activityListQueryOptions(project, kind, { limit }, revoked));
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
          <Table<TeamActivityItem>
            size="small"
            rowKey="id"
            pagination={false}
            scroll={{ x: 720 }}
            dataSource={visibleItems}
            onRow={() => ({ className: 'activity-row', 'data-testid': 'activity-row' } as { className: string; 'data-testid': string })}
            columns={[
              { title: t('activity.time'), dataIndex: 'created_at_unix_ms', width: 160, render: (value) => formatUnixMs(value, i18n.language) },
              { title: t('activity.actor'), dataIndex: 'actor_id', width: 230, render: (value) => <Typography.Text style={{ overflowWrap: 'anywhere' }}>{value}</Typography.Text> },
              { title: t('activity.action'), dataIndex: 'action', width: 190 },
              { title: t('activity.work'), dataIndex: 'work_id', width: 170, render: (value) => value ?? t('freshness.state.not_measured') },
              { title: t('activity.result'), dataIndex: 'result', width: 120, render: (value) => raw(value) ?? t('freshness.state.not_measured') },
            ]}
          />
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
