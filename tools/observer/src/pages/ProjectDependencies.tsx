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
  mergeDependenciesFreshness,
  type DependencyAdoptedCell,
  type DependencyAvailableCell,
  type DependencyRowStatus,
  type ProjectBlockerItem,
  type ProjectChainDisplay,
} from '../components/projects/project-dependencies-model';
import type { ProjectListEntry, ProjectListModel } from '../components/projects/project-list-model';
import { ObservationForbidden } from '../components/states';
import { workbenchAgentName, workbenchDisplayText } from '../components/workbench/model';
import { readsAsName, workLabel } from '../core/observation';
import { hasObservationValue } from '../core/observation/values';
import type { ChainNode, DerivedTask, OverviewPart } from '../core/overview';
import { humanText } from '../i18n/human-language';
import { apiPost, isAuthError } from '../lib/api';
import { dependenciesQueryOptions } from '../lib/collaboration';
import { projectLabel } from '../lib/project-label';
import { overviewModelQueryOptions, overviewQueryOptions, projectsQueryOptions, readQueryDefaults } from '../lib/queries';
import { useRevocation } from '../lib/revocation';
import { taskPath } from '../lib/task-path';
import { useCompactViewport } from '../lib/viewport';
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

function asView(value: string | null): DependenciesView {
  return (VIEW_KEYS as readonly string[]).includes(value ?? '') ? (value as DependenciesView) : 'overview';
}

function validChainIndex(value: string | null, count: number): number {
  const parsed = Number(value ?? 0);
  return Number.isInteger(parsed) && parsed >= 0 && parsed < count ? parsed : 0;
}

