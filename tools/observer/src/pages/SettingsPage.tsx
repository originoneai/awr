import { useQuery, useQueryClient } from '@tanstack/react-query';
import { Button } from 'antd';
import type { ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import { Link } from 'react-router';

import { FreshnessBadge } from '../components/FreshnessBadge';
import {
  DataTable,
  DisplayIcon,
  EmptyState,
  HealthTag,
  Notice,
  PageHeader,
  RowList,
  Section,
  TaskStatusTag,
  type DataTableRow,
  type HealthStatus,
} from '../components/display';
import { BuildingIcon, CheckIcon } from '../components/icons/prototype-icons';
import { ObservationForbidden } from '../components/states';
import { readableNarrative } from '../core/observation';
import type { DerivedTaskStatus } from '../core/overview/types';
import { apiPost, isAuthError } from '../lib/api';
import { capabilitiesQueryOptions, overviewModelQueryOptions, projectsQueryOptions } from '../lib/queries';
import { useRevocation } from '../lib/revocation';
import { useScope } from '../lib/scope';
import './SettingsPage.css';

interface DirectoryDepartment {
  key: string;
  name: string;
  parentKey?: string | null;
}

interface DirectoryMember {
  personId?: string;
  name: string | null;
  available: boolean;
  departments: DirectoryDepartment[];
}

interface Directory {
  configured: boolean;
  organization: string | null;
  departments: DirectoryDepartment[];
  allDepartments?: DirectoryDepartment[];
  updated_at_ms: number | null;
  members: DirectoryMember[];
}

interface DutyCapabilities {
  business_role_presets?: {
    roles?: unknown;
    action_ceilings?: unknown;
  };
}

interface DutyAbilityRowDefinition {
  key: 'readWork' | 'executeAndSubmit' | 'reviewOthers' | 'assignAndPlan' | 'confirmIntegration' | 'manageAccess' | 'readAudit';
  actions: readonly string[];
}

/** Product rows and their server action identifiers from docs/dev/reference/business-duty-actions.md. */
const DUTY_ABILITY_ROWS: readonly DutyAbilityRowDefinition[] = [
  { key: 'readWork', actions: ['work.read'] },
  {
    key: 'executeAndSubmit',
    actions: ['claim.manage_own', 'execution.request_and_report_own', 'delivery.submit_and_request_review'],
  },
  { key: 'reviewOthers', actions: ['review.decide'] },
  {
    key: 'assignAndPlan',
    actions: ['work.assign', 'planning.propose', 'planning.edit_draft', 'planning.approve', 'planning.publish'],
  },
  { key: 'confirmIntegration', actions: ['delivery.finalize'] },
  { key: 'manageAccess', actions: ['access.manage_project', 'directory.manage_project'] },
  { key: 'readAudit', actions: ['audit.read_project'] },
];

type DutyPresetsStatus = 'empty' | 'missing' | 'malformed' | 'partial' | 'complete';

interface ParsedDutyPresets {
  roles: string[];
  ceilings: Map<string, ReadonlySet<string>>;
  status: DutyPresetsStatus;
}

function isPlainObject(value: unknown): value is Record<string, unknown> {
  return Boolean(value) && typeof value === 'object' && !Array.isArray(value);
}

function parseDutyPresets(presets: DutyCapabilities['business_role_presets'] | undefined): ParsedDutyPresets {
  const emptyCeilings = new Map<string, ReadonlySet<string>>();
  if (!presets || !isPlainObject(presets)) return { roles: [], ceilings: emptyCeilings, status: 'missing' };
  if (!Array.isArray(presets.roles)) return { roles: [], ceilings: emptyCeilings, status: 'missing' };
  const roles = presets.roles.filter((role): role is string => typeof role === 'string' && role.length > 0);
  if (roles.length === 0) return { roles: [], ceilings: emptyCeilings, status: 'empty' };
  if (presets.action_ceilings === undefined) return { roles, ceilings: emptyCeilings, status: 'missing' };
  if (!isPlainObject(presets.action_ceilings)) return { roles: [], ceilings: emptyCeilings, status: 'malformed' };

  const ceilings = new Map<string, ReadonlySet<string>>();
  for (const role of roles) {
    if (!Object.hasOwn(presets.action_ceilings, role)) continue;
    const raw = presets.action_ceilings[role];
    if (!Array.isArray(raw) || !raw.every((action): action is string => typeof action === 'string' && action.length > 0)) {
      return { roles: [], ceilings: emptyCeilings, status: 'malformed' };
    }
    ceilings.set(role, new Set(raw));
  }
  if (ceilings.size === 0) return { roles, ceilings, status: 'missing' };
  if (ceilings.size < roles.length) return { roles, ceilings, status: 'partial' };
  return { roles, ceilings, status: 'complete' };
}

interface DepartmentTreeNode {
  department: DirectoryDepartment;
  orderIndex: number;
  children: DepartmentTreeNode[];
}

interface BuiltDepartmentTree {
  roots: DepartmentTreeNode[];
  hasHierarchy: boolean;
  invalidHierarchy: boolean;
}

function collectDepartments(directory: Directory | undefined): DirectoryDepartment[] {
  if (!directory) return [];
  const byKey = new Map<string, DirectoryDepartment>();
  const raw = [
    ...(directory.allDepartments ?? []),
    ...(directory.departments ?? []),
    ...(directory.members.flatMap((member) => member.departments) ?? []),
  ];
  for (const department of raw) {
    if (!department || typeof department.key !== 'string' || !department.key) continue;
    const parentKey = typeof department.parentKey === 'string' && department.parentKey.length > 0
      ? department.parentKey : null;
    const existing = byKey.get(department.key);
    if (!existing) {
      byKey.set(department.key, { key: department.key, name: department.name, parentKey });
    } else if (existing.parentKey === null && parentKey !== null) {
      byKey.set(department.key, { ...existing, parentKey });
    }
  }
  return [...byKey.values()];
}

function buildDepartmentTree(departments: readonly DirectoryDepartment[]): BuiltDepartmentTree {
  const byKey = new Map(departments.map((department) => [department.key, department]));
  for (const department of departments) {
    const visited = new Set<string>([department.key]);
    let parentKey = department.parentKey ?? null;
    if (parentKey === department.key) return { roots: [], hasHierarchy: false, invalidHierarchy: true };
    while (parentKey !== null && byKey.has(parentKey)) {
      if (visited.has(parentKey)) return { roots: [], hasHierarchy: false, invalidHierarchy: true };
      visited.add(parentKey);
      parentKey = byKey.get(parentKey)?.parentKey ?? null;
    }
  }
  const nodes = new Map<string, DepartmentTreeNode>(
    departments.map((department, orderIndex) => [department.key, { department, orderIndex, children: [] }]),
  );
  const roots: DepartmentTreeNode[] = [];
  let hasHierarchy = false;
  for (const department of departments) {
    const node = nodes.get(department.key);
    if (!node) continue;
    const parentKey = department.parentKey ?? null;
    if (parentKey !== null && nodes.has(parentKey)) {
      nodes.get(parentKey)?.children.push(node);
      hasHierarchy = true;
    } else {
      roots.push(node);
    }
  }
  return { roots, hasHierarchy, invalidHierarchy: false };
}

type SettingsReadState = 'data' | 'loading' | 'empty' | 'failed' | 'partial' | 'unknown';

const TASK_STATUSES: readonly Exclude<DerivedTaskStatus, 'unknown'>[] = [
  'verified', 'claimed', 'returned', 'review', 'running', 'stalled', 'waiting', 'ready', 'planned',
];
const HEALTH_STATUSES: readonly Exclude<HealthStatus, 'info'>[] = ['ok', 'risk', 'off', 'none'];

function connectionPath(page: 'account' | 'agent-connect', project: string) {
  return project ? `/${page}?scope=${encodeURIComponent(project)}` : `/${page}`;
}

function ReadNotice({ state, onRetry }: { state: 'failed' | 'partial' | 'unknown'; onRetry: () => void }) {
  const { t } = useTranslation();
  return (
    <div data-testid={`settings-read-${state}`}>
      <Notice
        tone="warning"
        title={t(`settings.${state}Title`)}
        extra={<Button onClick={onRetry}>{t('settings.retry')}</Button>}
      >
        {t(`settings.${state}Description`)}
      </Notice>
    </div>
  );
}

function TableReadState({ children, testId }: { children: ReactNode; testId: string }) {
  return <div data-testid={testId}><EmptyState title={children as string} /></div>;
}

/** Prototype settings(): one explanatory strip, two columns and five bordered sections. */
export function SettingsPage() {
  const { t, i18n } = useTranslation();
  const { revoked } = useRevocation();
  const { projectKey } = useScope();
  const queryClient = useQueryClient();
  const projects = useQuery(projectsQueryOptions(revoked));
  const project = projectKey ?? projects.data?.projects[0]?.key ?? '';
  const capabilities = useQuery(capabilitiesQueryOptions(project, revoked));
  const model = useQuery(overviewModelQueryOptions(project || null, revoked));
  const directoryQuery = useQuery({
    queryKey: ['team', 'directory', project],
    queryFn: async ({ signal }: { signal: AbortSignal }) => {
      try {
        return await apiPost<{ data: Directory }>('/api/team/directory', { project }, signal);
      } catch (error) {
        if (isAuthError(error)) queryClient.setQueryData(['team', 'directory', project], null);
        throw error;
      }
    },
    enabled: Boolean(project) && !revoked,
    meta: { failureScope: 'object' },
    gcTime: 0,
    refetchOnMount: 'always',
    refetchOnWindowFocus: true,
    refetchInterval: false,
    retry: false,
  });

  if (revoked || isAuthError(projects.error) || isAuthError(capabilities.error)
    || isAuthError(model.error) || isAuthError(directoryQuery.error)) {
    return <ObservationForbidden />;
  }

  const directory = directoryQuery.isFetchedAfterMount && !directoryQuery.isError && !isAuthError(directoryQuery.error)
    ? directoryQuery.data?.data : undefined;
  const noProjects = projects.data?.projects.length === 0;
  const loading = projects.isPending || Boolean(project) && (capabilities.isPending || model.isPending || directoryQuery.isPending);

  const dutyPending = Boolean(project) && capabilities.isPending;
  const dutyResult = capabilities.isSuccess && capabilities.isFetchedAfterMount && !capabilities.isError
    ? capabilities.data?.result as DutyCapabilities | undefined : undefined;
  const parsedDuties = parseDutyPresets(dutyResult?.business_role_presets);
  const dutyMalformed = Boolean(dutyResult) && parsedDuties.status === 'malformed';
  const dutyFailed = projects.isError || capabilities.isError
    || (!dutyPending && Boolean(project) && !capabilities.data)
    || dutyMalformed;
  const dutyEmpty = !dutyFailed && (noProjects || (!dutyPending && parsedDuties.status === 'empty'));
  const dutyMissing = !dutyFailed && !dutyEmpty && !dutyPending && parsedDuties.status === 'missing';
  const dutyPartial = !dutyFailed && !dutyEmpty && !dutyMissing && !dutyPending && parsedDuties.status === 'partial';
  const dutyRoles = dutyFailed ? [] : parsedDuties.roles;

  const departments = collectDepartments(directory);
  const departmentTree = buildDepartmentTree(departments);
  const organizationFailed = projects.isError || directoryQuery.isError
    || (!directoryQuery.isPending && Boolean(project) && !directory)
    || departmentTree.invalidHierarchy;

  const anyFailure = projects.isError || capabilities.isError || model.isError || directoryQuery.isError
    || dutyMalformed || dutyPartial || departmentTree.invalidHierarchy;
  const totalFailure = !loading && (projects.isError && !project
    || Boolean(project) && (capabilities.isError || dutyMalformed) && model.isError
    && (directoryQuery.isError || departmentTree.invalidHierarchy));
  const unknown = !loading && !totalFailure && model.data?.ordering === 'incomparable';
  const empty = !anyFailure && (noProjects || directory?.configured === false);
  const state: SettingsReadState = loading ? 'loading'
    : totalFailure ? 'failed'
      : unknown ? 'unknown'
        : anyFailure ? 'partial'
          : empty ? 'empty' : 'data';

  const dutyColumns = [
    { key: 'ability', title: <span aria-hidden="true" /> },
    ...dutyRoles.map((role) => ({
      key: role,
      title: t(`settings.duties.roles.${role}`, { defaultValue: t('settings.dutyUnknown') }),
      width: 'var(--settings-duty-column)',
    })),
  ];
  const dutyRows: DataTableRow[] = !dutyFailed && !dutyEmpty && !dutyMissing && !dutyPending
    ? DUTY_ABILITY_ROWS.map((ability) => ({
      key: ability.key,
      testId: `settings-duty-row-${ability.key}`,
      cells: [
        t(`settings.duties.abilities.${ability.key}`),
        ...dutyRoles.map((role) => {
          const ceiling = parsedDuties.ceilings.get(role);
          if (!ceiling) {
            return (
              <span
                key={role}
                className="settings-duty-unknown"
                data-testid={`settings-duty-cell-${ability.key}-${role}`}
                data-allowed="unknown"
              >
                {t('settings.duties.cellUnknown')}
              </span>
            );
          }
          const allowed = ability.actions.every((action) => ceiling.has(action));
          if (allowed) {
            return (
              <span
                key={role}
                className="settings-duty-allowed"
                data-testid={`settings-duty-cell-${ability.key}-${role}`}
                data-allowed="true"
                aria-label={t('settings.duties.allowed')}
              >
                <DisplayIcon icon={CheckIcon} />
              </span>
            );
          }
          return (
            <span
              key={role}
              className="settings-duty-denied"
              data-testid={`settings-duty-cell-${ability.key}-${role}`}
              data-allowed="false"
              aria-label={t('settings.duties.notIncluded')}
            >
              —
            </span>
          );
        }),
      ],
    }))
    : [];
  const dutyState = dutyEmpty ? (
    <TableReadState testId="settings-duty-empty">{t('settings.dutiesEmpty')}</TableReadState>
  ) : dutyFailed ? (
    <TableReadState testId="settings-duty-read-failed">{t('settings.dutyReadFailed')}</TableReadState>
  ) : dutyPending ? (
    <TableReadState testId="settings-loading">{t('settings.loading')}</TableReadState>
  ) : dutyMissing ? (
    <TableReadState testId="settings-duty-definitions-missing">{t('settings.dutyDefinitionsMissing')}</TableReadState>
  ) : null;

  const name = (value: string | null, fallback: string) => value ? readableNarrative(value, i18n.language) ?? fallback : fallback;
  const organizationName = name(directory?.organization ?? null, t('settings.organizationUnknown'));
  const departmentCount = (key: string) => directory?.members.filter((member) => (
    member.departments.some((department) => department.key === key)
  )).length ?? 0;

  const renderDepartmentNodes = (nodes: readonly DepartmentTreeNode[]): ReactNode => {
    if (nodes.length === 0) return null;
    return (
      <ul>
        {nodes.map(({ department, orderIndex, children }) => {
          const displayName = name(department.name, t('settings.departmentUnknown', { number: orderIndex + 1 }));
          const count = departmentCount(department.key);
          const label = count > 0
            ? t('settings.departmentMembers', { name: displayName, count })
            : displayName;
          return (
            <li key={department.key}>
              <span className="nd settings-org-node">
                <span className="settings-org-node-label" data-truncate={department.name.length > 80 || undefined}>
                  {label}
                </span>
              </span>
              {renderDepartmentNodes(children)}
            </li>
          );
        })}
      </ul>
    );
  };

  const statusRows: DataTableRow[] = [
    ...TASK_STATUSES.map((status) => ({
      key: status,
      cells: [
        <TaskStatusTag key="status" status={status} label={t(`settings.taskStatus.${status}.label`)} meaning={t(`settings.taskStatus.${status}.meaning`)} />,
        t(`settings.taskStatus.${status}.meaning`),
      ],
    })),
    ...HEALTH_STATUSES.map((status) => ({
      key: status,
      cells: [
        <HealthTag key="status" status={status} label={t(`settings.healthStatus.${status}.label`)} meaning={t(`settings.healthStatus.${status}.meaning`)} />,
        t(`settings.healthStatus.${status}.meaning`),
      ],
    })),
  ];

  const activeTag = <span className="obs-tag" data-tone="info">{t('settings.inUse')}</span>;
  const reservedTag = <span className="obs-tag">{t('settings.reserved')}</span>;
  const coverage = state === 'loading' ? t('settings.coverageUnread')
    : state === 'partial' || state === 'failed' || dutyMissing || dutyPartial ? t('settings.coveragePartial')
      : t('settings.coverageComplete');
  const retry = () => {
    void Promise.all([
      projects.refetch(),
      ...(project ? [capabilities.refetch(), model.refetch(), directoryQuery.refetch()] : []),
    ]);
  };

  return (
    <div className="settings-page" data-testid="page-settings" data-state={state}>
      <PageHeader title={t('nav.settings')} subtitle={directory ? organizationName : t('settings.organizationUnknown')} />
      <div data-testid="settings-scope-notice">
        <Notice className="settings-scope-notice" title={t('settings.noticeTitle')}>
          {t('settings.noticeBefore')}
          <Link className="settings-inline-link" aria-label={t('nav.account')} to={connectionPath('account', project)}>{t('settings.noticeAccount')}</Link>
          {t('settings.noticeListSeparator')}
          <Link className="settings-inline-link" aria-label={t('nav.agentConnect')} to={connectionPath('agent-connect', project)}>{t('settings.noticeAgent')}</Link>
          {t('settings.noticeAfter')}
        </Notice>
      </div>
      {state === 'failed' || state === 'partial' || state === 'unknown' ? <ReadNotice state={state} onRetry={retry} /> : null}
      <div className="settings-columns">
        <div className="settings-left" data-testid="settings-left">
          <Section title={t('settings.dutiesTitle')} className="settings-duties">
            <div className="settings-duty-table" data-testid="settings-duty-table">
              <DataTable columns={dutyColumns} rows={dutyRows} empty={dutyState} />
            </div>
            <div className="settings-pnote settings-duty-note" data-testid="settings-duty-note">{t('settings.dutyNote')}</div>
          </Section>
          <Section title={t('settings.statusTitle')} className="settings-status">
            <div className="settings-status-table" data-testid="settings-status-table">
              <DataTable
                columns={[
                  { key: 'label', title: t('settings.statusLabelHeader'), width: 'var(--settings-status-label-column)' },
                  { key: 'meaning', title: t('settings.statusMeaningHeader') },
                ]}
                rows={statusRows}
              />
            </div>
          </Section>
        </div>
        <div className="settings-right" data-testid="settings-right">
          <Section title={t('settings.organizationTitle')} className="settings-organization">
            {noProjects || directory?.configured === false ? (
              <div className="settings-org-state" data-testid="settings-organization-empty">{t('settings.organizationEmpty')}</div>
            ) : projects.isError && !directory ? (
              <div className="settings-org-state" data-testid="settings-organization-read-failed">{t('settings.organizationReadFailed')}</div>
            ) : directoryQuery.isPending && !directory ? (
              <div className="settings-org-state" data-testid="settings-loading">{t('settings.loading')}</div>
            ) : organizationFailed || !directory ? (
              <div className="settings-org-state" data-testid="settings-organization-read-failed">{t('settings.organizationReadFailed')}</div>
            ) : (
              <div
                className="settings-org"
                data-testid="settings-organization-tree"
                data-hierarchy={departmentTree.hasHierarchy ? 'true' : 'false'}
              >
                <ul>
                  <li>
                    <span className="nd settings-org-node">
                      <DisplayIcon icon={BuildingIcon} />
                      <span className="settings-org-node-label" data-truncate={organizationName.length > 80 || undefined}>{organizationName}</span>
                    </span>
                    {renderDepartmentNodes(departmentTree.roots)}
                  </li>
                </ul>
              </div>
            )}
            <div className="settings-pnote settings-organization-note" data-testid="settings-organization-note">
              {directory && !organizationFailed && departmentTree.hasHierarchy
                ? t('settings.organizationHierarchyNote')
                : directory && !organizationFailed && departmentTree.roots.length === 0
                  ? t('settings.organizationNoDepartments')
                  : t('settings.organizationNote')}
            </div>
          </Section>
          <div data-testid="settings-vocabulary">
            <Section title={t('settings.vocabularyTitle')} className="settings-vocabulary">
              <RowList items={[{
                key: 'software',
                title: t('settings.vocabularyName'),
                meta: t('settings.vocabularyMeta'),
                trailing: activeTag,
              }]} />
              <div className="settings-pnote settings-vocabulary-note" data-testid="settings-vocabulary-note">{t('settings.vocabularyNote')}</div>
            </Section>
          </div>
          <Section title={t('settings.evidenceTitle')} className="settings-evidence">
            <div data-testid="settings-evidence">
              <RowList items={[
                { key: 'tests', title: t('settings.testResults'), meta: t('settings.testResultsMeta'), trailing: activeTag },
                { key: 'review', title: t('settings.reviewPassed'), meta: t('settings.reviewPassedMeta'), trailing: activeTag },
                { key: 'document', title: t('settings.humanDocument'), meta: t('settings.humanDocumentMeta'), trailing: reservedTag },
              ]} />
            </div>
          </Section>
        </div>
      </div>
      <div className="settings-read-status">
        <span data-testid="settings-read-coverage">{coverage}</span>
        <FreshnessBadge freshness={model.data?.freshness} />
      </div>
    </div>
  );
}
