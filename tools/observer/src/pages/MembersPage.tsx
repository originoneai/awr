import { useQuery, useQueryClient } from '@tanstack/react-query';
import { Button, Input, Popover, Select } from 'antd';
import type { TFunction } from 'i18next';
import { useEffect, useMemo, useState, type ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import { Link, useNavigate, useSearchParams } from 'react-router';

import { FreshnessBadge } from '../components/FreshnessBadge';
import { DataTable, type DataTableRow } from '../components/display/DataTable';
import { DetailPanel } from '../components/display/DetailPanel';
import { Notice } from '../components/display/Notice';
import { PageHeader } from '../components/display/PageHeader';
import { Section } from '../components/display/Section';
import { ObservationForbidden } from '../components/states';
import { readableNarrative, workLabel } from '../core/observation';
import { readsAsName } from '../core/observation/owner';
import { hasObservationValue } from '../core/observation/values';
import { deriveOverview, type DerivedTask, type OverviewDerivation, type PersonPendingItem, type PersonSummary } from '../core/overview';
import { FORBIDDEN_VISIBLE_PATTERNS, humanText } from '../i18n/human-language';
import { ApiError, apiPost, isAuthError } from '../lib/api';
import { projectLabel } from '../lib/project-label';
import { overviewModelQueryOptions, projectsQueryOptions, queryKeys } from '../lib/queries';
import { useRevocation } from '../lib/revocation';
import { useScope } from '../lib/scope';
import { taskPath } from '../lib/task-path';
import type { TeamOverviewModelResponse, TeamProject } from '../lib/types';
import { useCompactViewport } from '../lib/viewport';
import './MembersPage.css';

export const PERSON_ROUTE_BASE = 'people';

export function personPath(personId: string) {
  return `/${PERSON_ROUTE_BASE}/${encodeURIComponent(personId)}`;
}

interface Department {
  key: string;
  name: string;
}

interface DirectoryMember {
  personId: string;
  name: string | null;
  available: boolean;
  departments: Department[];
}

interface Directory {
  configured: boolean;
  organization: string | null;
  departments: Department[];
  updated_at_ms: number | null;
  members: DirectoryMember[];
}

interface Connection {
  kind: 'browser' | 'agent' | 'shared_access';
  client_name?: string;
  state: 'active' | 'revoked';
}

interface ConnectionsResponse {
  data: { items: Connection[]; next_cursor: string | null };
}

interface MemberRow {
  key: string;
  rowId: string;
  index: number;
  rawName: string | null;
  name: string;
  available: boolean;
  departments: Department[];
  personId: string | null;
  summary: PersonSummary | null;
}

type DetailKey = 'members' | 'groups' | 'pending';
type SortDirection = 'ascending' | 'descending' | null;

const CONNECTIONS_QUERY_KEY = (project: string) => ['team', 'connections', 'members', project] as const;
const DETAIL_KEYS: readonly string[] = ['members', 'groups', 'pending'];

function detailKey(value: string | null): DetailKey | null {
  return value && DETAIL_KEYS.includes(value) ? value as DetailKey : null;
}

function agentName(value: string | null | undefined, fallback: string) {
  const name = value?.split('（')[0].trim() ?? '';
  if (!name || !readsAsName(name) || FORBIDDEN_VISIBLE_PATTERNS.some(({ pattern }) => pattern.test(name)) || /\bMCP\b/i.test(name)) {
    return fallback;
  }
  return name;
}

function taskTitle(task: DerivedTask, language: string, fallback: string) {
  if (!task.title) return fallback;
  return workLabel(task.title, { language, recordedTitle: true, fallback }).text;
}

function pendingKind(item: PersonPendingItem, t: TFunction) {
  if (item.kind === 'approval') return t('members.pendingKinds.approval');
  if (item.kind === 'handoff') return t('members.pendingKinds.handoff');
  switch (item.wait?.kind) {
    case 'decision': return t('members.pendingKinds.decision');
    case 'review': return t('members.pendingKinds.review');
    case 'dependency': return t('members.pendingKinds.dependency');
    case 'external': return t('members.pendingKinds.external');
    default: return t('members.pendingKinds.other');
  }
}

function AgentLamp({ state }: { state: 'active' | 'inactive' | 'unknown' }) {
  return <span className="obs-lamp" data-lamp={state === 'active' ? 'running' : state === 'inactive' ? 'ready' : 'waiting'} data-ring={state === 'active' ? undefined : 'true'} aria-hidden />;
}

/** The directory owns row membership; the composed read owns only the facts explicitly tied to a person. */
function ProjectMembers({ project, projects }: { project: string; projects: { data?: { projects: TeamProject[] } } }) {
  const { t, i18n } = useTranslation();
  const language = i18n.language;
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const { revoked } = useRevocation();
  const compactViewport = useCompactViewport();
  const [searchParams, setSearchParams] = useSearchParams();
  const [search, setSearch] = useState('');
  const [department, setDepartment] = useState<string | null>(null);
  const [sortDirection, setSortDirection] = useState<SortDirection>(null);

  const directoryQuery = useQuery({
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
  const denied = isAuthError(directoryQuery.error);
  const overview = useQuery({
    ...overviewModelQueryOptions(project, revoked),
    enabled: !revoked && !denied,
    meta: { failureScope: 'object' },
  });
  const connections = useQuery({
    queryKey: CONNECTIONS_QUERY_KEY(project),
    queryFn: async ({ signal }) => {
      const response = await apiPost<ConnectionsResponse>('/api/team/connections/list', { project, limit: 50 }, signal);
      if (!Array.isArray(response.data?.items) || response.data.items.length > 50
        || !(response.data.next_cursor === null || typeof response.data.next_cursor === 'string')) {
        throw new ApiError('InvalidResponse', 'Connection list unavailable');
      }
      return response;
    },
    enabled: !revoked && !denied && !isAuthError(overview.error),
    meta: { failureScope: 'object' },
    gcTime: 0,
    refetchOnMount: 'always',
    refetchOnWindowFocus: true,
    refetchInterval: false,
    retry: false,
  });
  const accessDenied = revoked || denied || isAuthError(overview.error) || isAuthError(connections.error);

  useEffect(() => {
    if (!isAuthError(overview.error) && !isAuthError(connections.error)) return;
    queryClient.setQueryData(queryKeys.overviewModel(project), undefined);
    queryClient.setQueryData(CONNECTIONS_QUERY_KEY(project), undefined);
  }, [connections.error, overview.error, project, queryClient]);

  const directory = directoryQuery.isFetchedAfterMount && !isAuthError(directoryQuery.error)
    ? directoryQuery.data?.data : undefined;
  const overviewData: TeamOverviewModelResponse | undefined = !isAuthError(overview.error)
    && overview.data?.project === project && overview.data.input ? overview.data : undefined;
  const derived: OverviewDerivation | null = useMemo(() => {
    if (!overviewData) return null;
    try {
      return deriveOverview(overviewData.input);
    } catch {
      return null;
    }
  }, [overviewData]);

  const unavailableName = humanText('nameUnavailable', language);
  const label = (value: string, fallback: string) => readableNarrative(value, language)
    ?? (hasObservationValue(value) ? unavailableName : fallback);
  const departments = [...new Map([
    ...(directory?.departments ?? []),
    ...(directory?.members.flatMap((member) => member.departments) ?? []),
  ].map((item) => [item.key, item])).values()].map((item, index) => ({
    key: item.key,
    name: label(item.name, t('members.departmentUnnamed', { number: index + 1 })),
  }));
  const departmentLabels = new Map(departments.map((item, index) => [item.key,
    departments.filter((other) => other.name === item.name).length > 1
      ? t('members.sameNameDepartment', { name: item.name, number: index + 1 }) : item.name,
  ]));
  const selectedDepartment = department && departmentLabels.has(department) ? department : null;
  const projectIndex = projects.data?.projects.findIndex((item: TeamProject) => item.key === project) ?? -1;
  const currentProject = projectIndex >= 0 ? projects.data?.projects[projectIndex] : undefined;
  const currentProjectName = currentProject ? projectLabel(currentProject, projectIndex + 1) : t('members.currentProject');
  const organization = directory?.organization
    ? label(directory.organization, t('members.organizationUnnamed')) : t('members.noOrganization');
  const viewerPersonId = overviewData?.input.options.views?.me?.personId ?? null;

  const rows = useMemo<MemberRow[]>(() => {
    const overviewPersonIds = new Set((overviewData?.input.members.items ?? [])
      .flatMap((member) => member.personId ? [member.personId] : []));
    const summaries = new Map((derived?.people ?? []).map((summary) => [summary.person.id, summary]));
    return (directory?.members ?? []).map((member, index) => {
      const rawName = typeof member.name === 'string' ? member.name : null;
      const name = rawName && readsAsName(rawName) ? rawName
        : hasObservationValue(rawName) ? unavailableName : t('members.unnamed', { number: index + 1 });
      // Directory identity joins only to the same project-scoped ID in the overview membership read.
      const personId = overviewPersonIds.has(member.personId) ? member.personId : null;
      return {
        key: personId ?? `directory-${index}`,
        rowId: personId ?? `unknown-${index}`,
        index,
        rawName,
        name,
        available: member.available,
        departments: member.departments,
        personId,
        summary: personId ? summaries.get(personId) ?? null : null,
      };
    });
  }, [derived, directory, overviewData, t, unavailableName]);

  const filtered = rows.filter((member) => (!selectedDepartment
    || member.departments.some((item) => item.key === selectedDepartment))
    && [member.name, ...member.departments.map((item) => departmentLabels.get(item.key) ?? '')]
      .some((value) => value.toLocaleLowerCase().includes(search.trim().toLocaleLowerCase())));
  const visibleRows = [...filtered].sort((left, right) => sortDirection === null ? left.index - right.index
    : left.name.localeCompare(right.name, language) * (sortDirection === 'ascending' ? 1 : -1));

  function openDetail(key: DetailKey, personId?: string) {
    const next = new URLSearchParams(searchParams);
    next.set('detail', key);
    if (personId) next.set('member', personId);
    else next.delete('member');
    setSearchParams(next);
  }

  function closeDetail() {
    const next = new URLSearchParams(searchParams);
    next.delete('detail');
    next.delete('member');
    setSearchParams(next);
  }

  function cycleSort() {
    setSortDirection((current) => current === null ? 'ascending' : current === 'ascending' ? 'descending' : null);
  }

  function agentsFor(row: MemberRow) {
    const tasks = row.personId ? derived?.tasks.filter((task) => task.owner?.id === row.personId) ?? [] : [];
    const genericName = t('connections.agent');
    const result: Array<{ name: string; state: 'active' | 'inactive' | 'unknown' }> = [];
    const seen = new Set<string>();
    const add = (candidate: string | null | undefined) => {
      const name = agentName(candidate, genericName);
      if (seen.has(name)) return;
      seen.add(name);
      const facts = tasks.filter((task) => agentName(task.agentName, genericName) === name).map((task) => task.agentActive);
      const state = facts.some((value) => value === true) ? 'active'
        : facts.length > 0 && facts.every((value) => value === false) ? 'inactive' : 'unknown';
      result.push({ name, state });
    };
    if (row.rawName && rows.filter((candidate) => candidate.rawName === row.rawName).length === 1) {
      for (const connection of connections.data?.data.items ?? []) {
        if (connection.kind !== 'agent' || connection.state !== 'active' || !connection.client_name) continue;
        const parts = connection.client_name.split(/\s+·\s+/);
        if (parts.length === 2 && parts[0] === row.rawName) add(parts[1]);
      }
    }
    for (const task of tasks) if (task.agentName) add(task.agentName);
    return result;
  }

  function doingCell(row: MemberRow) {
    const doing = row.summary?.inProgress.items[0];
    if (doing) {
      const title = taskTitle(doing, language, t('members.unnamedTask'));
      return doing.id ? <Link className="members-ellipsis members-doing" data-testid={`member-doing-${row.rowId}`} title={title} to={taskPath(project, doing.id)}>{title}</Link>
        : <span className="members-ellipsis" data-testid={`member-doing-${row.rowId}`} title={title}>{title}</span>;
    }
    const knownEmpty = row.summary?.inProgress.count === 0;
    return <span className="members-dim" data-testid={`member-doing-${row.rowId}`}>{t(knownEmpty ? 'members.noDoing' : 'members.doingUnknown')}</span>;
  }

  function pendingCell(row: MemberRow) {
    const count = row.summary?.pending.count;
    if (count == null) {
      return <span className="members-dim members-quiet-count" data-testid={`member-pending-${row.rowId}`} aria-label={t('members.pendingUnknown')}>—</span>;
    }
    if (count === 0) {
      return <span className="members-dim members-quiet-count" data-testid={`member-pending-${row.rowId}`}>{t('members.noPending')}</span>;
    }
    return <button type="button" className="members-pending-button" data-testid={`member-pending-${row.rowId}`} onClick={() => row.personId && openDetail('pending', row.personId)}>{t('members.pendingCount', { count })}</button>;
  }

  const tableRows: DataTableRow[] = visibleRows.map((row) => {
    const agents = agentsFor(row);
    const pending = row.summary?.pending.count;
    return {
      key: row.key,
      testId: `member-row-${row.rowId}`,
      highlighted: pending != null && pending >= 3,
      onOpen: row.personId ? () => navigate(personPath(row.personId!)) : undefined,
      cells: [
        <div className="members-who" data-testid={`member-who-${row.rowId}`} data-viewer={row.personId && row.personId === viewerPersonId ? 'true' : undefined}>
          <span className="members-avatar" aria-hidden>{Array.from(row.name)[0] ?? ''}</span>
          <span className="members-name members-clamp" title={row.name}>{row.name}</span>
        </div>,
        <div className="members-group-cell">
          <div className="members-clamp" title={row.departments.map((item) => departmentLabels.get(item.key)).join(' · ')}>
            {row.departments.map((item) => departmentLabels.get(item.key)).join(' · ') || t('members.noDepartment')}
          </div>
          <div className="members-small">{t(row.available ? 'members.roleMissing' : 'members.unavailableMember')}</div>
        </div>,
        <Link className="members-small members-ellipsis members-project" data-testid={`member-projects-${row.rowId}`} to={`/projects/${encodeURIComponent(project)}`} title={currentProjectName}>{currentProjectName}</Link>,
        doingCell(row),
        <div className="members-chips" data-testid={`member-agents-${row.rowId}`}>
          {agents.length ? agents.map((agent, index) => {
            const stateText = t(`members.agentStates.${agent.state}`);
            return <span className="members-chip" data-testid={`member-agent-${row.rowId}-${index}`} data-agent-state={agent.state} title={`${agent.name} · ${stateText}`} aria-label={`${agent.name} · ${stateText}`} key={`${agent.name}-${index}`}>
              <AgentLamp state={agent.state} />
              <span>{agent.name}</span>
            </span>;
          }) : <span className="members-dim">{connections.data && row.summary ? t('members.noAgents') : '—'}</span>}
        </div>,
        pendingCell(row),
      ],
    };
  });

  const coverageStates = Object.values(overviewData?.coverage ?? {}).map((part) => part.status);
  const hasPartialCoverage = coverageStates.includes('partial') || Boolean(connections.data?.data.next_cursor);
  const hasUnreadCoverage = coverageStates.includes('unread');
  const partial = Boolean(directory && ((directoryQuery.isError && directory) || overview.isError || connections.isError || hasPartialCoverage));
  const unknown = Boolean(directory && !overview.isPending && !overview.isError && (!derived || hasUnreadCoverage));
  const initialLoading = directoryQuery.isPending;
  const initialFailure = !initialLoading && directoryQuery.isError && !directory;
  const selectedDetailKey = detailKey(searchParams.get('detail'));
  const selectedPersonId = searchParams.get('member');
  const selectedPerson = rows.find((row) => row.personId && row.personId === selectedPersonId) ?? null;

  let detail: { title: string; items: PersonPendingItem[] | MemberRow[] | Department[]; content: ReactNode } | null = null;
  if (selectedDetailKey === 'members' && directory) {
    const content = visibleRows.length ? <div className="members-detail-list">{visibleRows.map((row) => (
      row.personId ? <Link className="members-detail-row" data-testid="members-detail-row" key={row.key} to={personPath(row.personId)}>{row.name}</Link>
        : <div className="members-detail-row" data-testid="members-detail-row" key={row.key}>{row.name}</div>
    ))}</div> : <div className="members-detail-empty" data-testid="members-detail-empty">{t('members.detailEmpty')}</div>;
    detail = { title: t('members.detailMembers'), items: visibleRows, content };
  } else if (selectedDetailKey === 'groups' && directory) {
    const content = departments.length ? <div className="members-detail-list">{departments.map((item) => (
      <button type="button" className="members-detail-row" data-testid="members-detail-row" key={item.key} onClick={() => { setDepartment(item.key); closeDetail(); }}>{departmentLabels.get(item.key)}</button>
    ))}</div> : <div className="members-detail-empty" data-testid="members-detail-empty">{t('members.detailEmpty')}</div>;
    detail = { title: t('members.detailGroups'), items: departments, content };
  } else if (selectedDetailKey === 'pending' && selectedPerson?.summary?.pending.count != null) {
    const items = selectedPerson.summary.pending.items;
    const content = items.length ? <div className="members-detail-list">{items.map((item, index) => {
      const title = item.task ? taskTitle(item.task, language, t('members.unnamedTask'))
        : item.approval?.title?.trim() || t('members.pendingItem');
      const body = <><span className="members-small">{pendingKind(item, t)}</span><span className="members-ellipsis" title={title}>{title}</span></>;
      const href = item.task?.id ? taskPath(project, item.task.id) : item.kind === 'approval' ? `/inbox?scope=${encodeURIComponent(project)}` : null;
      return href ? <Link className="members-detail-row" data-testid="members-detail-row" key={`${item.kind}-${index}`} to={href}>{body}</Link>
        : <div className="members-detail-row" data-testid="members-detail-row" key={`${item.kind}-${index}`}>{body}</div>;
    })}</div> : <div className="members-detail-empty" data-testid="members-detail-empty">{t('members.detailEmpty')}</div>;
    detail = { title: t('members.detailPending', { name: selectedPerson.name }), items, content };
  }

  const refresh = () => void Promise.all([directoryQuery.refetch(), overview.refetch(), connections.refetch()]);
  const filterFields = <>
    <Input className="members-search" aria-label={t('members.search')} placeholder={t('members.search')} value={search} onChange={(event) => setSearch(event.target.value)} allowClear disabled={!directory} />
    <Select className="members-department-filter" aria-label={t('members.departmentFilter')} placeholder={t('members.allDepartments')} value={selectedDepartment ?? undefined} onChange={(value) => setDepartment(value ?? null)} allowClear disabled={!directory}
      options={departments.map((item) => ({ value: item.key, label: departmentLabels.get(item.key) }))} />
  </>;
  const controls = <div className="members-controls">
    {compactViewport ? <Popover trigger="click" placement="bottomRight" content={<div className="members-mobile-filters">{filterFields}</div>}>
      <Button>{t('members.filterShort')}</Button>
    </Popover> : filterFields}
    <Button aria-label={t('members.refresh')} loading={directoryQuery.isFetching || overview.isFetching || connections.isFetching} onClick={refresh}>{t('members.refresh')}</Button>
  </div>;

  let body: ReactNode;
  if (accessDenied) body = <ObservationForbidden />;
  else if (initialLoading) body = <div className="members-state" data-testid="members-loading">{t('members.readLoading')}</div>;
  else if (initialFailure) body = <div className="members-state" data-testid="members-read-error"><Notice tone="warning">{t(directoryQuery.error instanceof ApiError && directoryQuery.error.code === 'Unsupported' ? 'members.notEnabled' : 'members.readFailed')}</Notice></div>;
  else if (!directory) body = null;
  else if (!directory.configured) body = <div className="members-state" data-testid="members-empty"><div>{t('members.notRegistered')}</div><div className="members-small">{t('members.registrationHelp')}</div></div>;
  else {
    body = <>
      {directoryQuery.isError ? <div className="members-notice"><Notice tone="warning">{t('members.refreshFailed')}</Notice></div> : null}
      {partial ? <div className="members-notice" data-testid="members-partial"><Notice tone="warning">{t(overview.isError || connections.isError ? 'members.partialRead' : 'members.partialCoverage')}</Notice></div> : null}
      {visibleRows.length ? <div className="members-table" data-testid="member-table"><DataTable navigationWidth="var(--members-column-navigation)" stickyNavigation={false} columns={[
        { key: 'member', title: <button type="button" className="members-sort" aria-label={t('members.sortByName')} aria-sort={sortDirection ?? 'none'} onClick={cycleSort}>{t('members.name')}</button>, width: 'var(--members-column-member)' },
        { key: 'group', title: t('members.groupRole'), width: 'var(--members-column-group)' },
        { key: 'projects', title: t('members.projects'), hideOnSmall: true },
        { key: 'doing', title: t('members.doing') },
        { key: 'agents', title: t('members.agentsColumn'), width: 'var(--members-column-agents)', hideOnSmall: true },
        { key: 'pending', title: <span className="members-number">{t('members.pending')}</span>, width: 'var(--members-column-pending)', align: 'right' },
      ]} rows={tableRows} /></div> : <div className="members-state" data-testid="members-empty">{t(rows.length ? 'members.noMatch' : 'members.empty')}</div>}
    </>;
  }

  return <>
    <PageHeader title={t('members.title')} subtitle={directory ? t('members.subtitle', { organization, count: rows.length }) : t(accessDenied ? 'members.subtitleUnavailable' : 'members.subtitleLoading')} />
    <p className="members-lead" data-testid="members-summary">
      {directory ? <>
        <button type="button" data-testid="members-summary-members" onClick={() => openDetail('members')}>{t('members.summaryMembers', { count: rows.length })}</button>
        {t('members.summarySeparator')}
        <button type="button" data-testid="members-summary-groups" onClick={() => openDetail('groups')}>{t('members.summaryGroups', { count: departments.length })}</button>
        <span>{t('members.summaryTail')}</span>
      </> : <span>{t('members.summaryUnavailable')}</span>}
    </p>
    <Section title={t('members.sectionTitle')} count={directory?.configured ? visibleRows.length : undefined} onCountClick={directory?.configured ? () => openDetail('members') : undefined} countTestId="members-section-count" extra={controls} className="members-section">
      {body}
      <div className="members-pnote">{t('members.collectionNote')}</div>
      <div className="members-observation-meta">
        <div data-testid="members-flat-note">{t('members.flatHierarchy')}</div>
        <div data-testid="members-coverage">{t(overview.isError ? 'members.coverageFailed' : hasPartialCoverage || partial ? 'members.coveragePartial' : unknown ? 'members.coverageUnknown' : overview.isPending ? 'members.coverageLoading' : 'members.coverageComplete')}</div>
        {unknown ? <div data-testid="members-unknown">{t('members.unknownRead')}</div> : null}
        {connections.isError ? <div>{t('members.connectionsUnavailable')}</div> : null}
        <FreshnessBadge freshness={overviewData?.freshness} />
        <div className="members-account-links">
          <Link to={`/account?scope=${encodeURIComponent(project)}`}>{t('members.viewConnections')}</Link>
          <Link to={`/agent-connect?scope=${encodeURIComponent(project)}`}>{t('members.connectAgent')}</Link>
        </div>
      </div>
    </Section>
    {detail ? <div data-testid="members-detail"><DetailPanel tabs={[{
      key: 'detail',
      label: <span>{detail.title}<span className="members-detail-count" data-testid="members-detail-count">{detail.items.length}</span></span>,
      content: detail.content,
    }]} extra={<Button type="text" onClick={closeDetail}>{t('members.closeDetail')}</Button>} /></div> : null}
  </>;
}

export function MembersPage() {
  const { t } = useTranslation();
  const { revoked } = useRevocation();
  const { projectKey, setProjectKey } = useScope();
  const projects = useQuery(projectsQueryOptions(revoked));
  const forbidden = revoked || isAuthError(projects.error);
  return <div className="members-page" data-testid="page-members">
    {forbidden ? <><PageHeader title={t('members.title')} subtitle={t('members.subtitleUnavailable')} /><ObservationForbidden /></>
      : projectKey ? <ProjectMembers key={projectKey} project={projectKey} projects={projects} /> : <>
        <PageHeader title={t('members.title')} subtitle={t('members.chooseProject')} />
        <Section title={t('members.chooseProject')} className="members-project-chooser" padded>
          {projects.isPending ? <div className="members-state">{t('common.loading')}</div>
            : projects.isError ? <Notice tone="warning" extra={<Button onClick={() => void projects.refetch()}>{t('members.retry')}</Button>}>{t('members.projectsFailed')}</Notice>
              : (projects.data?.projects.length ? <div className="members-project-choices">{projects.data.projects.map((project, index) => <div className="members-project-choice" key={project.key}>
                <span>{projectLabel(project, index + 1)}</span><Button onClick={() => setProjectKey(project.key)}>{t('members.open')}</Button>
              </div>)}</div> : <div className="members-state">{t('members.noProjects')}</div>)}
        </Section>
      </>}
  </div>;
}
