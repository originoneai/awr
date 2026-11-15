import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { Alert, App, Button, Card, Descriptions, Empty, Grid, List, Popconfirm, Space, Table, Tag, Typography } from 'antd';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { readsAsName } from '../core/observation/owner';
import { ApiError, apiPost } from '../lib/api';
import { formatUnixMs } from '../lib/format';
import { projectsQueryOptions } from '../lib/queries';
import { useRevocation } from '../lib/revocation';
import { useScope } from '../lib/scope';

interface Connection {
  connection_id: string;
  kind: 'browser' | 'agent' | 'shared_access';
  client_name?: string;
  state: 'active' | 'revoked';
  created_at_ms?: number | null;
  expires_at_ms?: number | null;
  last_seen_ms?: number | null;
  revocation_scope: 'connection' | 'connection_family' | 'shared_project_access' | 'shared_account_access';
  is_current?: boolean;
  can_revoke: boolean;
}

interface ConnectionsResponse {
  data: { items: Connection[]; next_cursor: string | null };
}

/** Account operations have a separate entry from project work observation. */
export function AccountConnections() {
  const { revoked } = useRevocation();
  const { projectKey } = useScope();
  const projects = useQuery(projectsQueryOptions(revoked));
  const project = projectKey ?? projects.data?.projects?.[0]?.key ?? null;
  return <ProjectConnections key={project ?? ''} project={project} projectsLoading={projects.isPending} />;
}

