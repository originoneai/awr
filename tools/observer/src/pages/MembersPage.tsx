import { useQuery, useQueryClient } from '@tanstack/react-query';
import { Alert, Button, Card, Descriptions, Empty, Flex, Grid, Input, List, Select, Space, Table, Tag, Typography } from 'antd';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { Link } from 'react-router';
import { ObservationForbidden } from '../components/states';
import { readableNarrative } from '../core/observation';
import { readsAsName } from '../core/observation/owner';
import { ApiError, apiPost, isAuthError } from '../lib/api';
import { formatUnixMs } from '../lib/format';
import { projectLabel } from '../lib/project-label';
import { projectsQueryOptions } from '../lib/queries';
import { useRevocation } from '../lib/revocation';
import { useScope } from '../lib/scope';

interface Department {
  key: string;
  name: string;
}

interface Member {
  name: string | null;
  available: boolean;
  departments: Department[];
}

interface Directory {
  configured: boolean;
  organization: string | null;
  departments: Department[];
  updated_at_ms: number | null;
  members: Member[];
}

/** This view observes the recorded project directory; Agents maintain its relationships. */
function ProjectMembers({ project }: { project: string }) {
  const { t, i18n } = useTranslation();
  const { revoked } = useRevocation();
  const queryClient = useQueryClient();
  const screens = Grid.useBreakpoint();
  const [search, setSearch] = useState('');
  const [department, setDepartment] = useState<string | null>(null);
  const query = useQuery({
    queryKey: ['team', 'directory', project],
    queryFn: async ({ signal }) => {
      try {
        return await apiPost<{ data: Directory }>('/api/team/directory', { project }, signal);
      } catch (error) {
        if (isAuthError(error)) queryClient.setQueryData(['team', 'directory', project], null);
        throw error;
      }
    },
    enabled: !revoked,
    meta: { failureScope: 'object' },
    gcTime: 0,
    refetchOnMount: 'always',
    refetchOnWindowFocus: true,
    refetchInterval: false,
    retry: false,
  });
  if (revoked) return <ObservationForbidden />;
  const directory = query.isFetchedAfterMount && !isAuthError(query.error) ? query.data?.data : undefined;
  const label = (value: string, fallback: string) => readableNarrative(value, i18n.language) ?? fallback;
  const departments = [...new Map([
    ...(directory?.departments ?? []), ...(directory?.members.flatMap((member) => member.departments) ?? []),
  ].map((item) => [item.key, item])).values()].map((item, index) => ({
    key: item.key, name: label(item.name, t('members.departmentUnnamed', { number: index + 1 })),
  }));
  const departmentLabels = new Map(departments.map((item, index) => [item.key,
    departments.filter((other) => other.name === item.name).length > 1
      ? t('members.sameNameDepartment', { name: item.name, number: index + 1 }) : item.name,
  ]));
  const members = (directory?.members ?? []).map((member, index) => ({
    ...member,
    key: index,
    name: member.name && readsAsName(member.name) ? member.name : t('members.unnamed', { number: index + 1 }),
  }));
  const selectedDepartment = department && departmentLabels.has(department) ? department : null;
  const filtered = members.filter((member) => (!selectedDepartment || member.departments.some((item) => item.key === selectedDepartment))
    && [member.name, ...member.departments.map((item) => departmentLabels.get(item.key) ?? '')]
      .some((value) => value.toLocaleLowerCase().includes(search.trim().toLocaleLowerCase())));
  const state = (member: Member) => <Tag>{t(member.available ? 'members.available' : 'members.unavailableMember')}</Tag>;
  const departmentNames = (member: Member) => member.departments.map((item) => departmentLabels.get(item.key)).join(' · ') || t('members.noDepartment');

  return <Space orientation="vertical" size={16} style={{ width: '100%', overflowWrap: 'anywhere' }} data-testid="project-members">
    <Card title={t('members.directory')} extra={<Button aria-label={t('members.refresh')} loading={query.isFetching} onClick={() => void query.refetch()}>{t('members.refresh')}</Button>}>
      {isAuthError(query.error) ? <ObservationForbidden />
        : <Space orientation="vertical" size={16} style={{ width: '100%' }}>
          {query.isError && <Alert type="warning" showIcon title={t(directory ? 'members.refreshFailed' : query.error instanceof ApiError && query.error.code === 'Unsupported' ? 'members.notEnabled' : 'members.readFailed')} />}
          {query.isPending ? <Typography.Text type="secondary">{t('common.loading')}</Typography.Text>
          : !directory ? null
          : !directory.configured ? <Space orientation="vertical" size={12} style={{ width: '100%' }}>
              <Empty image={Empty.PRESENTED_IMAGE_SIMPLE} description={t('members.notRegistered')} />
              <Typography.Text type="secondary">{t('members.registrationHelp')}</Typography.Text>
            </Space>
            : <Space orientation="vertical" size={16} style={{ width: '100%' }}>
              <Descriptions size="small" column={{ xs: 1, sm: 2, lg: 3 }} items={[
                { key: 'organization', label: t('members.organization'), children: directory.organization ? label(directory.organization, t('members.organizationUnnamed')) : t('members.noOrganization') },
                { key: 'departments', label: t('members.projectDepartments'), children: directory.departments.map((item) => departmentLabels.get(item.key)).join(' · ') || t('members.noDepartment') },
                { key: 'updated', label: t('members.updated'), children: directory.updated_at_ms !== null ? formatUnixMs(directory.updated_at_ms, i18n.language) : t('common.notProvided') },
              ]} />
              <Typography.Text type="secondary">{t('members.coverage', { count: members.length })}</Typography.Text>
              <Flex gap={12} wrap>
                <Input aria-label={t('members.search')} placeholder={t('members.search')} value={search} onChange={(event) => setSearch(event.target.value)} allowClear style={{ flex: '1 1 200px', minWidth: 0 }} />
                <Select aria-label={t('members.departmentFilter')} placeholder={t('members.allDepartments')} value={selectedDepartment} onChange={(value) => setDepartment(value ?? null)}
                  allowClear options={departments.map((item) => ({ value: item.key, label: departmentLabels.get(item.key) }))} style={{ flex: '1 1 160px', minWidth: 0 }} />
              </Flex>
              {screens.md === false ? <List data-testid="member-list" dataSource={filtered} locale={{ emptyText: t(members.length ? 'members.noMatch' : 'members.empty') }} renderItem={(member) => <List.Item key={member.key}>
                <Space orientation="vertical" size={8} style={{ minWidth: 0, overflowWrap: 'anywhere' }}>
                  <Typography.Text>{member.name}</Typography.Text>
                  <Typography.Text type="secondary">{departmentNames(member)}</Typography.Text>
                  {state(member)}
                </Space>
              </List.Item>} /> : <Table data-testid="member-table" dataSource={filtered} rowKey="key" size="middle" pagination={{ pageSize: 20, showSizeChanger: false, hideOnSinglePage: true }}
                locale={{ emptyText: <Empty image={Empty.PRESENTED_IMAGE_SIMPLE} description={t(members.length ? 'members.noMatch' : 'members.empty')} /> }}
                columns={[
                  { key: 'name', title: t('members.name'), render: (_, member) => <Typography.Text style={{ overflowWrap: 'anywhere' }}>{member.name}</Typography.Text>, sorter: (left, right) => left.name.localeCompare(right.name, i18n.language) },
                  { key: 'department', title: t('members.department'), render: (_, member) => <Typography.Text style={{ overflowWrap: 'anywhere' }}>{departmentNames(member)}</Typography.Text> },
                  { key: 'state', title: t('members.status'), render: (_, member) => state(member) },
                ]} />}
              <Typography.Paragraph type="secondary" style={{ margin: 0 }}>{t('members.guidance')}</Typography.Paragraph>
            </Space>}
        </Space>}
    </Card>
    <Card title={t('members.agents')}>
      <Space orientation="vertical" size={12}>
        <Typography.Text type="secondary">{t('members.agentsHelp')}</Typography.Text>
        <Space wrap>
          <Link to={`/account?scope=${encodeURIComponent(project)}`}>{t('members.viewConnections')}</Link>
          <Link to={`/agent-connect?scope=${encodeURIComponent(project)}`}>{t('members.connectAgent')}</Link>
        </Space>
      </Space>
    </Card>
  </Space>;
}

