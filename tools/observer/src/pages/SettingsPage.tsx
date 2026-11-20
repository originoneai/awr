import { useQuery, useQueryClient } from '@tanstack/react-query';
import { Button } from 'antd';
import { Building2 } from 'lucide-react';
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
}

interface DirectoryMember {
  name: string | null;
  available: boolean;
  departments: DirectoryDepartment[];
}

interface Directory {
  configured: boolean;
  organization: string | null;
  departments: DirectoryDepartment[];
  updated_at_ms: number | null;
  members: DirectoryMember[];
}

interface DutyCapabilities {
  business_role_presets?: {
    roles?: unknown;
  };
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

  const directory = directoryQuery.isFetchedAfterMount && !isAuthError(directoryQuery.error)
    ? directoryQuery.data?.data : undefined;
  const noProjects = projects.data?.projects.length === 0;
  const loading = projects.isPending || Boolean(project) && (capabilities.isPending || model.isPending || directoryQuery.isPending);
  const anyFailure = projects.isError || capabilities.isError || model.isError || directoryQuery.isError;
  const totalFailure = !loading && (projects.isError && !project
    || Boolean(project) && capabilities.isError && model.isError && directoryQuery.isError);
  const unknown = !loading && !totalFailure && model.data?.ordering === 'incomparable';
  const empty = !anyFailure && (noProjects || directory?.configured === false);
  const state: SettingsReadState = loading ? 'loading'
    : totalFailure ? 'failed'
      : unknown ? 'unknown'
        : anyFailure ? 'partial'
          : empty ? 'empty' : 'data';

  const dutyResult = capabilities.data?.result as DutyCapabilities | undefined;
  const roleValues = dutyResult?.business_role_presets?.roles;
  const dutyRoles = capabilities.isSuccess && capabilities.isFetchedAfterMount && !capabilities.isError && Array.isArray(roleValues)
    ? roleValues.filter((role): role is string => typeof role === 'string' && role.length > 0)
    : [];
  const dutyFailed = projects.isError || capabilities.isError || (!capabilities.isPending && Boolean(project) && !capabilities.data);
  const dutyMissing = !dutyFailed && !capabilities.isPending && !noProjects;
  const dutyColumns = [
    { key: 'ability', title: <span aria-hidden="true" /> },
    ...dutyRoles.map((role) => ({
      key: role,
      title: t(`settings.duties.roles.${role}`, { defaultValue: t('settings.dutyUnknown') }),
      width: 'var(--settings-duty-column)',
    })),
  ];
  const dutyRows: DataTableRow[] = [];
  const dutyState = noProjects ? (
    <TableReadState testId="settings-duty-empty">{t('settings.dutiesEmpty')}</TableReadState>
  ) : dutyFailed ? (
    <TableReadState testId="settings-duty-read-failed">{t('settings.dutyReadFailed')}</TableReadState>
  ) : capabilities.isPending ? (
    <TableReadState testId="settings-loading">{t('settings.loading')}</TableReadState>
  ) : dutyMissing ? (
    <TableReadState testId="settings-duty-definitions-missing">{t('settings.dutyDefinitionsMissing')}</TableReadState>
  ) : null;

  const name = (value: string | null, fallback: string) => value ? readableNarrative(value, i18n.language) ?? fallback : fallback;
  const organizationName = name(directory?.organization ?? null, t('settings.organizationUnknown'));
  const departments = [...new Map([
    ...(directory?.departments ?? []),
    ...(directory?.members.flatMap((member) => member.departments) ?? []),
  ].map((department) => [department.key, department])).values()];
  const departmentCount = (key: string) => directory?.members.filter((member) => (
    member.departments.some((department) => department.key === key)
  )).length ?? 0;

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
    : state === 'partial' || state === 'failed' || dutyMissing ? t('settings.coveragePartial')
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
            {empty ? (
              <div className="settings-org-state" data-testid="settings-organization-empty">{t('settings.organizationEmpty')}</div>
            ) : projects.isError && !directory ? (
              <div className="settings-org-state" data-testid="settings-organization-read-failed">{t('settings.organizationReadFailed')}</div>
            ) : directoryQuery.isPending && !directory ? (
              <div className="settings-org-state" data-testid="settings-loading">{t('settings.loading')}</div>
            ) : directoryQuery.isError && !directory ? (
              <div className="settings-org-state" data-testid="settings-organization-read-failed">{t('settings.organizationReadFailed')}</div>
            ) : directory ? (
              <div className="settings-org" data-testid="settings-organization-tree">
                <ul>
                  <li>
                    <span className="nd settings-org-node">
                      <DisplayIcon icon={Building2} />
                      <span className="settings-org-node-label" data-truncate={organizationName.length > 80 || undefined}>{organizationName}</span>
                    </span>
                    <ul>
                      {departments.map((department, index) => (
                        <li key={department.key}>
                          <span className="nd settings-org-node">
                            <span className="settings-org-node-label" data-truncate={department.name.length > 80 || undefined}>
                              {t('settings.departmentMembers', {
                                name: name(department.name, t('settings.departmentUnknown', { number: index + 1 })),
                                count: departmentCount(department.key),
                              })}
                            </span>
                          </span>
                        </li>
                      ))}
                    </ul>
                  </li>
                </ul>
              </div>
            ) : (
              <div className="settings-org-state" data-testid="settings-organization-read-failed">{t('settings.organizationReadFailed')}</div>
            )}
            <div className="settings-pnote settings-organization-note" data-testid="settings-organization-note">{t('settings.organizationNote')}</div>
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