function ProjectConnections({ project, projectsLoading }: { project: string | null; projectsLoading: boolean }) {
  const { t, i18n } = useTranslation();
  const { message } = App.useApp();
  const { revoked, revoke } = useRevocation();
  const screens = Grid.useBreakpoint();
  const client = useQueryClient();
  const [pages, setPages] = useState<string[]>([]);
  const cursor = pages.at(-1);
  const connections = useQuery({
    queryKey: ['team', 'connections', project, cursor ?? null],
    queryFn: async ({ signal }) => {
      const response = await apiPost<ConnectionsResponse>('/api/team/connections/list', { project, limit: 20, ...(cursor ? { cursor } : {}) }, signal);
      if (!Array.isArray(response.data?.items) || response.data.items.length > 20
        || !(response.data.next_cursor === null || typeof response.data.next_cursor === 'string')) {
        throw new ApiError('InvalidResponse', 'Connection list unavailable');
      }
      return response;
    },
    enabled: Boolean(project) && !revoked,
    gcTime: 0,
    refetchOnMount: 'always',
    refetchOnWindowFocus: true,
    retry: false,
  });
  const disconnect = useMutation({
    mutationFn: async (connection: Connection) => {
      const result = await apiPost<{ data: { connection_id: string; state: string; ends_current?: boolean } }>(
        '/api/team/connections/revoke', { project, connection_id: connection.connection_id });
      if (result.data?.state !== 'revoked' || result.data.connection_id !== connection.connection_id) {
        throw new ApiError('InvalidResponse', 'Disconnection is unconfirmed');
      }
      return result;
    },
    onSuccess: async (result) => {
      message.success(t('connections.ended'));
      if (result.data.ends_current) revoke();
      await client.invalidateQueries({ queryKey: ['team', 'connections'] });
    },
    onError: async () => {
      message.warning(t('connections.unknown'));
      await client.invalidateQueries({ queryKey: ['team', 'connections'] });
    },
  });
  if (revoked) return null;

  function name(connection: Connection) {
    if (connection.kind === 'browser') return t(connection.is_current ? 'connections.currentBrowser' : 'connections.browser');
    if (connection.kind === 'shared_access') return t('connections.sharedAccess');
    return connection.client_name && readsAsName(connection.client_name) ? connection.client_name : t('connections.agent');
  }

  function disconnectButton(connection: Connection) {
    if (!connection.can_revoke || connection.state !== 'active') return null;
    return (
      <Popconfirm title={t('connections.confirm')} description={t(connection.kind === 'shared_access' ? 'connections.confirmShared' : 'connections.confirmConnection')}
        okText={t('connections.disconnect')} cancelText={t('connections.cancel')} onConfirm={() => disconnect.mutateAsync(connection).catch(() => undefined)}>
        <Button danger size="small" loading={disconnect.isPending && disconnect.variables?.connection_id === connection.connection_id} disabled={disconnect.isPending}>{t('connections.disconnect')}</Button>
      </Popconfirm>
    );
  }

  function status(connection: Connection) {
    return t(connection.state === 'active' ? 'connections.active' : connection.state === 'revoked' ? 'connections.revoked' : 'connections.unknownState');
  }

  const items = connections.isError ? [] : connections.data?.data.items ?? [];
  return (
    <Card title={t('connections.title')} data-testid="account-connections" extra={
      <Button onClick={() => void connections.refetch()} loading={connections.isFetching} disabled={!project}>
        {t('connections.refresh')}
      </Button>
    }>
      <Space orientation="vertical" size={16} style={{ width: '100%' }}>
        <Typography.Paragraph type="secondary" style={{ margin: 0 }}>{t('connections.description')}</Typography.Paragraph>
        {connections.isError ? <Alert type="warning" showIcon title={t('connections.unavailable')} /> : screens.md === false ? (
          <List<Connection> dataSource={items} loading={projectsLoading || (connections.isPending && Boolean(project))}
            locale={{ emptyText: t('connections.empty') }} renderItem={(row) => (
              <List.Item key={row.connection_id}>
                <Space orientation="vertical" size={12} style={{ width: '100%' }}>
                  <Space style={{ width: '100%', justifyContent: 'space-between' }}>
                    <Typography.Text>{name(row)}</Typography.Text>
                    {disconnectButton(row)}
                  </Space>
                  <Tag>{status(row)}</Tag>
                  {row.kind === 'shared_access' && <Typography.Text type="secondary">{t(row.revocation_scope === 'shared_account_access' ? 'connections.sharedAccount' : 'connections.sharedProject')}</Typography.Text>}
                  <Descriptions size="small" column={1} items={[
                    { key: 'created', label: t('account.sessionCreated'), children: row.created_at_ms != null ? formatUnixMs(row.created_at_ms, i18n.language) : t('common.notProvided') },
                    { key: 'expires', label: t('connections.expires'), children: row.expires_at_ms != null ? formatUnixMs(row.expires_at_ms, i18n.language) : t('common.notProvided') },
                  ]} />
                </Space>
              </List.Item>
            )} />
        ) : (
          <Table<Connection> rowKey="connection_id" dataSource={items} loading={projectsLoading || (connections.isPending && Boolean(project))}
            pagination={false} scroll={{ x: 640 }} locale={{ emptyText: <Empty description={t('connections.empty')} image={Empty.PRESENTED_IMAGE_SIMPLE} /> }}
            columns={[
              { title: t('connections.name'), key: 'name', render: (_, row) => <Space orientation="vertical" size={4}>
                <Typography.Text>{name(row)}</Typography.Text>
                {row.kind === 'shared_access' && <Typography.Text type="secondary">{t(row.revocation_scope === 'shared_account_access' ? 'connections.sharedAccount' : 'connections.sharedProject')}</Typography.Text>}
              </Space> },
              { title: t('connections.status'), key: 'status', render: (_, row) => <Tag>{status(row)}</Tag> },
              { title: t('account.sessionCreated'), key: 'created', render: (_, row) => row.created_at_ms != null ? formatUnixMs(row.created_at_ms, i18n.language) : t('common.notProvided') },
              { title: t('connections.expires'), key: 'expires', render: (_, row) => row.expires_at_ms != null ? formatUnixMs(row.expires_at_ms, i18n.language) : t('common.notProvided') },
              { title: t('connections.action'), key: 'action', render: (_, row) => disconnectButton(row) },
            ]}
          />
        )}
        {(pages.length > 0 || connections.data?.data.next_cursor) && <Space>
          <Button disabled={pages.length === 0 || connections.isFetching} onClick={() => setPages((previous) => previous.slice(0, -1))}>{t('connections.previous')}</Button>
          <Button disabled={!connections.data?.data.next_cursor || connections.isFetching} onClick={() => {
            const next = connections.data?.data.next_cursor;
            if (next) setPages((previous) => [...previous, next]);
          }}>{t('connections.next')}</Button>
        </Space>}
      </Space>
    </Card>
  );
}