export function MembersPage() {
  const { t } = useTranslation();
  const { revoked } = useRevocation();
  const { projectKey, setProjectKey } = useScope();
  const projects = useQuery(projectsQueryOptions(revoked));
  if (revoked || isAuthError(projects.error)) return <ObservationForbidden />;
  return <Space orientation="vertical" size={16} style={{ width: '100%' }} data-testid="page-members">
    <Typography.Title level={3} style={{ margin: 0 }}>{t('nav.members')}</Typography.Title>
    {projectKey ? <ProjectMembers key={projectKey} project={projectKey} /> : <Card title={t('members.chooseProject')}
      extra={projects.isError ? <Button onClick={() => void projects.refetch()}>{t('members.retry')}</Button> : undefined}>
      <List loading={projects.isPending} dataSource={projects.isError ? [] : projects.data?.projects ?? []}
        locale={{ emptyText: t(projects.isError ? 'members.projectsFailed' : 'members.noProjects') }} renderItem={(project, index) => <List.Item
          actions={[<Button key="open" onClick={() => setProjectKey(project.key)}>{t('members.open')}</Button>]}>
          <Typography.Text style={{ overflowWrap: 'anywhere' }}>{projectLabel(project, index + 1)}</Typography.Text>
        </List.Item>} />
    </Card>}
  </Space>;
}
