import { useQuery, type UseQueryOptions } from '@tanstack/react-query';
import { Button } from 'antd';
import type { TFunction } from 'i18next';
import { useMemo, useState, type ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import { useNavigate, useSearchParams } from 'react-router';
import { DependencyObservations } from '../components/DependencyObservations';
import { FreshnessBadge } from '../components/FreshnessBadge';
import { RecordedTime } from '../components/RecordedTime';
import { Chain, type ChainNodeView } from '../components/display/Chain';
import { DataTable, type DataTableRow } from '../components/display/DataTable';
import { DisplayIcon } from '../components/display/DisplayIcon';
import { Notice } from '../components/display/Notice';
import { RowList, type RowListItem } from '../components/display/RowList';
import { Section } from '../components/display/Section';
import { StatusLamp, TaskStatusTag } from '../components/display/StatusTag';
import { ArrowLeftIcon, ArrowRightIcon, CircleCheckIcon } from '../components/icons/prototype-icons';
import {
  deriveProjectDependencies,
  hasRecordedCommitmentTitle,
  mergeDependenciesFreshness,
  type DependencyAdoptedCell,
  type DependencyAvailableCell,
  type DependencyRowStatus,
  type ProjectBlockerItem,
  type ProjectChainDisplay,
  type ProjectDirectoryMemberRole,
} from '../components/projects/project-dependencies-model';
import type { ProjectListEntry, ProjectListModel } from '../components/projects/project-list-model';
import { ObservationForbidden } from '../components/states';
import { workbenchAgentName, workbenchDisplayText } from '../components/workbench/model';
import { readableNarrative, readsAsName, workLabel } from '../core/observation';
import { containsDiagnosticContent } from '../core/observation/readable';
import { hasObservationValue } from '../core/observation/values';
import type { ChainNode, DerivedTask, OverviewPart } from '../core/overview';
import { humanText } from '../i18n/human-language';
import { apiPost, isAuthError } from '../lib/api';
import { dependenciesQueryOptions } from '../lib/collaboration';
import { findForbiddenVisibleText } from '../lib/human-language-guard';
import { projectLabel } from '../lib/project-label';
import {
  overviewModelQueryOptions,
  overviewQueryOptions,
  projectsQueryOptions,
  readQueryDefaults,
  SHELL_POLL_INTERVAL_MS,
} from '../lib/queries';
import { useRevocation } from '../lib/revocation';
import { taskPath } from '../lib/task-path';
import type { HandoffListData, TeamFreshness, TeamReadResponse } from '../lib/types';
import { PERSON_ROUTE_BASE } from './MembersPage';
import { ProjectMaps } from './ProjectMaps';
import './ProjectDependencies.css';

const VIEW_KEYS = ['overview', 'map', 'details', 'handoffs'] as const;
type DependenciesView = (typeof VIEW_KEYS)[number];

const FAILED_FRESHNESS: TeamFreshness = {
  report_received: { status: 'failed' },
  authoritative: { status: 'failed' },
  page_read: { status: 'failed' },
};

const HANDOFF_STATUS = {
  proposed: 'proposed',
  inspected: 'inspected',
  accepted: 'accepted',
  rejected: 'rejected',
  cancelled: 'cancelled',
  expired: 'expired',
} as const;

type DetailSelection =
  | { kind: 'chains' }
  | { kind: 'blocker'; key: string }
  | { kind: 'chain-impact'; index: number };

interface ProjectDirectoryResponse {
  data?: {
    configured?: boolean;
    members?: Array<{
      personId?: string | null;
      member_ref?: { person_id?: string | null };
      available?: boolean;
      duty?: string | null;
      business_roles?: string[] | null;
    }>;
  };
}

function asView(value: string | null): DependenciesView {
  return (VIEW_KEYS as readonly string[]).includes(value ?? '') ? (value as DependenciesView) : 'overview';
}

function validChainIndex(value: string | null, count: number): number {
  const parsed = Number(value ?? 0);
  return Number.isInteger(parsed) && parsed >= 0 && parsed < count ? parsed : 0;
}

/**
 * Pass every human-facing free-text field (requirements, decision questions, outside party names,
 * commitment titles, agent names) through product-language cleaning before rendering.
 */
function sanitizeHumanText(
  raw: string | null | undefined,
  language: string,
  fallback: string,
  budget = 120,
): string {
  if (typeof raw !== 'string') return fallback;
  const normalized = raw.replace(/\s+/g, ' ').trim();
  if (!normalized || /[\p{Cc}\p{Cf}]/u.test(normalized)) return fallback;
  const cleaned = readableNarrative(normalized, language);
  if (!cleaned) return fallback;
  if (containsDiagnosticContent(cleaned) || findForbiddenVisibleText(cleaned).length > 0) {
    return fallback;
  }
  return workbenchDisplayText(cleaned, fallback, budget);
}

function safeTitle(raw: string | null | undefined, language: string, fallback: string): string {
  if (!raw) return fallback;
  const labelled = workLabel(raw, { language, recordedTitle: true, fallback }).text;
  return sanitizeHumanText(labelled, language, fallback);
}

function shortQueueTitle(raw: string | null | undefined, language: string, fallback: string): string {
  const full = safeTitle(raw, language, fallback);
  return full.split('：')[0] || full;
}

function safePersonName(raw: unknown, language: string, fallback: string): string {
  if (!hasObservationValue(raw)) return fallback;
  return typeof raw === 'string' && readsAsName(raw.trim())
    ? raw.trim()
    : humanText('nameUnavailable', language);
}

function safeAgentName(raw: string | null | undefined, language: string, fallback: string): string {
  const stripped = workbenchAgentName(raw, '');
  return stripped ? sanitizeHumanText(stripped, language, fallback, 60) : fallback;
}

function partNames(parts: readonly OverviewPart[], t: TFunction): string {
  return parts.map((part) => t(`workbench.home.coverageParts.${part}`)).join(t('common.listSeparator'));
}

function projectHandoffsQueryOptions(project: string, cursor: string | null, revoked: boolean, enabled: boolean) {
  return {
    queryKey: ['team', 'handoff.list', 'project', project, cursor],
    meta: { failureScope: 'object' },
    queryFn: ({ signal }: { signal: AbortSignal }) => apiPost<TeamReadResponse<HandoffListData>>('/api/team/read', {
      project,
      query: {
        protocol_version: 1,
        op: 'handoff.list',
        handoff_state: 'all',
        limit: 20,
        max_context_bytes: 65536,
        ...(cursor ? { cursor } : {}),
      },
    }, signal),
    enabled: Boolean(project) && enabled && !revoked,
    ...readQueryDefaults,
  } satisfies UseQueryOptions<TeamReadResponse<HandoffListData>>;
}

function projectDirectoryRolesQueryOptions(project: string, revoked: boolean) {
  return {
    queryKey: ['team', 'directory', 'roles', project],
    meta: { failureScope: 'object' },
    queryFn: ({ signal }: { signal: AbortSignal }) =>
      apiPost<ProjectDirectoryResponse>('/api/team/directory', { project }, signal),
    enabled: Boolean(project) && !revoked,
    retry: false,
    refetchInterval: SHELL_POLL_INTERVAL_MS,
    refetchOnWindowFocus: false,
  } satisfies UseQueryOptions<ProjectDirectoryResponse>;
}

function formatAdoptedCell(cell: DependencyAdoptedCell, t: TFunction): string {
  switch (cell.kind) {
    case 'matchesAvailable': return t('projectDependencies.table.adoptedMatchesAvailable');
    case 'differsFromAvailable': return t('projectDependencies.table.adoptedDiffersFromAvailable');
    case 'recorded': return t('projectDependencies.table.adoptedRecorded');
    case 'restricted': return t('projectDependencies.table.restricted');
    case 'adoptionUnavailable': return t('projectDependencies.table.adoptionUnavailable');
    case 'notRecorded': return t('projectDependencies.table.notRecorded');
  }
}

function formatAvailableCell(cell: DependencyAvailableCell, t: TFunction): string {
  switch (cell.kind) {
    case 'recorded': return t('projectDependencies.table.availableRecorded');
    case 'inReview': return t('projectDependencies.table.availableInReview');
    case 'stalledAnswer': return t('projectDependencies.table.availableStalledAnswer');
    case 'stalled': return t('projectDependencies.table.availableStalled');
    case 'waitingUpstream': return t('projectDependencies.table.availableWaitingUpstream');
    case 'cannotStart': return t('projectDependencies.table.availableCannotStart');
    case 'waiting': return t('projectDependencies.table.availableWaiting');
    case 'running': return t('projectDependencies.table.availableRunning');
    case 'returned': return t('projectDependencies.table.availableReturned');
    case 'claimed': return t('projectDependencies.table.availableClaimed');
    case 'restricted': return t('projectDependencies.table.restricted');
    case 'notRecorded': return t('projectDependencies.table.notRecorded');
  }
}

function DependencyStatusTag({ status }: { status: DependencyRowStatus }) {
  const { t } = useTranslation();
  const tone = status === 'changed' || status === 'unadopted'
    ? 'risk'
    : status === 'ok'
      ? 'ok'
      : undefined;
  const lamp = status === 'changed' || status === 'unadopted'
    ? 'stalled' as const
    : status === 'ok'
      ? 'done' as const
      : null;
  return (
    <span className="obs-tag" data-tone={tone} data-dep-status={status}>
      {lamp ? <StatusLamp lamp={lamp} /> : null}
      {t(`projectDependencies.table.status.${status}`)}
    </span>
  );
}

function chainTabLabel(
  entry: ProjectChainDisplay,
  reviewChainCount: number,
  t: TFunction,
  language: string,
): string {
  const root = entry.chain.root;
  if (root.kind === 'review') {
    const name = root.person?.displayName ? safePersonName(root.person.displayName, language, '') : '';
    if (reviewChainCount > 1 && name) return t('projectDependencies.chain.tabReviewNamed', { name });
    return t('projectDependencies.chain.tabReview');
  }
  if (root.kind === 'external') {
    const party = sanitizeHumanText(root.externalParty, language, '');
    return party
      ? t('projectDependencies.chain.tabExternal', { party })
      : t('projectDependencies.chain.tabExternalUnknown');
  }
  const firstWait = root.queue.items[0];
  const responder = firstWait?.waitingOn[0];
  const name = responder?.displayName ? safePersonName(responder.displayName, language, '') : '';
  const isOwnTask = responder?.id != null && firstWait?.task.owner?.id === responder.id;
  if (name) {
    return isOwnTask
      ? t('projectDependencies.chain.tabDecisionAnswerNamed', { name })
      : t('projectDependencies.chain.tabDecisionDecideNamed', { name });
  }
  return t('projectDependencies.chain.tabDecisionUnknown');
}

function blockerTitle(blocker: ProjectBlockerItem, t: TFunction, language: string): string {
  if (blocker.kind === 'review') {
    return blocker.personName
      ? t('projectDependencies.blockers.reviewQueueTitle', { name: blocker.personName })
      : t('projectDependencies.blockers.reviewQueueUnknownTitle');
  }
  if (blocker.kind === 'external') {
    const party = sanitizeHumanText(blocker.externalParty, language, '');
    const what = sanitizeHumanText(blocker.what, language, '');
    if (party && what) return t('projectDependencies.blockers.externalTitle', { party, what });
    if (party) return t('projectDependencies.blockers.externalFallbackTitle', { party });
    return t('projectDependencies.blockers.externalUnknownTitle');
  }
  if (blocker.personName) {
    return blocker.isOwnTaskAnswer
      ? t('projectDependencies.blockers.decisionAnswerTitle', { name: blocker.personName })
      : t('projectDependencies.blockers.decisionDecideTitle', { name: blocker.personName });
  }
  return t('projectDependencies.blockers.decisionUnknownTitle');
}

function blockerMeta(blocker: ProjectBlockerItem, t: TFunction, language: string): string {
  if (blocker.kind === 'review') {
    return blocker.oldestDays !== null
      ? t('projectDependencies.blockers.reviewQueueMeta', { count: blocker.queuedCount, days: blocker.oldestDays })
      : t('projectDependencies.blockers.reviewQueueMetaUnknownAge', { count: blocker.queuedCount });
  }
  if (blocker.kind === 'external') {
    return blocker.oldestDays !== null
      ? t('projectDependencies.blockers.externalMeta', { days: blocker.oldestDays })
      : t('projectDependencies.blockers.externalMetaUnknownAge');
  }
  const stripped = blocker.what?.replace(/^(?:回答|决定)[：:]/, '');
  const what = sanitizeHumanText(stripped, language, t('projectDependencies.blockers.decisionMetaFallback'));
  return blocker.oldestDays !== null
    ? t('projectDependencies.blockers.decisionMetaWithAge', { what, days: blocker.oldestDays })
    : what;
}

/** Project-level People & Handoffs view (Criterion 4: read-only switchable view). */
function ProjectHandoffsView({
  project,
  tasks,
  onOpenTask,
}: {
  project: string;
  tasks: readonly DerivedTask[];
  onOpenTask: (workId: string) => void;
}) {
  const { t, i18n } = useTranslation();
  const language = i18n.language;
  const { revoked } = useRevocation();
  const [cursors, setCursors] = useState<Array<string | null>>([null]);
  const handoffs = useQuery(projectHandoffsQueryOptions(project, cursors.at(-1) ?? null, revoked, true));

  if (revoked || isAuthError(handoffs.error)) {
    return <ObservationForbidden />;
  }

  const data = handoffs.data?.result.data;
  const handoffItems: RowListItem[] = (data?.items ?? []).map((item) => {
    const statusKey = Object.hasOwn(HANDOFF_STATUS, item.effective_status)
      ? HANDOFF_STATUS[item.effective_status as keyof typeof HANDOFF_STATUS]
      : 'unknown';
    const from = safePersonName(item.from_display_name, language, t('collaboration.nameMissing'));
    const to = safePersonName(item.to_display_name, language, t('collaboration.nameMissing'));
    const workId = (item as { work_id?: string }).work_id;
    const workTitle = (item as { work_title?: string }).work_title;
    const taskLabel = workTitle ? safeTitle(workTitle, language, t('projectDependencies.detail.taskUnnamed')) : null;
    const kindLabel = t(item.kind === 'responsibility' ? 'collaboration.responsibilityHandoff' : 'collaboration.executionHandoff');
    const statusLabel = t(`collaboration.handoffStatus.${statusKey}`);
    return {
      key: item.handoff_id,
      title: taskLabel
        ? `${taskLabel} · ${t('collaboration.handoffPeople', { from, to })}`
        : t('collaboration.handoffPeople', { from, to }),
      meta: (
        <>
          {t('projectDependencies.handoffs.handoffMeta', { kind: kindLabel, status: statusLabel })}
          {' · '}
          <RecordedTime value={item.updated_at_unix_ms} />
        </>
      ),
      onOpen: workId ? () => onOpenTask(workId) : undefined,
    };
  });

  const responsibilityRows: DataTableRow[] = tasks.map((task, idx) => ({
    key: task.id ?? `task-${idx}`,
    onOpen: task.id ? () => onOpenTask(task.id!) : undefined,
    cells: [
      <span className="project-deps-consumer">
        {safeTitle(task.title, language, t('projectDependencies.detail.taskUnnamed'))}
      </span>,
      task.owner?.displayName
        ? safePersonName(task.owner.displayName, language, t('collaboration.nameMissing'))
        : <span className="project-deps-dim">{t('projectDependencies.detail.ownerUnknown')}</span>,
      task.agentName
        ? safeAgentName(task.agentName, language, t('projectDependencies.handoffs.noAgent'))
        : <span className="project-deps-dim">{t('projectDependencies.handoffs.noAgent')}</span>,
      <TaskStatusTag status={task.status} live={task.agentActive === true} />,
    ],
  }));

  return (
    <div className="project-deps-handoffs-grid" data-testid="project-deps-handoffs">
      <Section
        title={t('projectDependencies.handoffs.responsibilitiesTitle')}
        count={tasks.length || undefined}
        className="project-deps-section"
      >
        <DataTable
          columns={[
            { key: 'task', title: t('projectDependencies.handoffs.ownerColumn'), width: '34%' },
            { key: 'owner', title: t('projectDependencies.handoffs.assigneeColumn'), width: 120 },
            { key: 'agent', title: t('projectDependencies.handoffs.agentColumn'), width: 136, hideOnSmall: true },
            { key: 'status', title: t('projectDependencies.handoffs.statusColumn'), width: 136 },
          ]}
          rows={responsibilityRows}
          navigationWidth={36}
          stickyNavigation={false}
          empty={<div className="project-deps-empty">{t('projectDependencies.handoffs.responsibilitiesEmpty')}</div>}
        />
      </Section>

      <Section
        title={t('projectDependencies.handoffs.title')}
        count={data?.total || undefined}
        className="project-deps-section"
      >
        {handoffs.isPending ? (
          <div className="project-deps-empty">{t('projectDependencies.handoffs.loading')}</div>
        ) : handoffs.isError ? (
          <div className="project-deps-empty">{t('projectDependencies.handoffs.failed')}</div>
        ) : (
          <>
            <RowList
              items={handoffItems}
              empty={<div className="project-deps-empty">{t('projectDependencies.handoffs.empty')}</div>}
            />
            {(cursors.length > 1 || data?.next_cursor) ? (
              <div className="project-deps-pagination">
                <span>{t('projectDependencies.handoffs.totalCount', { count: data?.total ?? handoffItems.length })}</span>
                <span className="project-deps-pagination-actions">
                  {cursors.length > 1 ? (
                    <Button size="small" onClick={() => setCursors(cursors.slice(0, -1))}>
                      {t('plan.observation.previous')}
                    </Button>
                  ) : null}
                  {data?.next_cursor ? (
                    <Button size="small" onClick={() => setCursors([...cursors, data.next_cursor])}>
                      {t('plan.observation.next')}
                    </Button>
                  ) : null}
                </span>
              </div>
            ) : null}
          </>
        )}
      </Section>
    </div>
  );
}

/**
 * Project Dependencies tab (AWR-PPO-043):
 * Default view matches prototype pDeps() pixel-for-pixel (top blocker chain, bottom-left
 * upstream/downstream version table, bottom-right impact-sorted blockers), with switchable
 * Relationship map, Dependency details, and People & handoffs views preserved on the same page.
 */
export function ProjectDependencies({ project }: { project: string }) {
  const { t, i18n } = useTranslation();
  const language = i18n.language;
  const navigate = useNavigate();
  const { revoked } = useRevocation();
  const [searchParams, setSearchParams] = useSearchParams();
  const [cursors, setCursors] = useState<Array<string | null>>([null]);
  const [detail, setDetail] = useState<DetailSelection | null>(null);

  const activeView = asView(searchParams.get('view'));

  const projects = useQuery(projectsQueryOptions(revoked));
  const overview = useQuery(overviewQueryOptions(project, revoked));
  const modelQuery = useQuery(overviewModelQueryOptions(project || null, revoked));
  const dependencies = useQuery(dependenciesQueryOptions(project, null, cursors.at(-1) ?? null, revoked));
  const directoryQuery = useQuery(projectDirectoryRolesQueryOptions(project, revoked));

  const projectEntries = projects.data?.projects as ProjectListEntry[] | undefined;
  const projectIndex = projectEntries ? projectEntries.findIndex((item) => item.key === project) : -1;
  const projectEntry = projectIndex >= 0 && projectEntries ? projectEntries[projectIndex] : undefined;
  const projectTitle = projectEntry ? projectLabel(projectEntry, projectIndex + 1) : t('projectDependencies.chain.commitmentProjectKicker');

  const modelData = modelQuery.data?.project === project && modelQuery.data.input && modelQuery.data.input.options
    ? (modelQuery.data as ProjectListModel)
    : undefined;
  const dependenciesData = dependencies.data?.result.data;

  const directoryMembers = useMemo<ProjectDirectoryMemberRole[] | null>(() => {
    const dir = directoryQuery.data?.data;
    if (!dir?.configured || !Array.isArray(dir.members) || dir.members.length === 0) return null;
    return dir.members.map((member) => ({
      personId: member.personId ?? member.member_ref?.person_id ?? null,
      available: member.available === true,
      duty: member.duty ?? null,
      businessRoles: member.business_roles ?? null,
    }));
  }, [directoryQuery.data]);

  const derived = useMemo(() => deriveProjectDependencies({
    projectKey: project,
    overview: overview.data,
    model: modelData,
    dependencies: dependenciesData,
    directoryMembers,
  }), [project, overview.data, modelData, dependenciesData, directoryMembers]);

  if (
    revoked
    || isAuthError(projects.error)
    || isAuthError(overview.error)
    || isAuthError(modelQuery.error)
    || isAuthError(dependencies.error)
  ) {
    return <ObservationForbidden />;
  }

  const scopeQuery = `scope=${encodeURIComponent(project)}`;
  const activeChainIndex = validChainIndex(searchParams.get('chain'), derived.chains.length);
  const activeChainEntry = derived.chains[activeChainIndex];

  function updateParams(patch: Record<string, string | null>) {
    const next = new URLSearchParams(searchParams);
    for (const [key, value] of Object.entries(patch)) {
      if (value === null) next.delete(key);
      else next.set(key, value);
    }
    setSearchParams(next);
  }

  function setView(nextView: DependenciesView) {
    setDetail(null);
    updateParams({ view: nextView === 'overview' ? null : nextView });
  }

  function openTask(workId: string) {
    navigate(taskPath(project, workId));
  }

  function openPerson(personId: string) {
    navigate(`/${PERSON_ROUTE_BASE}/${encodeURIComponent(personId)}?${scopeQuery}`);
  }

  function openGoal(goalId: string) {
    navigate(`/goals/${encodeURIComponent(goalId)}?${scopeQuery}`);
  }

  function openPlan() {
    const next = new URLSearchParams(searchParams);
    next.set('tab', 'plan');
    next.delete('view');
    next.delete('chain');
    setSearchParams(next);
  }

  function retryFailed() {
    if (modelQuery.isError) void modelQuery.refetch();
    if (dependencies.isError) {
      if (cursors.length > 1) setCursors([null]);
      else void dependencies.refetch();
    }
    if (overview.isError) void overview.refetch();
  }

  const viewsBar = (position: 'top' | 'bottom') => (
    <div className="project-deps-views-bar" data-position={position} data-testid="project-deps-view-switcher">
      <div className="project-deps-views-seg" role="group" aria-label={t('projectDependencies.views.groupLabel')}>
        {VIEW_KEYS.map((key) => (
          <button
            key={key}
            type="button"
            aria-pressed={activeView === key}
            data-testid={`project-deps-view-${key}`}
            onClick={() => setView(key)}
          >
            {t(`projectDependencies.views.${key}`)}
          </button>
        ))}
      </div>
    </div>
  );

  // Switchable views (Criterion 4): Relationship map, Dependency details, People & handoffs.
  if (activeView !== 'overview') {
    return (
      <div className="project-deps" data-testid="page-project-dependencies" data-view={activeView}>
        {viewsBar('top')}
        {activeView === 'map' ? <ProjectMaps project={project} /> : null}
        {activeView === 'details' ? <DependencyObservations key={project} project={project} /> : null}
        {activeView === 'handoffs' ? (
          <ProjectHandoffsView
            project={project}
            tasks={derived.derived?.tasks ?? []}
            onOpenTask={openTask}
          />
        ) : null}
      </div>
    );
  }

  const primaryLoading = modelQuery.isPending && dependencies.isPending;
  const primaryFailed = !primaryLoading && modelQuery.isError && !modelData && dependencies.isError && !dependenciesData;
  const missingModelParts = [...derived.coverage.modelPartialParts, ...derived.coverage.modelUnreadParts];
  const missingReadLabels: string[] = [];
  if (dependencies.isError) missingReadLabels.push(t('projectDependencies.states.missingDepsRead'));
  if (modelQuery.isError) missingReadLabels.push(t('projectDependencies.states.missingChainsAndBlockersRead'));
  if (overview.isError || Boolean(overview.data && overview.data.coverage.failed > 0)) {
    missingReadLabels.push(t('projectDependencies.states.missingOverviewRead'));
  }
  if (missingModelParts.length > 0) missingReadLabels.push(partNames(missingModelParts, t));

  const partialFailure = !primaryLoading && !primaryFailed && (
    modelQuery.isError
    || dependencies.isError
    || overview.isError
    || missingModelParts.length > 0
    || Boolean(overview.data && overview.data.coverage.failed > 0)
  );
  const showUnknownNotice = !primaryLoading && !primaryFailed && !partialFailure
    && !modelQuery.isPending && !dependencies.isPending
    && (derived.coverage.orderingIncomparable || derived.coverage.hasUnknownFacts);

  const mergedFreshness = mergeDependenciesFreshness(
    dependencies.isError ? FAILED_FRESHNESS : dependencies.data?.freshness,
    modelQuery.isError ? FAILED_FRESHNESS : modelData?.freshness,
    overview.isError ? FAILED_FRESHNESS : overview.data?.freshness,
  );

  if (primaryLoading) {
    return (
      <div className="project-deps" data-testid="page-project-dependencies" data-state="loading">
        <div className="project-deps-state" data-testid="project-deps-loading">
          {t('projectDependencies.states.loading')}
        </div>
      </div>
    );
  }

  if (primaryFailed) {
    return (
      <div className="project-deps" data-testid="page-project-dependencies" data-state="failed">
        <Notice
          tone="warning"
          title={t('projectDependencies.states.failedTitle')}
          extra={<Button size="small" onClick={retryFailed}>{t('projectDependencies.states.retry')}</Button>}
          className="project-deps-notice"
        >
          <span data-testid="project-deps-failed">{t('projectDependencies.states.failedDescription')}</span>
        </Notice>
        {viewsBar('bottom')}
        <footer className="project-deps-footer" data-testid="project-deps-coverage">
          <FreshnessBadge freshness={FAILED_FRESHNESS} />
        </footer>
      </div>
    );
  }

  // Chain node presentation & navigation
  const reviewChainCount = derived.chains.filter((entry) => entry.chain.root.kind === 'review').length;

  function presentChainNode(entry: ProjectChainDisplay, node: ChainNode): ChainNodeView {
    if (node.root) {
      const root = entry.chain.root;
      const firstWait = root.queue.items[0];
      if (root.kind === 'review') {
        const name = root.person?.displayName ? safePersonName(root.person.displayName, language, '') : '';
        return {
          kicker: t('display.chain.rootKicker'),
          title: name
            ? t('projectDependencies.chain.rootReviewQueue', { name })
            : t('projectDependencies.chain.rootReviewUnknown'),
          ownerName: name || null,
          queue: root.queue.items.map((wait) => ({
            title: shortQueueTitle(wait.task.title, language, t('projectDependencies.detail.taskUnnamed')),
            days: wait.durationDays,
          })),
          queueNote: root.queue.count === null ? null : { queued: root.queue.count, oldestDays: root.oldestDays },
          footNote: root.queue.count === null ? t('workbench.home.countUnknown') : null,
          open: Boolean(root.person?.id || firstWait?.task.id),
        };
      }
      if (root.kind === 'external') {
        const party = sanitizeHumanText(root.externalParty, language, '');
        const what = sanitizeHumanText(firstWait?.what, language, '');
        const title = party && what
          ? t('projectDependencies.chain.rootExternal', { party, what })
          : party
            ? t('projectDependencies.chain.rootExternalFallback', { party })
            : t('projectDependencies.chain.rootExternalUnknown');
        return {
          kicker: t('display.chain.rootKicker'),
          title,
          meta: root.oldestDays !== null
            ? t('projectDependencies.chain.externalNoReplyDays', { days: root.oldestDays })
            : t('projectDependencies.chain.externalNoReplyUnknown'),
          ownerName: party || null,
          // Strictly from the read's wait kind ('external'); never inferred from party name patterns.
          ownerExternal: true,
          open: Boolean(firstWait?.task.id),
        };
      }
      const responder = firstWait?.waitingOn[0];
      const responderName = responder?.displayName ? safePersonName(responder.displayName, language, '') : '';
      const isOwnTask = responder?.id != null && firstWait?.task.owner?.id === responder.id;
      const title = responderName
        ? isOwnTask
          ? t('projectDependencies.chain.rootDecisionAnswer', { name: responderName })
          : t('projectDependencies.chain.rootDecisionDecide', { name: responderName })
        : t('projectDependencies.chain.rootDecisionUnknown');
      const rawQuestion = firstWait?.what?.replace(/^(?:回答|决定)[：:]/, '');
      const cleanQuestion = sanitizeHumanText(rawQuestion, language, '');
      return {
        kicker: t('display.chain.rootKicker'),
        title,
        meta: cleanQuestion || null,
        ownerName: responderName || null,
        open: Boolean(firstWait?.task.id),
      };
    }

    if (node.task) {
      const task = node.task;
      const owner = task.owner?.displayName ? safePersonName(task.owner.displayName, language, '') : '';
      const taskWaits = (derived.derived?.waits ?? []).filter((wait) => wait.task.id === task.id);
      const reviewWait = taskWaits.find((wait) => wait.kind === 'review');
      const depWait = taskWaits.find((wait) => wait.kind === 'dependency');
      const extWait = taskWaits.find((wait) => wait.kind === 'external');
      let meta: string | null = owner || null;
      if (owner) {
        if (reviewWait && reviewWait.durationDays !== null) {
          meta = t('projectDependencies.chain.taskMetaOwnerReviewDays', { owner, days: reviewWait.durationDays });
        } else if (extWait) {
          meta = t('projectDependencies.chain.taskMetaOwnerCannotStart', { owner });
        } else if (depWait?.providerTask?.title) {
          meta = t('projectDependencies.chain.taskMetaOwnerWaitingUpstream', {
            owner,
            upstream: shortQueueTitle(depWait.providerTask.title, language, t('projectDependencies.detail.taskUnnamed')),
          });
        } else if (depWait) {
          meta = t('projectDependencies.chain.taskMetaOwnerWaiting', { owner });
        } else {
          meta = t('projectDependencies.chain.taskMetaOwnerStatus', {
            owner,
            status: t(`display.taskStatus.${task.status}.label`),
          });
        }
      }
      return {
        kicker: t('display.chain.taskKicker'),
        title: shortQueueTitle(task.title, language, t('projectDependencies.detail.taskUnnamed')),
        meta,
        status: task.status,
        ownerName: owner || null,
        open: Boolean(task.id),
      };
    }

    // Commitment end node: use explicit commitment kind and recorded title; never sniff IDs or invent risk.
    const commitment = node.commitment;
    const isGoal = commitment?.kind === 'goal';
    const kicker = isGoal
      ? t('projectDependencies.chain.commitmentGoalKicker')
      : t('projectDependencies.chain.commitmentProjectKicker');
    const fallbackTitle = isGoal
      ? t('workbench.home.commitmentGoalFallback')
      : projectTitle;
    const title = hasRecordedCommitmentTitle(commitment, project)
      ? sanitizeHumanText(commitment?.title, language, fallbackTitle)
      : fallbackTitle;
    const progress = entry.commitmentProgress;
    const remaining = progress ? Math.max(0, progress.total - progress.done) : null;
    return {
      kicker,
      title,
      meta: remaining !== null && remaining > 0
        ? t('projectDependencies.chain.commitmentRemaining', { remaining })
        : null,
      health: null,
      progress,
      open: Boolean(commitment?.id),
    };
  }

  function openChainNode(entry: ProjectChainDisplay, node: ChainNode) {
    if (node.root) {
      const root = entry.chain.root;
      if (root.kind === 'review' && root.person?.id) {
        openPerson(root.person.id);
        return;
      }
      const firstTaskId = root.queue.items[0]?.task.id;
      if (firstTaskId) openTask(firstTaskId);
      return;
    }
    if (node.task?.id) {
      openTask(node.task.id);
      return;
    }
    if (node.commitment?.id) {
      if (node.commitment.kind === 'goal') openGoal(node.commitment.id);
      else openPlan();
    }
  }

  function renderChainFooter(entry: ProjectChainDisplay): ReactNode {
    const root = entry.chain.root;
    const impactCount = entry.impact.count;
    let sentence: string;
    let ctaLabel: string | null = null;
    let ctaAction: (() => void) | null = null;

    if (root.kind === 'review') {
      const name = root.person?.displayName ? safePersonName(root.person.displayName, language, '') : '';
      sentence = impactCount === null
        ? t('projectDependencies.chain.footerReviewUnknownCount', { name: name || t('projectDependencies.chain.rootReviewUnknown') })
        : name && entry.soleReviewer
          ? t('projectDependencies.chain.footerReviewSole', { name, count: impactCount })
          : name
            ? t('projectDependencies.chain.footerReview', { name, count: impactCount })
            : t('projectDependencies.chain.footerReviewUnknownPerson', { count: impactCount });
      if (entry.relatedOpenDecision) {
        const question = sanitizeHumanText(entry.relatedOpenDecision.question, language, '');
        if (question) {
          const responder = entry.relatedOpenDecision.responderName
            ? safePersonName(entry.relatedOpenDecision.responderName, language, '')
            : '';
          const suffix = responder
            ? t('projectDependencies.chain.footerReviewRelatedDecisionNamed', { question, responder })
            : t('projectDependencies.chain.footerReviewRelatedDecision', { question });
          sentence = `${sentence}${language.toLowerCase().startsWith('zh') ? '' : ' '}${suffix}`;
        }
      }
      if (root.person?.id && name) {
        ctaLabel = t('projectDependencies.chain.ctaPerson', { name });
        ctaAction = () => openPerson(root.person!.id);
      }
    } else if (root.kind === 'external') {
      sentence = impactCount === null
        ? t('projectDependencies.chain.footerExternalUnknownCount')
        : root.oldestDays !== null
          ? t('projectDependencies.chain.footerExternalDays', { days: root.oldestDays, count: impactCount })
          : t('projectDependencies.chain.footerExternal', { count: impactCount });
      const firstTaskId = root.queue.items[0]?.task.id;
      if (firstTaskId) {
        ctaLabel = t('projectDependencies.chain.ctaTask');
        ctaAction = () => openTask(firstTaskId);
      }
    } else {
      sentence = impactCount === null
        ? t('projectDependencies.chain.footerDecisionUnknownCount')
        : t('projectDependencies.chain.footerDecision', { count: impactCount });
      const firstTaskId = root.queue.items[0]?.task.id;
      if (firstTaskId) {
        ctaLabel = t('projectDependencies.chain.ctaTask');
        ctaAction = () => openTask(firstTaskId);
      }
    }

    return (
      <>
        <span>{sentence}</span>
        {impactCount !== null && impactCount > 0 ? (
          <button
            type="button"
            className="project-deps-link"
            data-testid="project-deps-chain-impact-btn"
            onClick={() => setDetail({ kind: 'chain-impact', index: activeChainIndex })}
          >
            {t('projectDependencies.chain.ctaImpact', { count: impactCount })}
          </button>
        ) : null}
        {ctaLabel && ctaAction ? (
          <button type="button" className="project-deps-link" data-testid="project-deps-chain-cta" onClick={ctaAction}>
            {ctaLabel}
            <DisplayIcon icon={ArrowRightIcon} />
          </button>
        ) : null}
      </>
    );
  }

  // Dependency table rows (prototype pDeps() left table)
  const dependencyRows: DataTableRow[] = derived.rows.map((row) => {
    const consumerText = safeTitle(row.consumerTitle, language, t('projectDependencies.detail.taskUnnamed'));
    const providerText = row.providerTitle
      ? safeTitle(row.providerTitle, language, t('projectDependencies.table.restrictedWork'))
      : t('projectDependencies.table.restrictedWork');
    const requirementText = row.requirement.kind === 'recorded'
      ? sanitizeHumanText(row.requirement.text, language, t('projectDependencies.table.requirementReviewed'))
      : row.requirement.kind === 'pinned'
        ? t('projectDependencies.table.requirementPinned')
        : t('projectDependencies.table.requirementReviewed');
    return {
      key: row.key,
      testId: `dep-row-${row.consumerWorkId}-${row.providerWorkId}`,
      highlighted: row.highlighted,
      onOpen: () => openTask(row.consumerWorkId),
      cells: [
        <span className="project-deps-consumer">{consumerText}</span>,
        <div>
          <div>{providerText}</div>
          <div className="project-deps-req">
            {t('projectDependencies.table.requirementPrefix', { requirement: requirementText })}
          </div>
        </div>,
        formatAdoptedCell(row.adopted, t),
        formatAvailableCell(row.available, t),
        <DependencyStatusTag status={row.status} />,
      ],
    };
  });

  // Inline detail panel for clicked counts (Criterion 5: detail row count === clicked number)
  let detailPanel: ReactNode = null;
  if (detail?.kind === 'chains') {
    const chainItems: RowListItem[] = derived.chains.map((entry, idx) => ({
      key: entry.chain.id,
      leading: <StatusLamp lamp={entry.chain.root.kind === 'review' ? 'blocked' : 'stalled'} />,
      title: chainTabLabel(entry, reviewChainCount, t, language),
      meta: entry.impact.count !== null
        ? t('projectDependencies.blockers.behindCount', { count: entry.impact.count })
        : t('projectDependencies.blockers.behindUnknown'),
      selected: idx === activeChainIndex,
      onOpen: () => {
        updateParams({ chain: String(idx) });
        setDetail(null);
      },
    }));
    detailPanel = (
      <div className="project-deps-detail" data-testid="project-deps-detail" data-kind="chains" data-count={chainItems.length}>
        <Section
          title={t('projectDependencies.detail.chainsTitle')}
          count={chainItems.length}
          className="project-deps-section"
          extra={(
            <button type="button" className="project-deps-link" data-testid="project-deps-detail-back" onClick={() => setDetail(null)}>
              <DisplayIcon icon={ArrowLeftIcon} />
              {t('projectDependencies.detail.back')}
            </button>
          )}
        >
          <RowList items={chainItems} empty={<div className="project-deps-empty">{t('projectDependencies.detail.empty')}</div>} />
        </Section>
      </div>
    );
  } else if (detail?.kind === 'blocker') {
    const blocker = derived.blockers.find((item) => item.key === detail.key);
    if (blocker) {
      const items: RowListItem[] = blocker.behind.items.map((task, idx) => ({
        key: task.id ?? `behind-${idx}`,
        title: safeTitle(task.title, language, t('projectDependencies.detail.taskUnnamed')),
        meta: task.owner?.displayName
          ? safePersonName(task.owner.displayName, language, t('projectDependencies.detail.ownerUnknown'))
          : t('projectDependencies.detail.ownerUnknown'),
        trailing: <TaskStatusTag status={task.status} live={task.agentActive === true} />,
        onOpen: task.id ? () => openTask(task.id!) : undefined,
      }));
      detailPanel = (
        <div className="project-deps-detail" data-testid="project-deps-detail" data-kind="blocker" data-count={items.length}>
          <Section
            title={t('projectDependencies.detail.impactTitle', { name: blockerTitle(blocker, t, language) })}
            count={items.length}
            className="project-deps-section"
            extra={(
              <button type="button" className="project-deps-link" data-testid="project-deps-detail-back" onClick={() => setDetail(null)}>
                <DisplayIcon icon={ArrowLeftIcon} />
                {t('projectDependencies.detail.back')}
              </button>
            )}
          >
            <RowList items={items} empty={<div className="project-deps-empty">{t('projectDependencies.detail.empty')}</div>} />
          </Section>
        </div>
      );
    }
  } else if (detail?.kind === 'chain-impact') {
    const entry = derived.chains[detail.index];
    if (entry) {
      const items: RowListItem[] = entry.impact.items.map((task, idx) => ({
        key: task.id ?? `impact-${idx}`,
        title: safeTitle(task.title, language, t('projectDependencies.detail.taskUnnamed')),
        meta: task.owner?.displayName
          ? safePersonName(task.owner.displayName, language, t('projectDependencies.detail.ownerUnknown'))
          : t('projectDependencies.detail.ownerUnknown'),
        trailing: <TaskStatusTag status={task.status} live={task.agentActive === true} />,
        onOpen: task.id ? () => openTask(task.id!) : undefined,
      }));
      detailPanel = (
        <div className="project-deps-detail" data-testid="project-deps-detail" data-kind="chain-impact" data-count={items.length}>
          <Section
            title={t('projectDependencies.detail.impactTitle', { name: chainTabLabel(entry, reviewChainCount, t, language) })}
            count={items.length}
            className="project-deps-section"
            extra={(
              <button type="button" className="project-deps-link" data-testid="project-deps-detail-back" onClick={() => setDetail(null)}>
                <DisplayIcon icon={ArrowLeftIcon} />
                {t('projectDependencies.detail.back')}
              </button>
            )}
          >
            <RowList items={items} empty={<div className="project-deps-empty">{t('projectDependencies.detail.empty')}</div>} />
          </Section>
        </div>
      );
    }
  }

  // Independent read state for the top chains block: never show calm empty during loading, error, or partial reads.
  let chainsEmptyContent: ReactNode;
  if (modelQuery.isPending) {
    chainsEmptyContent = (
      <div className="project-deps-empty" data-testid="project-deps-chains-loading">
        {t('projectDependencies.chain.loading')}
      </div>
    );
  } else if (modelQuery.isError && !modelData) {
    chainsEmptyContent = (
      <div className="project-deps-empty" data-testid="project-deps-chains-failed">
        {t('projectDependencies.chain.readFailed')}
      </div>
    );
  } else if (!derived.chainsComplete) {
    chainsEmptyContent = (
      <div className="project-deps-empty" data-testid="project-deps-chains-partial">
        {missingModelParts.length > 0
          ? t('projectDependencies.chain.partialMissing', { parts: partNames(missingModelParts, t) })
          : t('projectDependencies.chain.partialUnknown')}
      </div>
    );
  } else {
    chainsEmptyContent = (
      <div className="project-deps-calm" data-testid="project-deps-calm">
        <span className="project-deps-calm-icon" aria-hidden="true">
          <CircleCheckIcon size={20} strokeWidth={1.5} />
        </span>
        <div>
          <strong>{t('projectDependencies.chain.calmTitle')}</strong>
          <p>{t('projectDependencies.chain.calmDescription')}</p>
        </div>
      </div>
    );
  }

  // Independent read state for the bottom-left versions table block.
  const depsListIncompleteEmpty = Boolean(
    dependenciesData && dependencyRows.length === 0 && (dependenciesData.total > 0 || dependenciesData.next_cursor),
  );
  let versionsBody: ReactNode;
  if (dependencies.isPending) {
    versionsBody = (
      <div className="project-deps-empty" data-testid="project-deps-table-loading">
        {t('projectDependencies.table.loading')}
      </div>
    );
  } else if (dependencies.isError && !dependenciesData) {
    versionsBody = (
      <div className="project-deps-empty" data-testid="project-deps-table-failed">
        {t('projectDependencies.table.readFailed')}
      </div>
    );
  } else if (dependencyRows.length > 0) {
    versionsBody = (
      <DataTable
        columns={[
          { key: 'consumer', title: t('projectDependencies.table.columns.consumer'), width: '24%' },
          { key: 'provider', title: t('projectDependencies.table.columns.provider') },
          { key: 'adopted', title: t('projectDependencies.table.columns.adopted'), width: 96 },
          { key: 'available', title: t('projectDependencies.table.columns.available'), width: 132, hideOnSmall: true },
          { key: 'status', title: t('projectDependencies.table.columns.status'), width: 136 },
        ]}
        rows={dependencyRows}
        navigationWidth={36}
        stickyNavigation={false}
      />
    );
  } else if (depsListIncompleteEmpty) {
    versionsBody = (
      <div className="project-deps-empty" data-testid="project-deps-table-partial">
        {t('projectDependencies.table.partialEmpty')}
      </div>
    );
  } else {
    versionsBody = (
      <div className="project-deps-empty" data-testid="project-deps-table-empty">
        {t('projectDependencies.table.empty')}
      </div>
    );
  }

  // Independent read state for the bottom-right blockers block.
  let blockersBody: ReactNode;
  if (modelQuery.isPending) {
    blockersBody = (
      <div className="project-deps-empty" data-testid="project-deps-blockers-loading">
        {t('projectDependencies.blockers.loading')}
      </div>
    );
  } else if (modelQuery.isError && !modelData) {
    blockersBody = (
      <div className="project-deps-empty" data-testid="project-deps-blockers-failed">
        {t('projectDependencies.blockers.readFailed')}
      </div>
    );
  } else if (derived.blockers.length === 0) {
    blockersBody = derived.blockersComplete ? (
      <div className="project-deps-empty" data-testid="project-deps-blockers-empty">
        {t('projectDependencies.blockers.empty')}
      </div>
    ) : (
      <div className="project-deps-empty" data-testid="project-deps-blockers-partial">
        {missingModelParts.length > 0
          ? t('projectDependencies.blockers.partialMissing', { parts: partNames(missingModelParts, t) })
          : t('projectDependencies.blockers.unread')}
      </div>
    );
  } else {
    blockersBody = (
      <>
        <ul className="project-deps-blocker-list" data-testid="project-deps-blocker-list">
          {derived.blockers.map((blocker) => {
            const title = blockerTitle(blocker, t, language);
            const meta = blockerMeta(blocker, t, language);
            const openDestination = () => {
              if (!blocker.destination) return;
              if (blocker.destination.kind === 'person') openPerson(blocker.destination.id);
              else openTask(blocker.destination.id);
            };
            const mainInner = (
              <>
                <StatusLamp lamp={blocker.lamp} />
                <div className="project-deps-blocker-body">
                  <div className="project-deps-blocker-title">{title}</div>
                  <div className="project-deps-blocker-meta">{meta}</div>
                </div>
              </>
            );
            return (
              <li key={blocker.key} className="project-deps-blocker-item" data-testid="project-deps-blocker-row">
                <div className="project-deps-blocker-row">
                  {blocker.destination ? (
                    <button
                      type="button"
                      className="project-deps-blocker-main"
                      data-testid="project-deps-blocker-main"
                      onClick={openDestination}
                    >
                      {mainInner}
                    </button>
                  ) : (
                    <div className="project-deps-blocker-main" data-static="true">
                      {mainInner}
                    </div>
                  )}
                  {blocker.behind.count !== null ? (
                    <button
                      type="button"
                      className="project-deps-blocker-behind"
                      data-testid="project-deps-blocker-behind"
                      aria-label={t('projectDependencies.blockers.behindAria', { count: blocker.behind.count })}
                      onClick={() => setDetail({ kind: 'blocker', key: blocker.key })}
                    >
                      {t('projectDependencies.blockers.behindCount', { count: blocker.behind.count })}
                    </button>
                  ) : (
                    <span className="project-deps-blocker-behind-static">
                      {t('projectDependencies.blockers.behindUnknown')}
                    </span>
                  )}
                </div>
              </li>
            );
          })}
        </ul>
        {!derived.blockersComplete && missingModelParts.length > 0 ? (
          <div className="project-deps-pnote" data-testid="project-deps-blockers-partial-note">
            {t('projectDependencies.blockers.partialConfirmedNote', { parts: partNames(missingModelParts, t) })}
          </div>
        ) : null}
      </>
    );
  }

  const anyPending = modelQuery.isPending || dependencies.isPending;
  const pageState = partialFailure
    ? 'partial'
    : anyPending
      ? 'loading'
      : showUnknownNotice
        ? 'unknown'
        : derived.chainsComplete
          && derived.blockersComplete
          && derived.chains.length === 0
          && derived.rows.length === 0
          && derived.blockers.length === 0
          && !depsListIncompleteEmpty
          ? 'empty'
          : 'data';

  const depsSectionCount = dependencies.isPending || dependencies.isError
    ? undefined
    : derived.dependenciesTotal !== null
      ? derived.dependenciesTotal
      : undefined;

  const blockersSectionCount = !modelQuery.isPending && !modelQuery.isError && derived.blockersCount !== null && derived.blockersCount > 0
    ? derived.blockersCount
    : undefined;

  return (
    <div className="project-deps" data-testid="page-project-dependencies" data-view="overview" data-state={pageState}>
      {partialFailure ? (
        <Notice
          tone="warning"
          title={t('projectDependencies.states.partialTitle')}
          extra={(modelQuery.isError || dependencies.isError || overview.isError) ? (
            <Button size="small" onClick={retryFailed}>{t('projectDependencies.states.retry')}</Button>
          ) : undefined}
          className="project-deps-notice"
        >
          <span data-testid="project-deps-partial">
            {missingReadLabels.length > 0
              ? t('projectDependencies.states.partialMissing', { missing: missingReadLabels.join(t('common.listSeparator')) })
              : t('projectDependencies.states.partialDescription')}
          </span>
        </Notice>
      ) : null}

      {showUnknownNotice ? (
        <Notice
          tone="info"
          title={t('projectDependencies.states.unknownTitle')}
          className="project-deps-notice"
        >
          <span data-testid="project-deps-unknown">{t('projectDependencies.states.unknownDescription')}</span>
        </Notice>
      ) : null}

      <section className="project-deps-inst" data-testid="project-deps-chains">
        <div className="project-deps-inst-body">
          <Chain
            nodes={activeChainEntry?.displayNodes ?? []}
            connectors={activeChainEntry?.displayConnectors}
            header={(
              <div className="project-deps-chain-head" data-testid="project-deps-chain-head">
                <h2>{t('projectDependencies.chain.title')}</h2>
                {derived.chains.length > 0 ? (
                  <button
                    type="button"
                    className="obs-tag project-deps-chain-tag"
                    data-testid="project-deps-chain-count"
                    onClick={() => setDetail({ kind: 'chains' })}
                  >
                    {t('projectDependencies.chain.blockedCount', { count: derived.chains.length })}
                  </button>
                ) : null}
                {derived.chains.length > 0 ? (
                  <div className="project-deps-chain-switch" role="group" aria-label={t('projectDependencies.chain.groupLabel')}>
                    {derived.chains.map((entry, index) => (
                      <button
                        key={entry.chain.id}
                        type="button"
                        aria-pressed={index === activeChainIndex}
                        onClick={() => {
                          setDetail(null);
                          updateParams({ chain: String(index) });
                        }}
                      >
                        {chainTabLabel(entry, reviewChainCount, t, language)}
                      </button>
                    ))}
                  </div>
                ) : null}
              </div>
            )}
            empty={chainsEmptyContent}
            present={(node) => presentChainNode(activeChainEntry!, node)}
            onOpen={(node) => openChainNode(activeChainEntry!, node)}
            footer={activeChainEntry ? renderChainFooter(activeChainEntry) : undefined}
          />
          {derived.chains.length > 0 && !derived.chainsComplete && missingModelParts.length > 0 ? (
            <div className="project-deps-pnote" data-testid="project-deps-chains-partial-note">
              {t('projectDependencies.chain.partialConfirmedNote', { parts: partNames(missingModelParts, t) })}
            </div>
          ) : null}
        </div>
      </section>

      <div className="project-deps-cols" data-testid="project-deps-cols">
        <div data-testid="project-deps-versions">
          <Section
            title={t('projectDependencies.table.title')}
            count={depsSectionCount}
            countTestId="project-deps-versions-count"
            className="project-deps-section"
          >
            {versionsBody}
            {(!dependencies.isPending && !dependencies.isError && (cursors.length > 1 || derived.dependenciesTruncated)) ? (
              <div className="project-deps-pagination" data-testid="project-deps-pagination">
                <span>
                  {t('projectDependencies.table.paginationNote', {
                    shown: dependencyRows.length,
                    total: derived.dependenciesTotal ?? dependencyRows.length,
                  })}
                </span>
                <span className="project-deps-pagination-actions">
                  {cursors.length > 1 ? (
                    <Button size="small" onClick={() => setCursors(cursors.slice(0, -1))}>
                      {t('plan.observation.previous')}
                    </Button>
                  ) : null}
                  {dependenciesData?.next_cursor ? (
                    <Button size="small" onClick={() => setCursors([...cursors, dependenciesData.next_cursor])}>
                      {t('plan.observation.next')}
                    </Button>
                  ) : null}
                </span>
              </div>
            ) : null}
            <div className="project-deps-pnote" data-testid="project-deps-pnote">
              {t('projectDependencies.table.note')}
            </div>
          </Section>
        </div>

        <div data-testid="project-deps-blockers">
          <Section
            title={t('projectDependencies.blockers.title')}
            count={blockersSectionCount}
            countTestId="project-deps-blockers-count"
            className="project-deps-section"
          >
            {blockersBody}
          </Section>
        </div>
      </div>

      {detailPanel}

      {viewsBar('bottom')}

      <footer className="project-deps-footer" data-testid="project-deps-coverage">
        <div>
          {t('projectDependencies.coverage.title')}
          {' · '}
          {dependencies.isPending
            ? t('projectDependencies.coverage.depsLoading')
            : dependenciesData
              ? t('projectDependencies.coverage.depsRead', {
                shown: dependencyRows.length,
                total: dependenciesData.total,
              })
              : t('projectDependencies.coverage.depsUnread')}
        </div>
        {derived.coverage.modelPartialParts.length > 0 ? (
          <div data-testid="project-deps-coverage-partial">
            {t('projectDependencies.coverage.modelPartial', {
              parts: partNames(derived.coverage.modelPartialParts, t),
            })}
          </div>
        ) : null}
        {derived.coverage.modelUnreadParts.length > 0 ? (
          <div data-testid="project-deps-coverage-unread">
            {t('projectDependencies.coverage.modelUnread', {
              parts: partNames(derived.coverage.modelUnreadParts, t),
            })}
          </div>
        ) : null}
        {derived.dependenciesTruncated ? (
          <div data-testid="project-deps-coverage-truncated">
            {t('projectDependencies.coverage.truncated')}
          </div>
        ) : null}
        <FreshnessBadge freshness={mergedFreshness} />
      </footer>
    </div>
  );
}