function safeTitle(raw: string | null | undefined, language: string, fallback: string): string {
  if (!raw) return fallback;
  return workLabel(raw, { language, recordedTitle: true, fallback }).text;
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

function isExternalParty(name: string): boolean {
  return name === '甲方' || name.endsWith('小组') || name.endsWith('改造') || name.endsWith('部');
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

function formatAdoptedCell(cell: DependencyAdoptedCell, t: TFunction): string {
  if (cell.kind === 'version') return t('projectDependencies.table.versionNumber', { version: cell.version });
  if (cell.kind === 'restricted') return t('projectDependencies.table.restricted');
  if (cell.kind === 'adoptionUnavailable') return t('projectDependencies.table.adoptionUnavailable');
  return t('projectDependencies.table.notYet');
}

function formatAvailableCell(cell: DependencyAvailableCell, t: TFunction): string {
  switch (cell.kind) {
    case 'version': return t('projectDependencies.table.versionNumber', { version: cell.version });
    case 'inReviewVersion': return t('projectDependencies.table.versionInReview', { version: cell.version });
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
    case 'notYet': return t('projectDependencies.table.notYet');
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
    const party = workbenchDisplayText(root.externalParty, '');
    return party
      ? t('projectDependencies.chain.tabExternal', { party })
      : t('projectDependencies.chain.tabExternalUnknown');
  }
  const responder = root.queue.items[0]?.waitingOn[0]?.displayName;
  const name = responder ? safePersonName(responder, language, '') : '';
  return name
    ? t('projectDependencies.chain.tabDecisionNamed', { name })
    : t('projectDependencies.chain.tabDecisionUnknown');
}

function blockerTitle(blocker: ProjectBlockerItem, t: TFunction, language: string): string {
  if (blocker.kind === 'review') {
    return blocker.personName
      ? t('projectDependencies.blockers.reviewQueueTitle', { name: blocker.personName })
      : t('projectDependencies.blockers.reviewQueueUnknownTitle');
  }
  if (blocker.kind === 'external') {
    const party = workbenchDisplayText(blocker.externalParty, '');
    const what = workbenchDisplayText(blocker.what, '');
    if (party && what) return t('projectDependencies.blockers.externalTitle', { party, what });
    if (party) return t('projectDependencies.blockers.externalFallbackTitle', { party });
    return t('projectDependencies.blockers.externalUnknownTitle');
  }
  if (blocker.personName) {
    return blocker.isOwnTaskAnswer
      ? t('projectDependencies.blockers.decisionAnswerTitle', { name: blocker.personName })
      : t('projectDependencies.blockers.decisionDecideTitle', { name: blocker.personName });
  }
  void language;
  return t('projectDependencies.blockers.decisionUnknownTitle');
}

function blockerMeta(blocker: ProjectBlockerItem, t: TFunction): string {
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
  const what = workbenchDisplayText(blocker.what?.replace(/^(?:回答|决定)[：:]/, ''), t('projectDependencies.blockers.decisionMetaFallback'));
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
        ? workbenchAgentName(task.agentName, t('projectDependencies.handoffs.noAgent'))
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
  const compactViewport = useCompactViewport();
  const [searchParams, setSearchParams] = useSearchParams();
  const [cursors, setCursors] = useState<Array<string | null>>([null]);
  const [detail, setDetail] = useState<DetailSelection | null>(null);

  const activeView = asView(searchParams.get('view'));

  const projects = useQuery(projectsQueryOptions(revoked));
  const overview = useQuery(overviewQueryOptions(project, revoked));
  const modelQuery = useQuery(overviewModelQueryOptions(project || null, revoked));
  const dependencies = useQuery(dependenciesQueryOptions(project, null, cursors.at(-1) ?? null, revoked));

  const projectEntries = projects.data?.projects as ProjectListEntry[] | undefined;
  const projectIndex = projectEntries ? projectEntries.findIndex((item) => item.key === project) : -1;
  const projectEntry = projectIndex >= 0 && projectEntries ? projectEntries[projectIndex] : undefined;
  const projectTitle = projectEntry ? projectLabel(projectEntry, projectIndex + 1) : t('projectDependencies.chain.commitmentProjectKicker');

  const modelData = modelQuery.data?.project === project && modelQuery.data.input && modelQuery.data.input.options
    ? (modelQuery.data as ProjectListModel)
    : undefined;
  const dependenciesData = dependencies.data?.result.data;

  const derived = useMemo(() => deriveProjectDependencies({
    projectKey: project,
    overview: overview.data,
    model: modelData,
    dependencies: dependenciesData,
  }), [project, overview.data, modelData, dependenciesData]);

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
  const partialFailure = !primaryLoading && !primaryFailed && (
    modelQuery.isError
    || dependencies.isError
    || derived.coverage.modelPartialParts.length > 0
    || Boolean(overview.data && overview.data.coverage.failed > 0)
  );
  const showUnknownNotice = !primaryLoading && !primaryFailed && !partialFailure
    && (derived.coverage.orderingIncomparable || derived.coverage.hasUnknownFacts);

  const mergedFreshness = mergeDependenciesFreshness(
    dependencies.data?.freshness,
    modelData?.freshness,
    overview.data?.freshness,
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
        const party = workbenchDisplayText(root.externalParty, '');
        const what = workbenchDisplayText(firstWait?.what, '');
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
          ownerExternal: party ? isExternalParty(party) : true,
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
      return {
        kicker: t('display.chain.rootKicker'),
        title,
        meta: workbenchDisplayText(firstWait?.what?.replace(/^(?:回答|决定)[：:]/, ''), ''),
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

    const commitment = node.commitment;
    const isWeek = Boolean(commitment?.id?.includes('-week-'));
    const isGoal = commitment?.kind === 'goal';
    const kicker = isWeek
      ? t('projectDependencies.chain.commitmentWeekKicker')
      : isGoal
        ? t('projectDependencies.chain.commitmentGoalKicker')
        : t('projectDependencies.chain.commitmentProjectKicker');
    const title = isGoal
      ? t('workbench.home.commitmentGoalFallback')
      : projectTitle;
    const progress = entry.commitmentProgress;
    const remaining = progress ? Math.max(0, progress.total - progress.done) : null;
    return {
      kicker,
      title,
      meta: remaining !== null && remaining > 0
        ? t('projectDependencies.chain.commitmentRemainingRisk', { remaining })
        : null,
      health: remaining !== null && remaining > 0 ? 'risk' : 'none',
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
        : name
          ? t('projectDependencies.chain.footerReview', { name, count: impactCount })
          : t('projectDependencies.chain.footerReviewUnknownPerson', { count: impactCount });
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
      ? workbenchDisplayText(row.requirement.text, t('projectDependencies.table.requirementReviewed'))
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

  const pageState = partialFailure
    ? 'partial'
    : showUnknownNotice
      ? 'unknown'
      : derived.chains.length === 0 && derived.rows.length === 0 && derived.blockers.length === 0
        ? 'empty'
        : 'data';

  const depsSectionCount = dependencies.isError
    ? undefined
    : derived.dependenciesTotal !== null
      ? derived.dependenciesTotal
      : undefined;

  const blockersSectionCount = derived.blockersCount !== null && derived.blockersCount > 0
    ? derived.blockersCount
    : derived.blockers.length > 0 && derived.blockersComplete
      ? derived.blockers.length
      : undefined;

  return (
    <div className="project-deps" data-testid="page-project-dependencies" data-view="overview" data-state={pageState}>
      {partialFailure ? (
        <Notice
          tone="warning"
          title={t('projectDependencies.states.partialTitle')}
          extra={(modelQuery.isError || dependencies.isError) ? (
            <Button size="small" onClick={retryFailed}>{t('projectDependencies.states.retry')}</Button>
          ) : undefined}
          className="project-deps-notice"
        >
          <span data-testid="project-deps-partial">{t('projectDependencies.states.partialDescription')}</span>
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
            empty={(
              <div className="project-deps-calm" data-testid="project-deps-calm">
                <span className="project-deps-calm-icon" aria-hidden="true">
                  <CircleCheckIcon size={20} strokeWidth={1.5} />
                </span>
                <div>
                  <strong>{t('projectDependencies.chain.calmTitle')}</strong>
                  <p>{t('projectDependencies.chain.calmDescription')}</p>
                </div>
              </div>
            )}
            present={(node) => presentChainNode(activeChainEntry!, node)}
            onOpen={(node) => openChainNode(activeChainEntry!, node)}
            footer={activeChainEntry ? renderChainFooter(activeChainEntry) : undefined}
          />
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
            {dependencies.isError ? (
              <div className="project-deps-empty" data-testid="project-deps-table-failed">
                {t('projectDependencies.table.readFailed')}
              </div>
            ) : dependencyRows.length > 0 ? (
              <DataTable
                columns={[
                  { key: 'consumer', title: t('projectDependencies.table.columns.consumer'), width: compactViewport ? '22%' : '24%' },
                  { key: 'provider', title: t('projectDependencies.table.columns.provider') },
                  { key: 'adopted', title: t('projectDependencies.table.columns.adopted'), width: compactViewport ? 56 : 96 },
                  { key: 'available', title: t('projectDependencies.table.columns.available'), width: 132, hideOnSmall: true },
                  { key: 'status', title: t('projectDependencies.table.columns.status'), width: compactViewport ? 108 : 136 },
                ]}
                rows={dependencyRows}
                navigationWidth={compactViewport ? 22 : 36}
                stickyNavigation={false}
              />
            ) : (
              <div className="project-deps-empty" data-testid="project-deps-table-empty">
                {t('projectDependencies.table.empty')}
              </div>
            )}
            {(!dependencies.isError && (cursors.length > 1 || derived.dependenciesTruncated)) ? (
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
            {modelQuery.isError && derived.blockers.length === 0 ? (
              <div className="project-deps-empty" data-testid="project-deps-blockers-unread">
                {t('projectDependencies.blockers.unread')}
              </div>
            ) : derived.blockers.length === 0 ? (
              <div className="project-deps-empty" data-testid="project-deps-blockers-empty">
                {derived.blockersComplete
                  ? t('projectDependencies.blockers.empty')
                  : t('projectDependencies.blockers.unread')}
              </div>
            ) : (
              <ul className="project-deps-blocker-list" data-testid="project-deps-blocker-list">
                {derived.blockers.map((blocker) => {
                  const title = blockerTitle(blocker, t, language);
                  const meta = blockerMeta(blocker, t);
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
            )}
          </Section>
        </div>
      </div>

      {detailPanel}

      {viewsBar('bottom')}

      <footer className="project-deps-footer" data-testid="project-deps-coverage">
        <div>
          {t('projectDependencies.coverage.title')}
          {' · '}
          {dependenciesData
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
