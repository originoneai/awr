import { useQuery } from '@tanstack/react-query';
import { Button, Tag } from 'antd';
import type { TFunction } from 'i18next';
import { useMemo, useState, type ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import { useNavigate, useSearchParams } from 'react-router';
import { FreshnessBadge } from '../components/FreshnessBadge';
import { CompletionBar, CompletionLegend } from '../components/display/CompletionBar';
import { DataTable, type DataTableRow } from '../components/display/DataTable';
import { DisplayIcon } from '../components/display/DisplayIcon';
import { KpiStrip, type KpiMetric } from '../components/display/KpiStrip';
import { Notice } from '../components/display/Notice';
import { RowList, type RowListItem } from '../components/display/RowList';
import { Section } from '../components/display/Section';
import { StatusLamp, TaskStatusTag, healthLampState } from '../components/display/StatusTag';
import { ArrowLeftIcon, ArrowRightIcon } from '../components/icons/prototype-icons';
import type { ProjectListEntry, ProjectListModel } from '../components/projects/project-list-model';
import {
  deriveProjectOverview,
  isoWeekNumber,
  mergeProjectFreshness,
  type ProjectOpenDecision,
  type ProjectOverviewKpiId,
  type ProjectOverviewModel,
  type ProjectPeriodUpdateView,
} from '../components/projects/project-overview-model';
import { ObservationForbidden } from '../components/states';
import {
  absoluteTimeText,
  compactTimeText,
  foldOperations,
  groupWorks,
  operationWord,
  rawObservationText,
  workLabel,
  type OperationEvent,
} from '../core/observation';
import { readsAsName } from '../core/observation/owner';
import type { DerivedTask, OverviewPart } from '../core/overview';
import { humanText } from '../i18n/human-language';
import { isAuthError } from '../lib/api';
import { readClockOffsetUnixMs, referenceNowUnixMs, useDisplayTimeZone } from '../lib/display-clock';
import { formatUnixMs } from '../lib/format';
import { freshnessWord } from '../lib/freshness';
import {
  activityQueryOptions,
  overviewModelQueryOptions,
  overviewQueryOptions,
  projectsQueryOptions,
} from '../lib/queries';
import { useRevocation } from '../lib/revocation';
import { taskPath } from '../lib/task-path';
import type { TeamFreshness } from '../lib/types';
import './ProjectOverview.css';

const RECENT_ACTIVITY_LIMIT = 6;
const WAITING_ANSWERS_LIMIT = 4;
const BLOCKED_TITLES_LIMIT = 2;

const FAILED_FRESHNESS: TeamFreshness = {
  report_received: { status: 'failed' },
  authoritative: { status: 'failed' },
  page_read: { status: 'failed' },
};

const KPI_DETAIL_IDS: readonly ProjectOverviewKpiId[] = ['verified', 'claimed', 'review', 'decisions'];

function asDetailId(value: string | null): ProjectOverviewKpiId | null {
  return value && (KPI_DETAIL_IDS as readonly string[]).includes(value)
    ? (value as ProjectOverviewKpiId)
    : null;
}

function shortTaskTitle(title: string | null, language: string, fallback: string): string {
  if (!title) return fallback;
  const label = workLabel(title, { language, recordedTitle: true, fallback }).text;
  return label.split('：')[0] || label;
}

function fullTaskTitle(title: string | null, language: string, fallback: string): string {
  if (!title) return fallback;
  return workLabel(title, { language, recordedTitle: true, fallback }).text;
}

function formatPeriodLabel(update: ProjectPeriodUpdateView, t: TFunction): string | null {
  if (update.periodLabel) return update.periodLabel;
  const week = isoWeekNumber(update.startsOn, update.endsOn);
  if (week !== null) return t('projectOverview.weekLabel', { week });
  if (update.startsOn && update.endsOn) return `${update.startsOn} – ${update.endsOn}`;
  return update.startsOn ?? update.endsOn ?? null;
}

function formatUpdateTime(
  unixMs: number | null,
  language: string,
  nowUnixMs: number,
  timeZone: string,
): string | null {
  if (unixMs === null) return null;
  return compactTimeText(unixMs, { language, nowUnixMs, timeZone });
}

function formatUpdateRowMeta(
  update: ProjectPeriodUpdateView,
  t: TFunction,
  language: string,
  nowUnixMs: number,
  timeZone: string,
): string | null {
  const period = formatPeriodLabel(update, t);
  const author = update.authorName;
  const time = formatUpdateTime(update.recordedAtUnixMs, language, nowUnixMs, timeZone);
  if (period && author && time) return t('projectOverview.updateMeta', { period, author, time });
  if (period && time) return t('projectOverview.updateMetaNoAuthor', { period, time });
  if (author && time) return t('projectOverview.updateMetaNoPeriod', { author, time });
  if (period && author) return t('projectOverview.verdictBylinePeriodAuthor', { period, author });
  if (period) return t('projectOverview.updateMetaOnlyPeriod', { period });
  return time ?? author ?? null;
}

function formatVerdictByline(
  derived: ProjectOverviewModel,
  t: TFunction,
  language: string,
  nowUnixMs: number,
  timeZone: string,
): string {
  const { verdict } = derived;
  if (verdict.kind === 'update' && verdict.latestUpdate) {
    const period = formatPeriodLabel(verdict.latestUpdate, t);
    const author = verdict.latestUpdate.authorName;
    const time = formatUpdateTime(verdict.latestUpdate.recordedAtUnixMs, language, nowUnixMs, timeZone);
    if (period && author && time) return t('projectOverview.verdictBylinePeriodAuthorTime', { period, author, time });
    if (period && time) return t('projectOverview.verdictBylinePeriodTime', { period, time });
    if (period && author) return t('projectOverview.verdictBylinePeriodAuthor', { period, author });
    if (author && time) return t('projectOverview.verdictBylineAuthorTime', { author, time });
    if (period) return t('projectOverview.verdictBylineOnlyPeriod', { period });
    return t('projectOverview.verdictBylineGenerated');
  }
  if (verdict.kind === 'stale-update') {
    if (verdict.latestUpdate) {
      const period = formatPeriodLabel(verdict.latestUpdate, t);
      const author = verdict.latestUpdate.authorName;
      const time = formatUpdateTime(verdict.latestUpdate.recordedAtUnixMs, language, nowUnixMs, timeZone);
      if (period && author && time) return t('projectOverview.verdictStaleBylinePeriodAuthorTime', { period, author, time });
      if (period && time) return t('projectOverview.verdictStaleBylinePeriodTime', { period, time });
    }
    return t('projectOverview.verdictStaleByline');
  }
  return t('projectOverview.verdictBylineGenerated');
}

function formatVerdictSentence(
  derived: ProjectOverviewModel,
  t: TFunction,
): { text: string; reporterText: boolean } {
  const { verdict } = derived;
  if (verdict.kind === 'update' && verdict.latestUpdate?.summary) {
    return { text: verdict.latestUpdate.summary, reporterText: true };
  }
  if (verdict.kind === 'stale-update' && verdict.daysSinceLastUpdate !== null) {
    return { text: t('projectOverview.verdictStaleSay', { days: verdict.daysSinceLastUpdate }), reporterText: false };
  }
  if (verdict.kind === 'facts') {
    const { total, verified, review, decisions } = verdict.factCounts;
    if (total !== null && verified !== null && review !== null && decisions !== null) {
      return {
        text: t('projectOverview.verdictFactsSay', { total, verified, review, decisions }),
        reporterText: false,
      };
    }
    return { text: t('projectOverview.verdictFactsSayPartial'), reporterText: false };
  }
  return { text: t('projectOverview.verdictUnknownSay'), reporterText: false };
}

function partNames(parts: readonly OverviewPart[], t: TFunction): string {
  return parts.map((part) => t(`workbench.home.coverageParts.${part}`)).join(t('common.listSeparator'));
}

/** Prototype pOverview(): instrument card (verdict + 4 KPIs + workstreams + waiting rail),
 * two-column lower grid (recent activity + past updates + reconciliation), and quiet coverage footer. */
export function ProjectOverview({ project }: { project: string }) {
  const { t, i18n } = useTranslation();
  const language = i18n.language;
  const navigate = useNavigate();
  const { revoked } = useRevocation();
  const [searchParams, setSearchParams] = useSearchParams();
  const [basisOpen, setBasisOpen] = useState(false);

  const projects = useQuery(projectsQueryOptions(revoked));
  const overview = useQuery(overviewQueryOptions(project, revoked));
  const modelQuery = useQuery(overviewModelQueryOptions(project || null, revoked));
  const activity = useQuery(activityQueryOptions(project, revoked));

  const displayTimeZone = useDisplayTimeZone();
  const projectEntries = projects.data?.projects as ProjectListEntry[] | undefined;
  const projectEntry = projectEntries?.find((item) => item.key === project);
  const modelData = modelQuery.data?.project === project && modelQuery.data.input && modelQuery.data.input.options
    ? (modelQuery.data as ProjectListModel)
    : undefined;
  const nowUnixMs = referenceNowUnixMs(readClockOffsetUnixMs(modelData));

  const derived = useMemo(() => deriveProjectOverview({
    projectKey: project,
    projectEntry,
    overview: overview.data,
    model: modelData,
    timeZone: displayTimeZone,
    referenceUnixMs: nowUnixMs,
  }), [project, projectEntry, overview.data, modelData, displayTimeZone, nowUnixMs]);

  const foldedActivity = useMemo(() => foldOperations(
    activity.data?.data.items ?? [],
    { language, gapThresholdMs: 30 * 60 * 1000 },
  ), [activity.data, language]);

  const overviewWorkTitleById = useMemo(() => {
    const map = new Map<string, string>();
    for (const work of overview.data?.works ?? []) {
      if (work.key && typeof work.title === 'string' && work.title.trim()) {
        map.set(work.key, work.title.trim());
      }
    }
    return map;
  }, [overview.data]);

  const modelTaskTitleById = useMemo(() => {
    const map = new Map<string, string>();
    for (const task of modelData?.input?.tasks?.items ?? []) {
      if (typeof task.id === 'string' && task.id && typeof task.title === 'string' && task.title.trim()) {
        map.set(task.id, task.title.trim());
      }
    }
    return map;
  }, [modelData]);

  if (
    revoked
    || isAuthError(projects.error)
    || isAuthError(overview.error)
    || isAuthError(modelQuery.error)
    || isAuthError(activity.error)
  ) {
    return <ObservationForbidden />;
  }

  const primaryLoading = overview.isPending && modelQuery.isPending;
  const primaryFailed = !primaryLoading && overview.isError && !overview.data && modelQuery.isError && !modelData;
  const partialFailure = !primaryLoading && !primaryFailed && (
    overview.isError
    || modelQuery.isError
    || Boolean(overview.data && overview.data.coverage.failed > 0)
    || derived.coverage.modelPartialParts.length > 0
  );
  const isEmpty = !primaryLoading && !primaryFailed && !partialFailure && !derived.coverage.orderingIncomparable
    && (overview.data?.works.length ?? 0) === 0
    && (overview.data?.workstreams.length ?? 0) === 0
    && (derived.taskCount.count === 0 || derived.taskCount.count === null)
    && derived.recentUpdates.items.length === 0
    && derived.kpis.decisions.items.length === 0
    && (foldedActivity.uniqueEvents.length === 0);
  const showUnknownNotice = !primaryLoading && !primaryFailed && !partialFailure && !isEmpty
    && (derived.coverage.orderingIncomparable || derived.coverage.hasUnknownFacts);

  const scopeQuery = `scope=${encodeURIComponent(project)}`;
  const activeDetail = asDetailId(searchParams.get('detail'));

  function setDetailParam(nextDetail: ProjectOverviewKpiId | null) {
    const next = new URLSearchParams(searchParams);
    if (nextDetail === null) next.delete('detail');
    else next.set('detail', nextDetail);
    setSearchParams(next);
  }

  function openTab(tab: string, extra?: Record<string, string>) {
    const next = new URLSearchParams(searchParams);
    next.set('tab', tab);
    next.delete('detail');
    next.delete('page');
    next.delete('work');
    if (extra) {
      for (const [k, v] of Object.entries(extra)) next.set(k, v);
    } else {
      next.delete('stream');
    }
    setSearchParams(next);
  }

  function openDecision(decision: ProjectOpenDecision) {
    if (decision.inboxDetailKey) {
      navigate(`/inbox?${scopeQuery}&detail=${encodeURIComponent(decision.inboxDetailKey)}`);
      return;
    }
    if (decision.taskId) {
      navigate(taskPath(project, decision.taskId));
      return;
    }
    openTab('knowledge');
  }

  function openTask(task: DerivedTask) {
    if (task.id) navigate(taskPath(project, task.id));
  }

  function actorText(event: OperationEvent): string {
    const recordedName = typeof event.actor?.display_name === 'string' ? event.actor.display_name.trim() : '';
    const name = recordedName && readsAsName(recordedName) ? recordedName : t('activity.unregisteredName');
    const recordedClient = typeof event.client?.display_name === 'string' ? event.client.display_name.trim() : '';
    if (!recordedClient || !readsAsName(recordedClient)) return name;
    return event.client_id
      ? t('activity.actorByClient', { client: recordedClient, name })
      : t('activity.actorViaClient', { client: recordedClient, name });
  }

  function activityTaskTitle(workId: string | null): string | null {
    if (!workId) return null;
    const raw = overviewWorkTitleById.get(workId) ?? modelTaskTitleById.get(workId);
    if (!raw || raw === workId) return null;
    return workLabel(raw, { language, recordedTitle: true }).text;
  }

  function retryFailed() {
    if (overview.isError) void overview.refetch();
    if (modelQuery.isError) void modelQuery.refetch();
    if (activity.isError) void activity.refetch();
  }

  if (primaryLoading) {
    return (
      <div className="project-overview" data-testid="page-project-overview">
        <div className="project-overview-state" data-testid="project-overview-loading">
          {t('projectOverview.loading')}
        </div>
      </div>
    );
  }

  if (primaryFailed) {
    return (
      <div className="project-overview" data-testid="page-project-overview">
        <Notice
          tone="warning"
          title={t('projectOverview.failedTitle')}
          extra={<Button size="small" onClick={retryFailed}>{t('projectOverview.retry')}</Button>}
          className="project-overview-notice"
        >
          {t('projectOverview.failedDescription')}
        </Notice>
        <div className="project-coverage-footer" data-testid="project-coverage">
          <FreshnessBadge freshness={FAILED_FRESHNESS} />
        </div>
      </div>
    );
  }

  // Verdict band
  const verdictSentence = formatVerdictSentence(derived, t);
  const verdictByline = formatVerdictByline(derived, t, language, nowUnixMs, displayTimeZone);
  const verdictHealthAttr = derived.verdict.health === 'none' || derived.verdict.health === 'unknown'
    ? undefined
    : derived.verdict.health;
  const verdictLamp = derived.verdict.health === 'unknown' ? 'none' : healthLampState(derived.verdict.health);
  const verdictWord = derived.verdict.health === 'unknown'
    ? t('projectOverview.unknownStatus')
    : t(`display.healthStatus.${derived.verdict.health}.label`);
  const verdictUpdateId = derived.verdict.latestUpdate?.id ?? null;

  // 4 KPI metrics (counts share the exact items arrays used by the detail panel)
  const { verified, claimed, review, decisions } = derived.kpis;
  const listSep = t('common.listSeparator');
  const claimedCaption = claimed.count === null
    ? t('projectOverview.countUnknown')
    : claimed.count === 0
      ? t('projectOverview.none')
      : claimed.items
        .map((task) => shortTaskTitle(overviewWorkTitleById.get(task.id ?? '') ?? task.title, language, t('projectOverview.taskUnnamed')))
        .join(listSep);

  let reviewCaption: string;
  if (review.count === null) {
    reviewCaption = t('projectOverview.countUnknown');
  } else if (review.count === 0) {
    reviewCaption = t('projectOverview.none');
  } else if (review.reviewerNames.length === 1 && !review.reviewerUnknown) {
    reviewCaption = review.oldestDays !== null
      ? t('projectOverview.waitingAllOnOne', { name: review.reviewerNames[0], days: review.oldestDays })
      : t('projectOverview.waitingAllOnOneUnknownAge', { name: review.reviewerNames[0] });
  } else if (review.reviewerNames.length > 0 && !review.reviewerUnknown) {
    const names = review.reviewerNames.join(listSep);
    reviewCaption = review.oldestDays !== null
      ? t('projectOverview.waitingOnMany', { names, days: review.oldestDays })
      : t('projectOverview.waitingOnManyUnknownAge', { names });
  } else {
    reviewCaption = review.oldestDays !== null
      ? t('projectOverview.waitingOnReviewerRole', { days: review.oldestDays })
      : t('projectOverview.waitingOnReviewerRoleUnknownAge');
  }

  const decisionsCaption = decisions.count === null
    ? t('projectOverview.countUnknown')
    : decisions.count === 0
      ? t('projectOverview.none')
      : decisions.oldestDays !== null
        ? t('projectOverview.oldestDays', { days: decisions.oldestDays })
        : t('projectOverview.waitTimeUnknown');

  const kpiItems: KpiMetric[] = [
    {
      key: 'verified',
      label: t('projectOverview.kpiVerified'),
      value: verified.count === null ? '—' : String(verified.count),
      unit: verified.count === null ? undefined : t('projectOverview.unitItem'),
      change: {
        text: derived.taskCount.count !== null
          ? t('projectOverview.totalTasks', { count: derived.taskCount.count })
          : t('projectOverview.totalTasksUnknown'),
      },
      onOpen: verified.count !== null ? () => setDetailParam('verified') : undefined,
    },
    {
      key: 'claimed',
      label: t('projectOverview.kpiClaimed'),
      value: claimed.count === null ? '—' : String(claimed.count),
      unit: claimed.count === null ? undefined : t('projectOverview.unitItem'),
      change: {
        text: claimedCaption,
        tone: claimed.count !== null && claimed.count > 0 ? 'warn' : undefined,
      },
      onOpen: claimed.count !== null && claimed.count > 0 ? () => setDetailParam('claimed') : undefined,
    },
    {
      key: 'review',
      label: t('projectOverview.kpiReview'),
      value: review.count === null ? '—' : String(review.count),
      unit: review.count === null ? undefined : t('projectOverview.unitItem'),
      change: {
        text: reviewCaption,
        tone: review.count !== null && review.count > 0 ? 'bad' : undefined,
      },
      onOpen: review.count !== null && review.count > 0 ? () => setDetailParam('review') : undefined,
    },
    {
      key: 'decisions',
      label: t('projectOverview.kpiDecisions'),
      value: decisions.count === null ? '—' : String(decisions.count),
      unit: decisions.count === null ? undefined : t('projectOverview.unitQuestion'),
      change: { text: decisionsCaption },
      onOpen: decisions.count !== null && decisions.count > 0 ? () => setDetailParam('decisions') : undefined,
    },
  ];

  // Workstreams table rows
  const workstreamRows: DataTableRow[] = derived.workstreams.map((stream) => {
    const streamLabel = workLabel(stream.title ?? stream.id, {
      language,
      recordedTitle: Boolean(stream.title),
      fallback: humanText('workstreamUnnamed', language),
    }).text;
    const blockedText = stream.blockedTasks.length > 0
      ? stream.blockedTasks
        .slice(0, BLOCKED_TITLES_LIMIT)
        .map((task) => shortTaskTitle(overviewWorkTitleById.get(task.id ?? '') ?? task.title, language, t('projectOverview.taskUnnamed')))
        .join(listSep)
      : t('projectOverview.noBlocked');

    return {
      key: stream.id,
      testId: `workstream-row-${stream.id}`,
      onOpen: () => openTab('plan', { stream: stream.id }),
      cells: [
        <span className="project-stream-name">{streamLabel}</span>,
        stream.counts !== null
          ? <CompletionBar counts={stream.counts} />
          : <span className="project-overview-sm">{t('projectOverview.completionUnknown')}</span>,
        <span className="project-overview-sm">{blockedText}</span>,
      ],
    };
  });

  // Waiting-for-answers rail items (at most 4, matching prototype)
  const railDecisions = decisions.items.slice(0, WAITING_ANSWERS_LIMIT);

  // KPI detail panel (opens from any clickable KPI; row count === KPI count)
  let detailPanel: ReactNode = null;
  if (activeDetail) {
    const detailTitle = t(`projectOverview.detailTitles.${activeDetail}`);
    if (activeDetail === 'decisions') {
      const items = decisions.items;
      const listItems: RowListItem[] = items.map((item, idx) => {
        const who = item.responderNames.length > 0
          ? t('projectOverview.waitingOnPerson', { name: item.responderNames.join(listSep) })
          : t('projectOverview.waitingOnAnyone');
        const age = item.durationDays !== null
          ? t('projectOverview.waitingDays', { days: item.durationDays })
          : t('projectOverview.waitingAgeUnknown');
        return {
          key: item.id ?? `decision-${idx}`,
          title: item.question ?? t('projectOverview.questionFallback'),
          meta: t('projectOverview.waitingMeta', { who, age }),
          onOpen: () => openDecision(item),
        };
      });
      detailPanel = (
        <div
          className="project-metric-detail"
          data-testid="project-overview-metric-detail"
          data-metric={activeDetail}
          data-count={items.length}
        >
          <Section
            title={detailTitle}
            count={items.length}
            className="project-overview-section"
            extra={(
              <div className="project-metric-detail-actions">
                <button type="button" className="project-link-btn" onClick={() => openTab('knowledge')}>
                  {t('projectOverview.openInKnowledge')}
                  <DisplayIcon icon={ArrowRightIcon} />
                </button>
                <button
                  type="button"
                  className="project-link-btn"
                  data-testid="project-overview-detail-back"
                  onClick={() => setDetailParam(null)}
                >
                  <DisplayIcon icon={ArrowLeftIcon} />
                  {t('projectOverview.detailBack')}
                </button>
              </div>
            )}
          >
            <RowList
              items={listItems}
              empty={<div className="project-overview-empty">{t('projectOverview.detailEmpty')}</div>}
            />
          </Section>
        </div>
      );
    } else {
      const tasks = activeDetail === 'verified' ? verified.items
        : activeDetail === 'claimed' ? claimed.items
          : review.items;
      const listItems: RowListItem[] = tasks.map((task, idx) => ({
        key: task.id ?? `${activeDetail}-${idx}`,
        title: fullTaskTitle(overviewWorkTitleById.get(task.id ?? '') ?? task.title, language, t('projectOverview.taskUnnamed')),
        meta: task.owner?.displayName ?? undefined,
        trailing: <TaskStatusTag status={task.status} />,
        onOpen: task.id ? () => openTask(task) : undefined,
      }));
      detailPanel = (
        <div
          className="project-metric-detail"
          data-testid="project-overview-metric-detail"
          data-metric={activeDetail}
          data-count={tasks.length}
        >
          <Section
            title={detailTitle}
            count={tasks.length}
            className="project-overview-section"
            extra={(
              <div className="project-metric-detail-actions">
                <button type="button" className="project-link-btn" onClick={() => openTab('plan')}>
                  {t('projectOverview.openInPlan')}
                  <DisplayIcon icon={ArrowRightIcon} />
                </button>
                <button
                  type="button"
                  className="project-link-btn"
                  data-testid="project-overview-detail-back"
                  onClick={() => setDetailParam(null)}
                >
                  <DisplayIcon icon={ArrowLeftIcon} />
                  {t('projectOverview.detailBack')}
                </button>
              </div>
            )}
          >
            <RowList
              items={listItems}
              empty={<div className="project-overview-empty">{t('projectOverview.detailEmpty')}</div>}
            />
          </Section>
        </div>
      );
    }
  }

  // Recent activity (at most 6 unique folded operations)
  const activityErrorCode = (activity.error as { code?: string } | null)?.code;
  const activityEvents = foldedActivity.uniqueEvents.slice(0, RECENT_ACTIVITY_LIMIT);
  const hasDuplicateActivity = foldedActivity.disclosures.some((item) => item.code === 'duplicate-id');
  let activityBody: ReactNode;
  if (activity.isPending && !activity.data) {
    activityBody = <div className="project-overview-empty" data-testid="activity-loading">{t('common.loading')}</div>;
  } else if (activityErrorCode === 'Unsupported') {
    activityBody = <div className="project-overview-empty" data-testid="activity-unavailable">{t('projectOverview.activity.unavailable')}</div>;
  } else if (activity.isError && activityEvents.length === 0) {
    activityBody = (
      <div className="project-overview-empty" data-testid="activity-failed">
        {t('activity.readFailed')}
      </div>
    );
  } else if (activityEvents.length === 0) {
    activityBody = <div className="project-overview-empty" data-testid="activity-empty">{t('projectOverview.noActivity')}</div>;
  } else {
    activityBody = (
      <ul className="project-activity-rows" data-testid="project-recent-changes">
        {activityEvents.map((event, idx) => {
          const actor = actorText(event);
          const action = operationWord(event.action, { language }).text;
          const taskTitle = activityTaskTitle(event.work_id);
          const validTime = Number.isFinite(event.created_at_unix_ms);
          const when = validTime
            ? compactTimeText(event.created_at_unix_ms, { language, nowUnixMs, timeZone: displayTimeZone })
            : t('activity.timeNotRecorded');
          const exact = validTime
            ? absoluteTimeText(event.created_at_unix_ms, { language, timeZone: displayTimeZone }).text
            : undefined;
          const unknownResult = event.result === 'unknown';
          const inner = (
            <>
              <div className="project-activity-body">
                <div className="project-activity-line">
                  <strong className="project-activity-actor">{actor}</strong>
                  {' '}
                  <span>{action}</span>
                  {unknownResult ? (
                    <>
                      {' · '}
                      <span className="project-overview-dim">{t('recentChanges.unknown')}</span>
                    </>
                  ) : null}
                </div>
                {taskTitle ? <div className="project-activity-task">{taskTitle}</div> : null}
              </div>
              <span className="project-activity-when" title={exact}>{when}</span>
            </>
          );
          return (
            <li key={event.id ?? `evt-${idx}`} className="project-activity-item" data-testid="project-activity-row">
              {event.work_id ? (
                <button
                  type="button"
                  className="project-activity-control"
                  onClick={() => navigate(taskPath(project, event.work_id!))}
                >
                  {inner}
                </button>
              ) : (
                <div className="project-activity-control">{inner}</div>
              )}
            </li>
          );
        })}
      </ul>
    );
  }

  // Past updates (recent 3 periods)
  const updateItems: RowListItem[] = derived.recentUpdates.items.map((update, idx) => ({
    key: update.id ?? `upd-${idx}`,
    leading: <StatusLamp lamp={healthLampState(update.status === 'unknown' ? 'none' : update.status)} />,
    title: <span data-reporter-text="">{update.summary ?? t('projectOverview.updateSummaryFallback')}</span>,
    meta: formatUpdateRowMeta(update, t, language, nowUnixMs, displayTimeZone) ?? undefined,
    onOpen: update.id ? () => navigate(`/updates/${encodeURIComponent(update.id!)}?${scopeQuery}`) : undefined,
  }));

  // Coverage & freshness footer disclosures (preserving all existing read coverage testIds)
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

  return (
    <div className="project-overview" data-testid="page-project-overview">
      {partialFailure ? (
        <Notice
          tone="warning"
          title={t('projectOverview.partialTitle')}
          extra={overview.isError || modelQuery.isError ? (
            <Button size="small" onClick={retryFailed}>{t('projectOverview.retry')}</Button>
          ) : undefined}
          className="project-overview-notice"
        >
          <span data-testid="project-overview-partial">{t('projectOverview.partialDescription')}</span>
        </Notice>
      ) : null}

      {showUnknownNotice ? (
        <Notice
          tone="info"
          title={t('projectOverview.unknownTitle')}
          className="project-overview-notice"
        >
          <span data-testid="project-overview-unknown">{t('projectOverview.unknownDescription')}</span>
        </Notice>
      ) : null}

      {isEmpty ? (
        <Notice
          tone="info"
          title={t('projectOverview.emptyTitle')}
          className="project-overview-notice"
        >
          <span data-testid="project-overview-empty">{t('projectOverview.emptyDescription')}</span>
        </Notice>
      ) : null}

      <section className="project-inst" data-testid="project-instrument">
        <div className="project-inst-top" data-health={verdictHealthAttr} data-testid="project-verdict">
          <div className="project-verdict">
            <span className="project-verdict-word" data-health={verdictHealthAttr} data-testid="project-verdict-word">
              <StatusLamp lamp={verdictLamp} />
              {verdictWord}
            </span>
            <p
              className="project-verdict-say"
              data-testid="project-verdict-sentence"
              data-reporter-text={verdictSentence.reporterText ? '' : undefined}
            >
              {verdictSentence.text}
            </p>
            <div className="project-verdict-by" data-testid="project-verdict-byline">
              {verdictByline}
            </div>
            {basisOpen ? (
              <div className="project-verdict-basis" data-testid="project-verdict-basis">
                {verdictUpdateId
                  ? t('projectOverview.verdictUpdateBasis')
                  : t('projectOverview.verdictGeneratedBasis')}
              </div>
            ) : null}
          </div>
          <div className="project-inst-acts">
            <button
              type="button"
              className="project-basis-btn"
              data-testid="project-verdict-basis-btn"
              aria-expanded={verdictUpdateId ? undefined : basisOpen}
              onClick={() => {
                if (verdictUpdateId) {
                  navigate(`/updates/${encodeURIComponent(verdictUpdateId)}?${scopeQuery}`);
                } else {
                  setBasisOpen((open) => !open);
                }
              }}
            >
              {basisOpen && !verdictUpdateId ? t('projectOverview.closeBasis') : t('projectOverview.verdictBasis')}
              <DisplayIcon icon={ArrowRightIcon} />
            </button>
          </div>
        </div>

        <KpiStrip items={kpiItems} />

        <div className="project-inst-body">
          <div className="project-why" data-testid="project-workstreams">
            <div className="project-why-head">
              <h2 data-testid="project-workstreams-title">
                {derived.workstreamCount !== null
                  ? t('projectOverview.streamsTitle', { count: derived.workstreamCount })
                  : t('projectOverview.streamsTitleUnknown')}
              </h2>
              <CompletionLegend />
            </div>
            <div className="project-why-table" data-testid="project-progress-table">
              <DataTable
                columns={[
                  { key: 'stream', width: 120 },
                  { key: 'completion' },
                  { key: 'blocked', width: 220, hideOnSmall: true },
                ]}
                rows={workstreamRows}
                navigationWidth={36}
                stickyNavigation={false}
                empty={<div className="project-overview-empty">{t('projectOverview.streamsEmpty')}</div>}
              />
            </div>
            <div className="project-why-foot">
              <button
                type="button"
                className="project-link-btn"
                data-testid="project-deps-link"
                onClick={() => openTab('deps')}
              >
                {t('projectOverview.streamsFooterLink')}
                <DisplayIcon icon={ArrowRightIcon} />
              </button>
            </div>
          </div>

          <aside className="project-rail" data-testid="project-waiting-answers">
            <div className="project-rail-head">
              <h2>{t('projectOverview.waitingAnswersTitle')}</h2>
              <span className="project-rail-count" data-testid="project-waiting-answers-count">
                {decisions.count === null ? '—' : decisions.count}
              </span>
            </div>
            {decisions.count === null && railDecisions.length === 0 ? (
              <div className="project-overview-empty" data-testid="project-waiting-answers-unknown">
                {t('projectOverview.waitingAnswersUnknown')}
              </div>
            ) : railDecisions.length === 0 ? (
              <div className="project-overview-empty" data-testid="project-waiting-answers-empty">
                {t('projectOverview.noWaitingAnswers')}
              </div>
            ) : (
              railDecisions.map((decision, idx) => {
                const who = decision.responderNames.length > 0
                  ? t('projectOverview.waitingOnPerson', { name: decision.responderNames.join(listSep) })
                  : t('projectOverview.waitingOnAnyone');
                const age = decision.durationDays !== null
                  ? t('projectOverview.waitingDays', { days: decision.durationDays })
                  : t('projectOverview.waitingAgeUnknown');
                return (
                  <button
                    key={decision.id ?? `rail-decision-${idx}`}
                    type="button"
                    className="project-answer-card"
                    data-testid="project-waiting-answer-item"
                    onClick={() => openDecision(decision)}
                  >
                    <div className="project-answer-title">
                      {decision.question ?? t('projectOverview.questionFallback')}
                    </div>
                    <div className="project-answer-age">
                      {t('projectOverview.waitingMeta', { who, age })}
                    </div>
                  </button>
                );
              })
            )}
          </aside>
        </div>
      </section>

      {detailPanel}

      <div className="project-overview-cols">
        <div className="project-overview-stack">
          <div data-testid="project-activity">
            <Section
              title={t('projectOverview.recentActivityTitle')}
              className="project-overview-section"
              extra={(
                <button
                  type="button"
                  className="project-link-btn"
                  data-testid="project-activity-all"
                  onClick={() => navigate(`/activity?project=${encodeURIComponent(project)}&${scopeQuery}`)}
                >
                  {t('projectOverview.viewAll')}
                  <DisplayIcon icon={ArrowRightIcon} />
                </button>
              )}
            >
              {activityBody}
              {activity.isError && activityEvents.length > 0 ? (
                <div className="project-activity-note" data-testid="activity-partial">
                  {t('activity.partialRead')}
                </div>
              ) : null}
              {activity.data?.data.next_cursor != null ? (
                <div className="project-activity-note" data-testid="activity-more">
                  {t('projectOverview.activity.earlier')}
                </div>
              ) : null}
              {hasDuplicateActivity ? (
                <div className="project-activity-note" data-testid="activity-duplicates">
                  {t('projectOverview.activityDuplicateNote')}
                </div>
              ) : null}
            </Section>
          </div>
        </div>

        <div className="project-overview-stack">
          <div className="project-updates-list" data-testid="project-updates">
            <Section
              title={t('projectOverview.pastUpdatesTitle')}
              count={derived.recentUpdates.total ?? undefined}
              className="project-overview-section"
            >
              {derived.recentUpdates.coverage === 'unread' ? (
                <div className="project-overview-empty" data-testid="project-updates-unread">
                  {t('projectOverview.updatesUnread')}
                </div>
              ) : (
                <RowList
                  items={updateItems}
                  empty={
                    <div className="project-overview-empty" data-testid="project-updates-empty">
                      {t('projectOverview.noUpdates')}
                    </div>
                  }
                />
              )}
            </Section>
          </div>

          <div data-testid="project-reconciliation">
            <Section
              title={t('projectOverview.reconciliationTitle')}
              padded
              className="project-overview-section"
            >
              <p data-testid="project-reconciliation-status">{t('projectOverview.reconciliationNone')}</p>
              <p className="project-overview-dim">{t('projectOverview.reconciliationNote')}</p>
            </Section>
          </div>
        </div>
      </div>

      <footer className="project-coverage-footer" data-testid="project-coverage">
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
            <div key={stream.workstream_id} className="project-coverage-stream" data-testid={`coverage-stream-${stream.workstream_id}`}>
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
          <div data-testid="project-coverage-unread">
            {t('projectOverview.coverage.unread')}
          </div>
        ) : null}
        {derived.coverage.modelPartialParts.length > 0 ? (
          <div data-testid="project-coverage-model-partial">
            {t('projectOverview.modelCoveragePartial', { parts: partNames(derived.coverage.modelPartialParts, t) })}
          </div>
        ) : null}
        {derived.coverage.modelUnreadParts.length > 0 ? (
          <div data-testid="project-coverage-model-unread">
            {t('projectOverview.modelCoverageUnread', { parts: partNames(derived.coverage.modelUnreadParts, t) })}
          </div>
        ) : null}
      </footer>
    </div>
  );
}
