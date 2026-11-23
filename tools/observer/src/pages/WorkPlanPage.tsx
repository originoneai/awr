import { useQuery } from '@tanstack/react-query';
import { Button, Tag } from 'antd';
import type { TFunction } from 'i18next';
import { useMemo, type ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import { useNavigate, useSearchParams } from 'react-router';
import { FreshnessBadge } from '../components/FreshnessBadge';
import { PlanningChanges, SourceReadiness } from '../components/PlanObservations';
import { CompletionBar, CompletionLegend } from '../components/display/CompletionBar';
import { DataTable, type DataTableRow } from '../components/display/DataTable';
import { Notice } from '../components/display/Notice';
import { Section } from '../components/display/Section';
import { TaskStatusTag } from '../components/display/StatusTag';
import { SplitIcon } from '../components/icons/prototype-icons';
import type { ProjectListModel } from '../components/projects/project-list-model';
import { mergeProjectFreshness } from '../components/projects/project-overview-model';
import {
  asWorkPlanFilter,
  deriveWorkPlan,
  projectGoalsQueryOptions,
  WORK_PLAN_FILTERS,
  type WorkPlanFilterId,
  type WorkPlanPendingBreakdown,
} from '../components/projects/work-plan-model';
import { ObservationForbidden } from '../components/states';
import {
  compactTimeText,
  groupWorks,
  rawObservationText,
  readableNarrative,
  workLabel,
} from '../core/observation';
import type { OverviewPart } from '../core/overview';
import { humanText } from '../i18n/human-language';
import { isAuthError } from '../lib/api';
import { readClockOffsetUnixMs, referenceNowUnixMs, useDisplayTimeZone } from '../lib/display-clock';
import { formatUnixMs } from '../lib/format';
import { freshnessWord } from '../lib/freshness';
import {
  overviewModelQueryOptions,
  overviewQueryOptions,
  planningCandidatesQueryOptions,
} from '../lib/queries';
import { useRevocation } from '../lib/revocation';
import { taskPath } from '../lib/task-path';
import type { TeamFreshness } from '../lib/types';
import './WorkPlanPage.css';

const FAILED_FRESHNESS: TeamFreshness = {
  report_received: { status: 'failed' },
  authoritative: { status: 'failed' },
  page_read: { status: 'failed' },
};

function partNames(parts: readonly OverviewPart[], t: TFunction): string {
  return parts.map((part) => t(`workbench.home.coverageParts.${part}`)).join(t('common.listSeparator'));
}

function formatPendingBreakdownDetail(
  breakdown: WorkPlanPendingBreakdown,
  t: TFunction,
  language: string,
  nowUnixMs: number,
  timeZone: string,
): string {
  const goal = breakdown.goalTitle
    ? readableNarrative(breakdown.goalTitle, language) ?? t('workPlan.pendingBreakdown.unnamedGoal')
    : null;
  const count = breakdown.taskCount;
  const author = breakdown.authorName;
  const when = breakdown.proposedAtUnixMs !== null
    ? compactTimeText(breakdown.proposedAtUnixMs, { language, nowUnixMs, timeZone })
    : null;

  if (goal && count !== null && author && when) {
    return t('workPlan.pendingBreakdown.full', { goal, count, author, when });
  }
  if (goal && count !== null && author) {
    return t('workPlan.pendingBreakdown.goalCountAuthor', { goal, count, author });
  }
  if (goal && count !== null && when) {
    return t('workPlan.pendingBreakdown.goalCountWhen', { goal, count, when });
  }
  if (goal && count !== null) {
    return t('workPlan.pendingBreakdown.goalCount', { goal, count });
  }
  if (count !== null && author && when) {
    return t('workPlan.pendingBreakdown.countAuthorWhen', { count, author, when });
  }
  if (count !== null) {
    return t('workPlan.pendingBreakdown.countOnly', { count });
  }
  if (goal) {
    return t('workPlan.pendingBreakdown.goalOnly', { goal });
  }
  return t('workPlan.pendingBreakdown.fallback');
}

/** Prototype pPlan(): pending-breakdown callout, filter bar with goal link & completion legend,
 * grouped-by-workstream task table, preserved plan changes & document readiness, and quiet coverage footer. */
export function WorkPlanPage({ project }: { project: string }) {
  const { t, i18n } = useTranslation();
  const language = i18n.language;
  const navigate = useNavigate();
  const { revoked } = useRevocation();
  const [searchParams, setSearchParams] = useSearchParams();
  const displayTimeZone = useDisplayTimeZone();

  const streamParam = searchParams.get('stream');
  const activeFilter = asWorkPlanFilter(searchParams.get('f'));

  const overview = useQuery(overviewQueryOptions(project, revoked));
  const modelQuery = useQuery(overviewModelQueryOptions(project || null, revoked));
  const planningQuery = useQuery(planningCandidatesQueryOptions(project, null, null, revoked));
  const goalsQuery = useQuery(projectGoalsQueryOptions(project, revoked));

  const modelData = modelQuery.data?.project === project && modelQuery.data.input && modelQuery.data.input.options
    ? (modelQuery.data as ProjectListModel)
    : undefined;
  const nowUnixMs = referenceNowUnixMs(readClockOffsetUnixMs(modelData));

  const derived = useMemo(() => deriveWorkPlan({
    projectKey: project,
    overview: overview.data,
    model: modelData,
    planningCandidates: planningQuery.data?.result.data,
    goals: goalsQuery.data?.result.data?.items,
    streamFilter: streamParam,
    activeFilter,
    referenceUnixMs: nowUnixMs,
  }), [project, overview.data, modelData, planningQuery.data, goalsQuery.data, streamParam, activeFilter, nowUnixMs]);

  if (
    revoked
    || isAuthError(overview.error)
    || isAuthError(modelQuery.error)
    || isAuthError(planningQuery.error)
  ) {
    return <ObservationForbidden />;
  }

  const primaryLoading = overview.isPending && modelQuery.isPending;
  const primaryFailed = !primaryLoading
    && overview.isError && !overview.data
    && modelQuery.isError && !modelData;
  const partialFailure = !primaryLoading && !primaryFailed && (
    overview.isError
    || modelQuery.isError
    || derived.coverage.overviewScopeFailed
    || derived.coverage.modelPartialParts.length > 0
  );
  const isEmpty = !primaryLoading && !primaryFailed && !partialFailure
    && !derived.coverage.orderingIncomparable
    && derived.allTasks.length === 0
    && derived.groups.length === 0
    && derived.activeStreamId === null;
  const showUnknownNotice = !primaryLoading && !primaryFailed && !partialFailure && !isEmpty
    && (
      derived.coverage.orderingIncomparable
      || derived.coverage.hasUnknownStatus
      || derived.coverage.modelUnreadParts.length > 0
      || derived.coverage.hasUnknownFacts
    );

  const scopeQuery = `scope=${encodeURIComponent(project)}`;

  function setFilter(nextFilter: WorkPlanFilterId) {
    const next = new URLSearchParams(searchParams);
    if (nextFilter === 'all') next.delete('f');
    else next.set('f', nextFilter);
    setSearchParams(next);
  }

  function focusWorkstream(streamId: string | null) {
    const next = new URLSearchParams(searchParams);
    if (streamId) next.set('stream', streamId);
    else next.delete('stream');
    next.delete('f');
    setSearchParams(next);
  }

  function retryFailed() {
    if (overview.isError) void overview.refetch();
    if (modelQuery.isError) void modelQuery.refetch();
    if (planningQuery.isError) void planningQuery.refetch();
  }

  if (primaryLoading) {
    return (
      <div className="work-plan-page" data-testid="work-plan-page">
        <div className="work-plan-state" data-testid="work-plan-loading">
          {t('workPlan.loading')}
        </div>
      </div>
    );
  }

  if (primaryFailed) {
    return (
      <div className="work-plan-page" data-testid="work-plan-page">
        <Notice
          tone="warning"
          title={t('workPlan.failedTitle')}
          extra={<Button size="small" onClick={retryFailed}>{t('workPlan.retry')}</Button>}
          className="work-plan-notice"
        >
          <span data-testid="work-plan-failed">{t('workPlan.failedDescription')}</span>
        </Notice>
        <footer className="work-plan-coverage-footer" data-testid="work-plan-coverage">
          <FreshnessBadge freshness={FAILED_FRESHNESS} />
        </footer>
      </div>
    );
  }

  // Coverage & freshness footer facts (matching ProjectOverview's disclosure contract)
  const overviewCoverage = overview.data?.coverage;
  const failedStreams = overviewCoverage?.streams.filter((stream) => stream.status === 'failed') ?? [];
  const overviewNames = overview.data ? groupWorks(overview.data.works, overview.data.workstreams).titles : {};
  const cursors = overviewCoverage
    ? [overviewCoverage.scope, ...overviewCoverage.streams]
      .map((part) => rawObservationText(part.next_cursor).text)
      .filter((cursor): cursor is string => cursor !== null)
    : [];
  const truncated = Boolean(
    overviewCoverage && (
      overviewCoverage.truncated
      || overviewCoverage.scope.truncated
      || overviewCoverage.streams.some((stream) => stream.truncated)
      || cursors.length > 0
    ),
  );
  const freshness = mergeProjectFreshness(overview.data?.freshness, modelData?.freshness);

  // Pending breakdown callout
  const pendingBreakdown = derived.pendingBreakdown;
  const inboxTarget = pendingBreakdown?.inboxDetailKey
    ? `/inbox?${scopeQuery}&detail=${encodeURIComponent(pendingBreakdown.inboxDetailKey)}`
    : `/inbox?${scopeQuery}`;

  // Grouped-by-workstream body
  let groupedBody: ReactNode;
  if (isEmpty) {
    groupedBody = (
      <div className="work-plan-empty" data-testid="work-plan-empty">
        {t('workPlan.emptyDescription')}
      </div>
    );
  } else if (derived.visibleGroups.length === 0) {
    groupedBody = (
      <div className="work-plan-empty" data-testid="work-plan-filter-empty">
        {t('workPlan.filterEmpty')}
      </div>
    );
  } else {
    groupedBody = derived.visibleGroups.map((group) => {
      const groupTitle = group.id === '__ungrouped__'
        ? t('workPlan.unnamedWorkstream')
        : workLabel(group.title ?? group.id, {
          language,
          recordedTitle: Boolean(group.title),
          fallback: humanText('workstreamUnnamed', language),
        }).text;

      const rows: DataTableRow[] = group.visibleTasks.map((task) => {
        const titleText = task.title
          ? workLabel(task.title, { language, recordedTitle: true }).text
          : t('workPlan.taskUnnamed');
        const nextText = task.nextStep
          ? readableNarrative(task.nextStep, language) ?? t('workPlan.nextStepUnavailable')
          : null;

        return {
          key: task.id,
          testId: `work-plan-row-${task.id}`,
          highlighted: task.highlighted,
          onOpen: () => navigate(taskPath(project, task.id)),
          cells: [
            <span className="work-plan-task-title">{titleText}</span>,
            <span data-testid={`plan-status-${task.id}`}>
              <TaskStatusTag status={task.status} live={task.agentActive === true} />
            </span>,
            task.ownerName ? (
              <div className="work-plan-owner" data-testid={`plan-owner-${task.id}`}>
                <span className="work-plan-avatar" aria-hidden="true">{task.ownerName.slice(0, 1)}</span>
                {task.ownerName}
              </div>
            ) : (
              <span className="work-plan-dim" data-testid={`plan-owner-${task.id}`}>
                {t('common.notProvided')}
              </span>
            ),
            nextText ? (
              <span className="work-plan-next" data-testid={`plan-next-${task.id}`}>{nextText}</span>
            ) : (
              <span className="work-plan-dim" data-testid={`plan-next-${task.id}`}>—</span>
            ),
          ],
        };
      });

      return (
        <div
          key={group.id}
          className="work-plan-group"
          data-testid={`workstream-group-${group.id}`}
          data-total-count={group.totalCount ?? undefined}
          data-visible-count={group.visibleTasks.length}
        >
          <div className="work-plan-group-head">
            <b className="work-plan-group-name">{groupTitle}</b>
            {group.id !== '__ungrouped__' ? (
              <button
                type="button"
                className="work-plan-group-count"
                data-testid={`workstream-count-${group.id}`}
                aria-label={t('workPlan.focusWorkstreamLabel', { name: groupTitle })}
                onClick={() => focusWorkstream(derived.activeStreamId === group.id ? null : group.id)}
              >
                {group.totalCount !== null
                  ? t('workPlan.itemCount', { count: group.totalCount })
                  : t('workPlan.itemCountUnknown')}
              </button>
            ) : (
              <span className="work-plan-group-count">
                {group.totalCount !== null
                  ? t('workPlan.itemCount', { count: group.totalCount })
                  : t('workPlan.itemCountUnknown')}
              </span>
            )}
            {group.counts !== null ? (
              <CompletionBar counts={group.counts} showSummary={false} />
            ) : (
              <span className="work-plan-group-unknown" data-testid={`workstream-bar-unknown-${group.id}`}>
                {t('workPlan.completionUnknown')}
              </span>
            )}
          </div>
          <div className="work-plan-table">
            <DataTable
              columns={[
                { key: 'task', width: '34%' },
                { key: 'status', width: 156 },
                { key: 'owner', width: 128, hideOnSmall: true },
                { key: 'next', hideOnSmall: true },
              ]}
              rows={rows}
              navigationWidth={36}
              stickyNavigation={false}
            />
          </div>
        </div>
      );
    });
  }

  return (
    <div className="work-plan-page" data-testid="work-plan-page">
      {partialFailure ? (
        <Notice
          tone="warning"
          title={t('workPlan.partialTitle')}
          extra={overview.isError || modelQuery.isError ? (
            <Button size="small" onClick={retryFailed}>{t('workPlan.retry')}</Button>
          ) : undefined}
          className="work-plan-notice"
        >
          <span data-testid="work-plan-partial">{t('workPlan.partialDescription')}</span>
        </Notice>
      ) : null}

      {showUnknownNotice ? (
        <Notice
          tone="info"
          title={t('workPlan.unknownTitle')}
          className="work-plan-notice"
        >
          <span data-testid="work-plan-unknown">{t('workPlan.unknownDescription')}</span>
        </Notice>
      ) : null}

      {pendingBreakdown ? (
        <div className="work-plan-callout" data-testid="work-plan-pending-callout" role="status">
          <SplitIcon size={18} className="work-plan-callout-icon" aria-hidden="true" />
          <div className="work-plan-callout-body">
            <b className="work-plan-callout-lead">{t('workPlan.pendingBreakdown.lead')}</b>
            {formatPendingBreakdownDetail(pendingBreakdown, t, language, nowUnixMs, displayTimeZone)}
          </div>
          <button
            type="button"
            className="work-plan-callout-btn"
            data-testid="work-plan-pending-open"
            onClick={() => navigate(inboxTarget)}
          >
            {t('workPlan.pendingBreakdown.open')}
          </button>
        </div>
      ) : null}

      <div className="work-plan-filters" data-testid="work-plan-filters">
        <div className="work-plan-segment" role="group" aria-label={t('workPlan.filterLabel')}>
          {WORK_PLAN_FILTERS.map((filterId) => {
            const detail = derived.filters[filterId];
            const countText = detail.count === null ? '—' : String(detail.count);
            return (
              <button
                key={filterId}
                type="button"
                className="work-plan-segment-option"
                data-testid={`work-plan-filter-${filterId}`}
                data-count={detail.count ?? undefined}
                aria-pressed={activeFilter === filterId ? 'true' : 'false'}
                onClick={() => setFilter(filterId)}
              >
                {`${t(`workPlan.filters.${filterId}`)} ${countText}`}
              </button>
            );
          })}
        </div>

        {derived.activeStreamId ? (
          <button
            type="button"
            className="work-plan-stream-chip"
            data-testid="work-plan-stream-reset"
            onClick={() => focusWorkstream(null)}
          >
            {t('workPlan.clearStreamScope', {
              name: workLabel(derived.activeStreamTitle ?? derived.activeStreamId, {
                language,
                recordedTitle: Boolean(derived.activeStreamTitle),
                fallback: humanText('workstreamUnnamed', language),
              }).text,
            })}
          </button>
        ) : null}

        {derived.goal ? (
          <button
            type="button"
            className="work-plan-goal-link"
            data-testid="work-plan-goal-link"
            onClick={() => navigate(`/goals/${encodeURIComponent(derived.goal!.id)}?${scopeQuery}`)}
          >
            {t('workPlan.goalPrefix', { title: derived.goal.title })}
          </button>
        ) : null}

        <CompletionLegend />
      </div>

      <div
        data-testid="plan-progress-table"
        data-filter={activeFilter}
        data-visible-count={derived.totalVisibleTasks}
      >
        <Section
          title={t('workPlan.byWorkstreamTitle')}
          className="work-plan-section"
        >
          {groupedBody}
        </Section>
      </div>

      <div className="work-plan-observations" data-testid="work-plan-observations">
        <PlanningChanges key={`changes:${project}:${streamParam ?? ''}`} project={project} stream={streamParam} />
        <SourceReadiness key={`materials:${project}:${streamParam ?? ''}`} project={project} stream={streamParam} workId={null} />
      </div>

      <footer className="work-plan-coverage-footer" data-testid="work-plan-coverage">
        {freshness ? <FreshnessBadge freshness={freshness} /> : null}
        {overviewCoverage ? (
          <div data-testid="coverage-summary">
            {overviewCoverage.works_total == null
              ? t('projectOverview.coverage.readUnknownTotal', { enumerated: overviewCoverage.works_enumerated })
              : t('projectOverview.coverage.readOfTotal', {
                enumerated: overviewCoverage.works_enumerated,
                total: overviewCoverage.works_total,
                count: overviewCoverage.works_enumerated,
              })}
          </div>
        ) : null}
        {failedStreams.length > 0 ? (
          <div data-testid="coverage-failed-summary">
            {t('projectOverview.coverage.failedSummary', { count: failedStreams.length })}
          </div>
        ) : null}
        {failedStreams.map((stream) => {
          const streamName = stream.workstream_id && overviewNames[stream.workstream_id]
            ? workLabel(overviewNames[stream.workstream_id], {
              language,
              recordedTitle: true,
              fallback: t('projectOverview.coverage.someWorkstream'),
            }).text
            : t('projectOverview.coverage.someWorkstream');
          return (
            <div key={stream.workstream_id} className="work-plan-coverage-stream" data-testid={`coverage-stream-${stream.workstream_id}`}>
              <span>{streamName}</span>
              <Tag color="error">{t('coverage.scope.failed')}</Tag>
              <span>
                {stream.read_at_unix_ms == null
                  ? freshnessWord(t, 'not_measured')
                  : formatUnixMs(stream.read_at_unix_ms, language)}
              </span>
            </div>
          );
        })}
        {truncated ? (
          <div data-testid="work-plan-coverage-unread">
            {t('projectOverview.coverage.unread')}
          </div>
        ) : null}
        {derived.coverage.modelPartialParts.length > 0 ? (
          <div data-testid="work-plan-coverage-model-partial">
            {t('projectOverview.modelCoveragePartial', { parts: partNames(derived.coverage.modelPartialParts, t) })}
          </div>
        ) : null}
        {derived.coverage.modelUnreadParts.length > 0 ? (
          <div data-testid="work-plan-coverage-model-unread">
            {t('projectOverview.modelCoverageUnread', { parts: partNames(derived.coverage.modelUnreadParts, t) })}
          </div>
        ) : null}
      </footer>
    </div>
  );
}
