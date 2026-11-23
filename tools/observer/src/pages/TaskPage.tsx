import { useQueries, useQuery } from '@tanstack/react-query';
import { Button, Typography } from 'antd';
import { useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useNavigate, useParams, useSearchParams } from 'react-router';
import { FreshnessBadge } from '../components/FreshnessBadge';
import { AcceptanceList } from '../components/display/AcceptanceList';
import { DataTable, type DataTableRow } from '../components/display/DataTable';
import { DisplayIcon } from '../components/display/DisplayIcon';
import { KeyValueTable } from '../components/display/KeyValueTable';
import { Notice } from '../components/display/Notice';
import { Section } from '../components/display/Section';
import { StatusLamp, TaskStatusTag, healthLampState, type HealthStatus } from '../components/display/StatusTag';
import { Timeline, type TimelineItem } from '../components/display/Timeline';
import {
  ArrowLeftIcon,
  ArrowRightIcon,
  BotIcon,
  CheckIcon,
  ChevronRightIcon,
  CircleIcon,
  CircleXIcon,
  FileTextIcon,
  InfoIcon,
} from '../components/icons/prototype-icons';
import { ObservationForbidden } from '../components/states';
import { ApiError, isAuthError } from '../lib/api';
import { dependenciesQueryOptions, handoffsQueryOptions } from '../lib/collaboration';
import { readClockOffsetUnixMs, referenceNowUnixMs, useDisplayTimeZone } from '../lib/display-clock';
import { reviewRoundsQueryOptions } from '../lib/outcome';
import { projectLabel } from '../lib/project-label';
import {
  activityQueryOptions,
  overviewModelQueryOptions,
  overviewQueryOptions,
  projectGoalsQueryOptions,
  projectLessonsQueryOptions,
  projectsQueryOptions,
  reviewQueryOptions,
  taskDocumentsQueryOptions,
  workSnapshotQueryOptions,
} from '../lib/queries';
import { useRevocation } from '../lib/revocation';
import { useScope } from '../lib/scope';
import { taskPath } from '../lib/task-path';
import type { TeamFreshness, TeamProject } from '../lib/types';
import { PERSON_ROUTE_BASE } from './MembersPage';
import { deriveTaskPageModel, type TaskDeliveryRoundView } from './task-page-model';
import './TaskPage.css';

const FAILED_FRESHNESS: TeamFreshness = {
  report_received: { status: 'failed' },
  authoritative: { status: 'failed' },
  page_read: { status: 'failed' },
};

function isUnsupportedError(error: unknown): boolean {
  return error instanceof ApiError && error.code === 'Unsupported';
}

function roundVerdictHealth(verdict: TaskDeliveryRoundView['verdict']): HealthStatus | null {
  if (verdict === 'open') return 'risk';
  if (verdict === 'approved') return 'ok';
  if (verdict === 'rejected') return 'off';
  return null;
}

function AvatarSmall({ name, external = false }: { name: string; external?: boolean }) {
  const { t } = useTranslation();
  const initial = external
    ? t('taskPage.blocker.externalBadge')
    : Array.from(name.trim())[0] ?? '';
  return (
    <span className="task-avatar-sm" data-external={external || undefined} aria-hidden="true">
      {initial}
    </span>
  );
}

/**
 * Prototype taskPage() (AWR-PPO-050):
 * - Header: title, programmatic one-sentence summary, status tag, owner, active Agent, next step
 * - Left column: Acceptance criteria (PPD-040 or "尚无逐条依据"), Latest report, Delivery & review, History
 * - Right column: Where it is stuck, Why, Upstream & downstream, Required documents, Lessons applied
 * - Deliberately omits the prototype's collapsed "技术细节" section and raw copy controls (AWR-OBS-042).
 */
export function TaskPage() {
  const { t, i18n } = useTranslation();
  const language = i18n.language;
  const navigate = useNavigate();
  const { task: workId = '' } = useParams();
  const [searchParams] = useSearchParams();
  const { projectKey } = useScope();
  const { revoked } = useRevocation();
  const displayTimeZone = useDisplayTimeZone();
  const [behindOpen, setBehindOpen] = useState(false);

  const acceptanceRef = useRef<HTMLDivElement>(null);
  const timelineRef = useRef<HTMLDivElement>(null);
  const depsRef = useRef<HTMLDivElement>(null);
  const docsRef = useRef<HTMLDivElement>(null);
  const lessonsRef = useRef<HTMLDivElement>(null);

  const projects = useQuery(projectsQueryOptions(revoked));
  const projectList = (projects.data?.projects ?? []) as Array<TeamProject & { takeover?: unknown }>;
  const ordinaryProjects = projectList.filter((p) => !p.takeover);

  // When navigated without ?scope=..., look up which visible project owns this workId.
  const discoveryOverviews = useQueries({
    queries: !projectKey
      ? ordinaryProjects.map((proj) => overviewQueryOptions(proj.key, revoked))
      : [],
  });
  const discoveredProject = !projectKey
    ? discoveryOverviews.find((q) => q.data?.works.some((w) => w.key === workId))?.data?.project ?? ordinaryProjects[0]?.key ?? null
    : null;
  const project = projectKey ?? discoveredProject;

  const overview = useQuery(overviewQueryOptions(project, revoked));
  const modelQuery = useQuery(overviewModelQueryOptions(project, revoked));

  const overviewWork = overview.data?.works.find((w) => w.key === workId);
  const workstreamId = overviewWork?.workstream_id ?? searchParams.get('workstream') ?? '';

  const snapshot = useQuery(workSnapshotQueryOptions(project, workId, workstreamId, revoked, 'detail'));
  const snapshotData = snapshot.data?.result.data;
  const currentRoundId = snapshotData?.work_id === workId
    ? snapshotData.observation?.collaboration?.review?.id ?? null
    : null;

  const reviewRounds = useQuery(reviewRoundsQueryOptions(project ?? '', workId, workstreamId, null, revoked));
  const reviewInspect = useQuery(reviewQueryOptions(project, workId || null, currentRoundId, revoked));
  const dependencies = useQuery(dependenciesQueryOptions(project ?? '', workId || null, null, revoked));
  const handoffs = useQuery(handoffsQueryOptions(project ?? '', workId, null, revoked));
  const activity = useQuery(activityQueryOptions(project, revoked, workId || undefined));
  const documents = useQuery(taskDocumentsQueryOptions(project, workId, revoked));
  const goals = useQuery(projectGoalsQueryOptions(project, revoked));
  const lessons = useQuery(projectLessonsQueryOptions(project, revoked));

  const modelData = modelQuery.data?.project === project && modelQuery.data.input?.options
    ? modelQuery.data
    : undefined;
  const nowUnixMs = referenceNowUnixMs(readClockOffsetUnixMs(modelData));

  const projectIndex = projectList.findIndex((p) => p.key === project);
  const projectMeta = projectIndex >= 0 ? projectList[projectIndex] : undefined;
  const projectTitle = projectMeta
    ? projectLabel(projectMeta, projectIndex + 1)
    : t('shell.currentProject');

  const viewModel = useMemo(() => deriveTaskPageModel({
    workId,
    projectKey: project ?? '',
    projectTitle,
    overview: overview.data,
    model: modelData,
    snapshot: snapshotData,
    snapshotFreshness: snapshot.data?.freshness,
    reviewRounds: reviewRounds.data?.result.data,
    reviewInspect: reviewInspect.data?.result.data,
    dependencies: dependencies.data?.result.data.items,
    handoffs: handoffs.data?.result.data,
    activity: activity.data?.data.items,
    documents: documents.data?.result.data.items,
    goals: goals.data?.result.data.items,
    lessons: lessons.data?.result.data.items,
    language,
    nowUnixMs,
    timeZone: displayTimeZone,
    t,
  }), [
    workId, project, projectTitle, overview.data, modelData, snapshotData, snapshot.data?.freshness,
    reviewRounds.data, reviewInspect.data, dependencies.data, handoffs.data, activity.data,
    documents.data, goals.data, lessons.data, language, nowUnixMs, displayTimeZone, t,
  ]);

  // Access revocation: whole-page reads clear immediately. Object-scoped denials on secondary
  // reads (activity, review.inspect, docs, goals, lessons) remain local partial failures.
  if (
    revoked
    || isAuthError(projects.error)
    || isAuthError(overview.error)
    || isAuthError(modelQuery.error)
    || isAuthError(snapshot.error)
  ) {
    return <ObservationForbidden />;
  }

  const scopeQuery = project ? `scope=${encodeURIComponent(project)}` : '';
  const fallbackBackTarget = project ? `/projects/${encodeURIComponent(project)}?tab=plan` : '/projects';

  function handleBack() {
    if (window.history.length > 1) {
      navigate(-1);
    } else {
      navigate(fallbackBackTarget);
    }
  }

  function retryAll() {
    void overview.refetch();
    void modelQuery.refetch();
    if (workstreamId) {
      void snapshot.refetch();
      void reviewRounds.refetch();
    }
    if (currentRoundId) void reviewInspect.refetch();
    void dependencies.refetch();
    void handoffs.refetch();
    void activity.refetch();
    void documents.refetch();
    void goals.refetch();
    void lessons.refetch();
  }

  const discovering = !projectKey && (projects.isPending || discoveryOverviews.some((q) => q.isPending && !q.data));
  const primaryLoading = discovering
    || (overview.isPending && !overview.data && modelQuery.isPending && !modelData)
    || (Boolean(overviewWork) && snapshot.isPending && !snapshotData && modelQuery.isPending && !modelData);

  const primaryFailed = !primaryLoading && !viewModel.found && (
    (overview.isError && !overview.data && (modelQuery.isError || !modelData))
    || (projects.isError && !projects.data)
    || (Boolean(workstreamId) && snapshot.isError && !snapshotData && !overviewWork && !modelData)
  );

  const isEmpty = !primaryLoading && !primaryFailed && !viewModel.found;

  const goalsUnsupported = isUnsupportedError(goals.error);
  const goalsFailed = goals.isError && !goalsUnsupported;
  const lessonsUnsupported = isUnsupportedError(lessons.error);
  const lessonsFailed = lessons.isError && !lessonsUnsupported;
  const activityUnsupported = isUnsupportedError(activity.error);

  const secondaryErrors = [
    snapshot.isError && !snapshotData,
    reviewRounds.isError && !isUnsupportedError(reviewRounds.error),
    reviewInspect.isError && !isUnsupportedError(reviewInspect.error),
    dependencies.isError && !isUnsupportedError(dependencies.error),
    handoffs.isError && !isUnsupportedError(handoffs.error),
    activity.isError && !activityUnsupported,
    documents.isError && !isUnsupportedError(documents.error),
    goalsFailed,
    lessonsFailed,
  ].some(Boolean);

  const overviewHasFailedStreams = Boolean(overview.data && overview.data.coverage.failed > 0);
  const modelHasPartialParts = Boolean(
    modelData && Object.values(modelData.coverage).some((part) => part.status === 'partial'),
  );
  const anyPartialReadError = overview.isError
    || modelQuery.isError
    || overviewHasFailedStreams
    || modelHasPartialParts
    || secondaryErrors;
  const partialFailure = !primaryLoading && !primaryFailed && !isEmpty && anyPartialReadError;

  const showUnknownNotice = !primaryLoading && !primaryFailed && !isEmpty && !partialFailure && viewModel.hasUnknownFacts;

  const backButton = (
    <button
      type="button"
      className="task-back"
      data-testid="task-back"
      onClick={handleBack}
    >
      <DisplayIcon icon={ArrowLeftIcon} />
      {t('taskPage.back')}
    </button>
  );

  if (primaryLoading) {
    return (
      <div className="task-page" data-testid="page-task">
        {backButton}
        <div className="task-state-card" data-testid="task-loading">
          <p className="task-state-desc">{t('taskPage.states.loading')}</p>
        </div>
      </div>
    );
  }

  if (primaryFailed) {
    return (
      <div className="task-page" data-testid="page-task">
        {backButton}
        <div className="task-state-card" data-testid="task-failed">
          <h1 className="task-state-title">{t('taskPage.states.failedTitle')}</h1>
          <p className="task-state-desc">{t('taskPage.states.failedDescription')}</p>
          <div className="task-state-actions">
            <Button size="small" onClick={retryAll}>{t('taskPage.states.retry')}</Button>
          </div>
        </div>
        <footer className="task-coverage-footer" data-testid="task-coverage">
          <FreshnessBadge freshness={FAILED_FRESHNESS} />
          <div data-testid="task-coverage-summary">{t('taskPage.coverage.failed')}</div>
        </footer>
      </div>
    );
  }

  if (isEmpty) {
    return (
      <div className="task-page" data-testid="page-task">
        {backButton}
        <div className="task-state-card" data-testid="task-empty">
          <h1 className="task-state-title">{t('taskPage.states.emptyTitle')}</h1>
          <p className="task-state-desc">{t('taskPage.states.emptyDescription')}</p>
          <div className="task-state-actions">
            <button
              type="button"
              className="task-link"
              onClick={() => navigate(fallbackBackTarget)}
            >
              {t('taskPage.states.backToPlan')}
              <DisplayIcon icon={ArrowRightIcon} />
            </button>
          </div>
        </div>
        <footer className="task-coverage-footer" data-testid="task-coverage">
          {viewModel.freshness ? <FreshnessBadge freshness={viewModel.freshness} /> : null}
          <div data-testid="task-coverage-summary">
            {anyPartialReadError ? t('taskPage.coverage.notFoundPartial') : t('taskPage.coverage.notFound')}
          </div>
        </footer>
      </div>
    );
  }

  // Build delivery review rounds table rows
  const reviewTableRows: DataTableRow[] = (viewModel.delivery?.rounds ?? []).map((round) => {
    const tone = roundVerdictHealth(round.verdict);
    const reasonText = round.reason
      ?? (round.verdict === 'open'
        ? (round.waitDays !== null
          ? t('taskPage.delivery.openReasonDays', { days: round.waitDays })
          : t('taskPage.delivery.openReason'))
        : t('taskPage.delivery.reasonNone'));
    return {
      key: round.key,
      cells: [
        t('taskPage.delivery.roundLabel', { index: round.roundIndex }),
        (
          <span className="obs-tag" data-tone={tone ?? undefined}>
            <StatusLamp lamp={tone ? healthLampState(tone) : 'none'} />
            {t(`taskPage.delivery.verdict.${round.verdict}`)}
          </span>
        ),
        (
          <div className="task-to">
            <AvatarSmall name={round.reviewerName} />
            <span>
              {t('taskPage.delivery.reviewerWithRole', {
                name: round.reviewerName,
                role: t(`taskPage.delivery.role.${round.reviewerRole}`),
              })}
            </span>
          </div>
        ),
        <span data-reporter-text={round.reason ? '' : undefined}>{reasonText}</span>,
      ],
    };
  });

  // Build Timeline items
  const timelineItems: TimelineItem[] = viewModel.timeline.map((entry) => ({
    key: entry.key,
    time: t('taskPage.timeline.whenWho', { time: entry.timeText, who: entry.who }),
    title: <span data-reporter-text={entry.reporterText ? '' : undefined}>{entry.what}</span>,
    detail: entry.next || entry.note ? (
      <>
        {entry.next ? (
          <div className="task-timeline-extra">
            {t('taskPage.timeline.nextPrefix')}
            <span data-reporter-text="">{entry.next}</span>
          </div>
        ) : null}
        {entry.note ? (
          <div className="task-timeline-extra">
            {t('taskPage.timeline.openPrefix')}
            <span data-reporter-text={entry.reporterText ? '' : undefined}>{entry.note}</span>
          </div>
        ) : null}
      </>
    ) : undefined,
    tone: entry.tone,
  }));

  const statusLabelOverride = viewModel.status === 'verified' ? t('taskPage.status.verified') : undefined;

  const scrollToRef = (ref: React.RefObject<HTMLDivElement | null>) => {
    ref.current?.scrollIntoView({ block: 'start' });
  };

  return (
    <div className="task-page" data-testid="page-task">
      {backButton}

      {partialFailure ? (
        <Notice
          tone="warning"
          title={t('taskPage.states.partialTitle')}
          extra={<Button size="small" onClick={retryAll}>{t('taskPage.states.retry')}</Button>}
          className="task-notice"
        >
          <span data-testid="task-partial">{t('taskPage.states.partialDescription')}</span>
        </Notice>
      ) : null}

      {showUnknownNotice ? (
        <Notice
          tone="info"
          title={t('taskPage.states.unknownTitle')}
          className="task-notice"
        >
          <span data-testid="task-unknown">{t('taskPage.states.unknownDescription')}</span>
        </Notice>
      ) : null}

      <header className="task-hero" data-testid="task-hero">
        <div className="task-hero-main">
          <Typography.Title level={1} className="task-title" data-testid="task-title">
            {viewModel.title}
          </Typography.Title>
          <p className="task-summary" data-testid="task-summary">
            {viewModel.summaryText}
          </p>
          <div className="task-meta" data-testid="task-meta">
            <TaskStatusTag
              status={viewModel.status}
              live={viewModel.liveRunning}
              label={statusLabelOverride}
            />
            <span>{t('taskPage.ownerLabel')}</span>
            {viewModel.ownerName ? (
              viewModel.ownerPersonId ? (
                <button
                  type="button"
                  className="task-person-link"
                  data-testid="task-owner-link"
                  onClick={() => navigate(`/${PERSON_ROUTE_BASE}/${encodeURIComponent(viewModel.ownerPersonId!)}?${scopeQuery}`)}
                >
                  <AvatarSmall name={viewModel.ownerName} />
                  {viewModel.ownerName}
                </button>
              ) : (
                <span className="task-person-static" data-testid="task-owner-name">
                  <AvatarSmall name={viewModel.ownerName} />
                  {viewModel.ownerName}
                </span>
              )
            ) : (
              <span className="task-dim" data-testid="task-owner-unknown">
                {t('taskPage.ownerUnknown')}
              </span>
            )}
            {viewModel.agentName ? (
              <span className="task-chip" data-testid="task-agent-chip">
                <BotIcon size={14} strokeWidth={1.5} aria-hidden />
                {t('taskPage.agentWorking', { agent: viewModel.agentName })}
              </span>
            ) : (
              <span className="task-dim" data-testid="task-agent-none">
                {t('taskPage.noAgent')}
              </span>
            )}
            <span data-testid="task-next-step">
              <span>
                {t('taskPage.nextLabel')}
                <span data-reporter-text={viewModel.nextStepReporterText ? '' : undefined}>
                  {viewModel.nextStepText}
                </span>
              </span>
            </span>
          </div>
        </div>
      </header>

      <div className="task-cols">
        <div className="task-stack" data-testid="task-col-left">
          {/* 1. Acceptance criteria */}
          <div ref={acceptanceRef} data-testid="task-section-acceptance">
            <Section
              title={t('taskPage.acceptance.title')}
              count={viewModel.acceptance.items.length > 0 ? viewModel.acceptance.items.length : undefined}
              countTestId="task-acceptance-count"
              onCountClick={viewModel.acceptance.items.length > 0 ? () => scrollToRef(acceptanceRef) : undefined}
              extra={
                !viewModel.acceptance.hasPerCriterionEvidence && viewModel.acceptance.items.length > 0 ? (
                  <span className="obs-tag" data-testid="task-no-criterion-evidence-tag">
                    {t('taskPage.acceptance.noEvidenceTag')}
                  </span>
                ) : undefined
              }
              className="task-section"
            >
              {snapshot.isError && !snapshotData && viewModel.acceptance.items.length === 0 ? (
                <div className="task-empty-block" data-testid="task-acceptance-failed">
                  {t('taskPage.states.sectionFailed')}
                </div>
              ) : viewModel.acceptance.items.length === 0 ? (
                <div className="task-empty-block" data-testid="task-acceptance-empty">
                  {t('taskPage.acceptance.empty')}
                </div>
              ) : viewModel.acceptance.hasPerCriterionEvidence ? (
                <AcceptanceList
                  items={viewModel.acceptance.items.map((item) => ({
                    key: item.key,
                    title: item.title,
                    evidence: item.evidence,
                    status: item.status ?? 'notStarted',
                  }))}
                />
              ) : (
                <ul className="task-acceptance-plain" data-testid="task-acceptance-no-evidence-list">
                  {viewModel.acceptance.items.map((item) => (
                    <li key={item.key} className="task-acceptance-plain-item">
                      <span className="task-acceptance-plain-icon">
                        <CircleIcon size={18} strokeWidth={1.5} aria-hidden />
                      </span>
                      <div className="task-acceptance-plain-body" data-reporter-text="">
                        {item.title}
                      </div>
                      <span className="task-acceptance-plain-word">
                        {t('taskPage.acceptance.noEvidenceWord')}
                      </span>
                    </li>
                  ))}
                </ul>
              )}
              <div className="task-pnote" data-testid="task-acceptance-note">
                {!viewModel.acceptance.hasPerCriterionEvidence && viewModel.acceptance.items.length > 0
                  ? t('taskPage.acceptance.noEvidenceNote')
                  : t('taskPage.acceptance.note')}
              </div>
            </Section>
          </div>

          {/* 2. Latest progress report */}
          <div data-testid="task-section-report">
            <Section title={t('taskPage.report.title')} className="task-section">
              {snapshot.isError && !snapshotData ? (
                <div className="task-empty-block" data-testid="task-report-failed">
                  {t('taskPage.states.sectionFailed')}
                </div>
              ) : viewModel.report ? (
                <>
                  {viewModel.report.contractNote ? (
                    <div className="task-report-contract-note" data-testid="task-report-contract-note">
                      {viewModel.report.contractNote}
                    </div>
                  ) : null}
                  <div className="task-block-body">
                    {viewModel.report.summary ? (
                      <p data-testid="task-report-summary" data-reporter-text="">
                        {viewModel.report.summary}
                      </p>
                    ) : null}
                    <p className="task-report-attr" data-testid="task-report-attribution">
                      {viewModel.report.reportedAtText
                        ? t('taskPage.report.byAt', {
                          who: viewModel.report.reporterName ?? '',
                          at: viewModel.report.reportedAtText,
                        })
                        : t('taskPage.report.byOnly', { who: viewModel.report.reporterName ?? '' })}
                    </p>
                  </div>
                  <div className="task-report-kv-wrap">
                    <KeyValueTable
                      items={[
                        {
                          key: 'completed',
                          label: t('taskPage.report.completed'),
                          value: viewModel.report.completed.length > 0
                            ? (
                              <span data-reporter-text="">
                                {viewModel.report.completed.join(t('taskPage.report.semicolon'))}
                              </span>
                            )
                            : t('taskPage.report.notWritten'),
                        },
                        {
                          key: 'blockers',
                          label: t('taskPage.report.blockers'),
                          value: viewModel.report.blockers.length > 0
                            ? (
                              <span data-reporter-text="">
                                {viewModel.report.blockers.join(t('taskPage.report.semicolon'))}
                              </span>
                            )
                            : t('taskPage.report.notWritten'),
                        },
                        {
                          key: 'tests',
                          label: t('taskPage.report.tests'),
                          value: viewModel.report.tests.length > 0 ? (
                            <div className="task-chips">
                              {viewModel.report.tests.map((test, idx) => (
                                <span key={`${test.name}-${idx}`} className="task-chip">
                                  <span className="task-chip-icon" data-tone={test.tone}>
                                    {test.tone === 'positive'
                                      ? <CheckIcon size={12} strokeWidth={1.5} aria-hidden />
                                      : test.tone === 'negative'
                                        ? <CircleXIcon size={12} strokeWidth={1.5} aria-hidden />
                                        : <InfoIcon size={12} strokeWidth={1.5} aria-hidden />}
                                  </span>
                                  {`${test.name} ${test.outcome}`}
                                </span>
                              ))}
                            </div>
                          ) : t('taskPage.report.notWritten'),
                        },
                        {
                          key: 'artifacts',
                          label: t('taskPage.report.artifacts'),
                          value: viewModel.report.artifacts.length > 0
                            ? viewModel.report.artifacts.join(t('common.listSeparator'))
                            : t('taskPage.report.notWritten'),
                        },
                      ]}
                    />
                  </div>
                </>
              ) : (
                <div className="task-empty-block" data-testid="task-report-empty">
                  {t('taskPage.report.empty')}
                </div>
              )}
            </Section>
          </div>

          {/* 3. Delivery & review */}
          <div data-testid="task-section-delivery">
            <Section title={t('taskPage.delivery.title')} className="task-section">
              {snapshot.isError && !snapshotData && reviewRounds.isError ? (
                <div className="task-empty-block" data-testid="task-delivery-failed">
                  {t('taskPage.states.sectionFailed')}
                </div>
              ) : viewModel.delivery ? (
                <>
                  <KeyValueTable
                    items={[
                      {
                        key: 'version',
                        label: t('taskPage.delivery.version'),
                        value: viewModel.delivery.versionLabel ?? t('taskPage.delivery.versionUnknown'),
                      },
                      {
                        key: 'checks',
                        label: t('taskPage.delivery.checks'),
                        value: viewModel.delivery.checks.length > 0 ? (
                          <div className="task-chips">
                            {viewModel.delivery.checks.map((check, idx) => (
                              <span key={`${check.name}-${idx}`} className="task-chip">
                                <span
                                  className="task-chip-icon"
                                  data-tone={check.outcome === 'passed' ? 'positive' : check.outcome === 'failed' ? 'negative' : 'neutral'}
                                >
                                  {check.outcome === 'passed'
                                    ? <CheckIcon size={12} strokeWidth={1.5} aria-hidden />
                                    : check.outcome === 'failed'
                                      ? <CircleXIcon size={12} strokeWidth={1.5} aria-hidden />
                                      : <InfoIcon size={12} strokeWidth={1.5} aria-hidden />}
                                </span>
                                {`${check.name} ${t(
                                  check.outcome === 'passed'
                                    ? 'taskPage.report.passedOutcome'
                                    : check.outcome === 'failed'
                                      ? 'taskPage.report.failedOutcome'
                                      : 'taskPage.report.unknownOutcome',
                                )}`}
                              </span>
                            ))}
                          </div>
                        ) : t('taskPage.delivery.checksNone'),
                      },
                      {
                        key: 'integration',
                        label: t('taskPage.delivery.integration'),
                        value: t(
                          viewModel.delivery.integrationKind === 'integrated'
                            ? 'taskPage.delivery.integrated'
                            : viewModel.delivery.integrationKind === 'waitingReview'
                              ? 'taskPage.delivery.integrationWaitingReview'
                              : viewModel.delivery.integrationKind === 'notMerged'
                                ? 'taskPage.delivery.integrationNotMerged'
                                : viewModel.delivery.integrationKind === 'pending'
                                  ? 'taskPage.delivery.integrationPending'
                                  : 'taskPage.delivery.integrationUnknown',
                        ),
                      },
                    ]}
                  />
                  {reviewTableRows.length > 0 ? (
                    <DataTable
                      columns={[
                        { key: 'round', title: t('taskPage.delivery.colRound'), width: 88 },
                        { key: 'verdict', title: t('taskPage.delivery.colVerdict'), width: 112 },
                        { key: 'reviewer', title: t('taskPage.delivery.colReviewer'), width: '30%' },
                        { key: 'reason', title: t('taskPage.delivery.colReason') },
                      ]}
                      rows={reviewTableRows}
                      stickyNavigation={false}
                    />
                  ) : null}
                </>
              ) : (
                <div className="task-empty-block" data-testid="task-delivery-empty">
                  {viewModel.status === 'claimed'
                    ? t('taskPage.delivery.emptyClaimed')
                    : t('taskPage.delivery.empty')}
                </div>
              )}
            </Section>
          </div>

          {/* 4. History (Timeline) */}
          <div ref={timelineRef} data-testid="task-section-timeline">
            <Section
              title={t('taskPage.timeline.title')}
              count={timelineItems.length > 0 ? timelineItems.length : undefined}
              countTestId="task-timeline-count"
              onCountClick={timelineItems.length > 0 ? () => scrollToRef(timelineRef) : undefined}
              className="task-section"
            >
              {activityUnsupported && timelineItems.length === 0 ? (
                <div className="task-empty-block" data-testid="task-timeline-unavailable">
                  {t('taskPage.timeline.unavailable')}
                </div>
              ) : activity.isError && !activityUnsupported && timelineItems.length === 0 ? (
                <div className="task-empty-block" data-testid="task-timeline-failed">
                  {t('taskPage.states.sectionFailed')}
                </div>
              ) : timelineItems.length > 0 ? (
                <>
                  <Timeline items={timelineItems} />
                  {activity.data?.data.next_cursor != null ? (
                    <div className="task-pnote">{t('taskPage.timeline.earlier')}</div>
                  ) : null}
                </>
              ) : (
                <div className="task-empty-block" data-testid="task-timeline-empty">
                  {t('taskPage.timeline.empty')}
                </div>
              )}
            </Section>
          </div>
        </div>

        <div className="task-stack" data-testid="task-col-right">
          {/* 1. Where it is stuck */}
          <div data-testid="task-section-blocker">
            <Section title={t('taskPage.blocker.title')} className="task-section">
              {viewModel.blocker.kind === 'wait' ? (
                <>
                  {viewModel.blocker.wait.contractMismatch ? (
                    <div className="task-report-contract-note" data-testid="task-blocker-contract-note">
                      {t('taskPage.blocker.contractMismatch')}
                    </div>
                  ) : null}
                  <div className="task-block-body">
                  <div className="task-to-wrap">
                    <span className="obs-tag" data-tone="risk" data-testid="task-blocker-kind">
                      <StatusLamp lamp={healthLampState('risk')} />
                      {t(`taskPage.blocker.kind.${viewModel.blocker.wait.kind}`)}
                    </span>
                    <span>{t('taskPage.blocker.waitingOn')}</span>
                    {viewModel.blocker.wait.waitingOnPersonId ? (
                      <button
                        type="button"
                        className="task-person-link"
                        data-testid="task-blocker-person-link"
                        onClick={() => navigate(`/${PERSON_ROUTE_BASE}/${encodeURIComponent(viewModel.blocker.kind === 'wait' ? viewModel.blocker.wait.waitingOnPersonId! : '')}?${scopeQuery}`)}
                      >
                        <AvatarSmall name={viewModel.blocker.wait.waitingOnName} />
                        {viewModel.blocker.wait.waitingOnName}
                      </button>
                    ) : (
                      <span className="task-person-static" data-testid="task-blocker-person-static">
                        <AvatarSmall
                          name={viewModel.blocker.wait.waitingOnName}
                          external={viewModel.blocker.wait.waitingOnExternal}
                        />
                        {viewModel.blocker.wait.waitingOnName}
                      </span>
                    )}
                    {viewModel.blocker.wait.days !== null ? (
                      <span className="task-dim" data-testid="task-blocker-days">
                        {t('taskPage.blocker.days', { days: viewModel.blocker.wait.days })}
                      </span>
                    ) : null}
                  </div>
                  {viewModel.blocker.wait.what ? (
                    <p className="task-blocker-what" data-testid="task-blocker-what">
                      {viewModel.blocker.wait.what}
                    </p>
                  ) : null}
                  {viewModel.blocker.wait.behindCount !== null ? (
                    <>
                      <button
                        type="button"
                        className="task-behind-btn"
                        data-testid="task-behind-toggle"
                        aria-expanded={behindOpen}
                        onClick={() => setBehindOpen((prev) => !prev)}
                      >
                        {t('taskPage.blocker.behind', { count: viewModel.blocker.wait.behindCount })}
                      </button>
                      {behindOpen ? (
                        <div className="task-behind-detail" data-testid="task-behind-detail">
                          <div className="task-behind-detail-title">
                            {t('taskPage.blocker.behindDetailTitle', { count: viewModel.blocker.wait.behindCount })}
                          </div>
                          <ul className="task-behind-list">
                            {viewModel.blocker.wait.behindItems.map((item) => (
                              <li key={item.id}>
                                <button
                                  type="button"
                                  className="task-behind-item-btn"
                                  data-testid="task-behind-item"
                                  onClick={() => navigate(taskPath(project, item.id))}
                                >
                                  <span>{item.title}</span>
                                  <ChevronRightIcon size={14} strokeWidth={1.5} aria-hidden />
                                </button>
                              </li>
                            ))}
                          </ul>
                        </div>
                      ) : null}
                    </>
                  ) : (
                    <p className="task-report-attr" data-testid="task-behind-unknown">
                      {t('taskPage.blocker.behindUnknown')}
                    </p>
                  )}
                  {viewModel.blocker.wait.moreWaitsCount > 0 ? (
                    <p className="task-report-attr" data-testid="task-blocker-more-waits">
                      {t('taskPage.blocker.moreWaits', { count: viewModel.blocker.wait.moreWaitsCount })}
                    </p>
                  ) : null}
                  {viewModel.blocker.wait.inboxDetailKey ? (
                    <div className="task-blocker-link-wrap">
                      <button
                        type="button"
                        className="task-link"
                        data-testid="task-blocker-inbox-link"
                        onClick={() => navigate(`/inbox?${scopeQuery}&detail=${encodeURIComponent(viewModel.blocker.kind === 'wait' ? viewModel.blocker.wait.inboxDetailKey! : '')}`)}
                      >
                        {t('taskPage.blocker.viewQuestion')}
                        <DisplayIcon icon={ArrowRightIcon} />
                      </button>
                    </div>
                  ) : null}
                </div>
                </>
              ) : viewModel.blocker.kind === 'assignment' ? (
                <div className="task-block-body">
                  <div className="task-to-wrap">
                    <span className="obs-tag" data-tone="info" data-testid="task-blocker-assign-tag">
                      <StatusLamp lamp={healthLampState('info')} />
                      {t('taskPage.blocker.assignTag')}
                    </span>
                    <span>{t('taskPage.blocker.waitingOn')}</span>
                    {viewModel.blocker.assignment.toPersonId ? (
                      <button
                        type="button"
                        className="task-person-link"
                        onClick={() => navigate(`/${PERSON_ROUTE_BASE}/${encodeURIComponent(viewModel.blocker.kind === 'assignment' ? viewModel.blocker.assignment.toPersonId! : '')}?${scopeQuery}`)}
                      >
                        <AvatarSmall name={viewModel.blocker.assignment.toName} />
                        {viewModel.blocker.assignment.toName}
                      </button>
                    ) : (
                      <span className="task-person-static">
                        <AvatarSmall name={viewModel.blocker.assignment.toName} />
                        {viewModel.blocker.assignment.toName}
                      </span>
                    )}
                  </div>
                  <p className="task-blocker-what">
                    {viewModel.blocker.assignment.atText
                      ? t('taskPage.blocker.assignBodyAt', {
                        from: viewModel.blocker.assignment.fromName,
                        at: viewModel.blocker.assignment.atText,
                        to: viewModel.blocker.assignment.toName,
                      })
                      : t('taskPage.blocker.assignBody', {
                        from: viewModel.blocker.assignment.fromName,
                        to: viewModel.blocker.assignment.toName,
                      })}
                  </p>
                  {viewModel.blocker.assignment.inboxDetailKey ? (
                    <div className="task-blocker-link-wrap">
                      <button
                        type="button"
                        className="task-link"
                        data-testid="task-blocker-assign-link"
                        onClick={() => navigate(`/inbox?${scopeQuery}&detail=${encodeURIComponent(viewModel.blocker.kind === 'assignment' ? viewModel.blocker.assignment.inboxDetailKey! : '')}`)}
                      >
                        {t('taskPage.blocker.viewAssign')}
                        <DisplayIcon icon={ArrowRightIcon} />
                      </button>
                    </div>
                  ) : null}
                </div>
              ) : viewModel.blocker.kind === 'unknown' ? (
                <div className="task-empty-block" data-testid="task-blocker-unknown">
                  {t('taskPage.blocker.unknown')}
                </div>
              ) : (
                <div className="task-empty-block" data-testid="task-blocker-empty">
                  {viewModel.status === 'verified'
                    ? t('taskPage.blocker.emptyVerified')
                    : t('taskPage.blocker.empty')}
                </div>
              )}
            </Section>
          </div>

          {/* 2. Why (Goal & Project) */}
          <div data-testid="task-section-why">
            <Section title={t('taskPage.why.title')} className="task-section">
              <ul className="task-rows">
                <li>
                  {goalsFailed && !goals.data ? (
                    <div className="task-row-static" data-testid="task-why-goal-failed">
                      <div className="task-row-body">
                        <div className="task-row-meta">{t('taskPage.why.goalKicker')}</div>
                        <div className="task-dim">{t('taskPage.why.goalFailed')}</div>
                      </div>
                    </div>
                  ) : viewModel.why.goalId && viewModel.why.goalTitle ? (
                    <button
                      type="button"
                      className="task-row-btn"
                      data-testid="task-why-goal"
                      onClick={() => navigate(`/goals/${encodeURIComponent(viewModel.why.goalId!)}?${scopeQuery}`)}
                    >
                      <div className="task-row-body">
                        <div className="task-row-meta">{t('taskPage.why.goalKicker')}</div>
                        <div className="task-row-title">{viewModel.why.goalTitle}</div>
                      </div>
                      <span className="task-row-chev">
                        <ChevronRightIcon size={14} strokeWidth={1.5} aria-hidden />
                      </span>
                    </button>
                  ) : (
                    <div className="task-row-static" data-testid="task-why-goal">
                      <div className="task-row-body">
                        <div className="task-row-meta">{t('taskPage.why.goalKicker')}</div>
                        <div className={viewModel.why.goalTitle ? 'task-row-title' : 'task-dim'}>
                          {viewModel.why.goalTitle ?? t('taskPage.why.goalMissing')}
                        </div>
                      </div>
                    </div>
                  )}
                </li>
                <li>
                  <button
                    type="button"
                    className="task-row-btn"
                    data-testid="task-why-project"
                    onClick={() => navigate(fallbackBackTarget)}
                  >
                    <div className="task-row-body">
                      <div className="task-row-meta">{t('taskPage.why.projectKicker')}</div>
                      <div className="task-row-plain">
                        {[
                          viewModel.why.projectTitle,
                          viewModel.why.workstreamTitle,
                          viewModel.why.weekCommitment === 'in'
                            ? t('taskPage.why.inWeekCommitment')
                            : viewModel.why.weekCommitment === 'out'
                              ? t('taskPage.why.notInWeekCommitment')
                              : null,
                        ].filter(Boolean).join(' · ')}
                      </div>
                    </div>
                    <span className="task-row-chev">
                      <ChevronRightIcon size={14} strokeWidth={1.5} aria-hidden />
                    </span>
                  </button>
                </li>
              </ul>
            </Section>
          </div>

          {/* 3. Upstream & downstream */}
          <div ref={depsRef} data-testid="task-section-deps">
            <Section
              title={t('taskPage.deps.title')}
              count={viewModel.dependencies.length > 0 ? viewModel.dependencies.length : undefined}
              countTestId="task-deps-count"
              onCountClick={viewModel.dependencies.length > 0 ? () => scrollToRef(depsRef) : undefined}
              className="task-section"
            >
              {dependencies.isError && !isUnsupportedError(dependencies.error) && viewModel.dependencies.length === 0 ? (
                <div className="task-empty-block" data-testid="task-deps-failed">
                  {t('taskPage.states.sectionFailed')}
                </div>
              ) : viewModel.dependencies.length > 0 ? (
                <ul className="task-rows">
                  {viewModel.dependencies.map((dep) => (
                    <li key={dep.key}>
                      <button
                        type="button"
                        className="task-row-btn"
                        data-testid={`task-dep-${dep.direction}`}
                        onClick={() => navigate(taskPath(project, dep.taskId))}
                      >
                        <div className="task-row-body">
                          <div className="task-row-meta">
                            {dep.direction === 'upstream'
                              ? t('taskPage.deps.upstreamKicker')
                              : t('taskPage.deps.downstreamKicker')}
                          </div>
                          <div className="task-row-title">{dep.title}</div>
                          <div className="task-row-meta">{dep.meta}</div>
                        </div>
                        <span className="task-row-chev">
                          <ChevronRightIcon size={14} strokeWidth={1.5} aria-hidden />
                        </span>
                      </button>
                    </li>
                  ))}
                </ul>
              ) : (
                <div className="task-empty-block" data-testid="task-deps-empty">
                  {t('taskPage.deps.empty')}
                </div>
              )}
            </Section>
          </div>

          {/* 4. Required documents */}
          <div ref={docsRef} data-testid="task-section-docs">
            <Section
              title={t('taskPage.docs.title')}
              count={viewModel.documents.length > 0 ? viewModel.documents.length : undefined}
              countTestId="task-docs-count"
              onCountClick={viewModel.documents.length > 0 ? () => scrollToRef(docsRef) : undefined}
              className="task-section"
            >
              {documents.isError && !isUnsupportedError(documents.error) && viewModel.documents.length === 0 ? (
                <div className="task-empty-block" data-testid="task-docs-failed">
                  {t('taskPage.states.sectionFailed')}
                </div>
              ) : viewModel.documents.length > 0 ? (
                <ul className="task-rows">
                  {viewModel.documents.map((doc) => (
                    <li key={doc.key}>
                      <button
                        type="button"
                        className="task-row-btn"
                        data-testid="task-doc-row"
                        onClick={() => navigate(
                          doc.docId
                            ? `/documents/${encodeURIComponent(doc.docId)}?${scopeQuery}`
                            : `/projects/${encodeURIComponent(project ?? '')}?tab=knowledge`,
                        )}
                      >
                        <span className="task-kind-icon" aria-hidden="true">
                          <FileTextIcon size={16} strokeWidth={1.5} aria-hidden />
                        </span>
                        <div className="task-row-body">
                          <div className="task-row-title">{doc.title}</div>
                          <div className="task-row-meta">{doc.meta}</div>
                        </div>
                        <span className="task-row-chev">
                          <ChevronRightIcon size={14} strokeWidth={1.5} aria-hidden />
                        </span>
                      </button>
                    </li>
                  ))}
                </ul>
              ) : (
                <div className="task-empty-block" data-testid="task-docs-empty">
                  {t('taskPage.docs.empty')}
                </div>
              )}
            </Section>
          </div>

          {/* 5. Lessons applied (no collapsed "技术细节" block per AWR-OBS-042) */}
          <div ref={lessonsRef} data-testid="task-section-lessons">
            <Section
              title={t('taskPage.lessons.title')}
              count={viewModel.lessons.length > 0 ? viewModel.lessons.length : undefined}
              countTestId="task-lessons-count"
              onCountClick={viewModel.lessons.length > 0 ? () => scrollToRef(lessonsRef) : undefined}
              className="task-section"
            >
              {lessonsUnsupported && viewModel.lessons.length === 0 ? (
                <div className="task-empty-block" data-testid="task-lessons-unsupported">
                  {t('taskPage.lessons.unsupported')}
                </div>
              ) : lessonsFailed && viewModel.lessons.length === 0 ? (
                <div className="task-empty-block" data-testid="task-lessons-failed">
                  {t('taskPage.states.sectionFailed')}
                </div>
              ) : viewModel.lessons.length > 0 ? (
                <ul className="task-rows">
                  {viewModel.lessons.map((lesson) => {
                    const stageLabel = t(`taskPage.lessons.stage.${lesson.stage}`);
                    return (
                      <li key={lesson.key}>
                        <button
                          type="button"
                          className="task-row-btn"
                          data-testid="task-lesson-row"
                          onClick={() => navigate(`/lessons?${scopeQuery}&lesson=${encodeURIComponent(lesson.lessonId)}`)}
                        >
                          <div className="task-row-body">
                            <div className="task-row-title">{lesson.title}</div>
                            <div className="task-row-meta">
                              {lesson.authorName
                                ? t('taskPage.lessons.metaWithAuthor', { stage: stageLabel, author: lesson.authorName })
                                : stageLabel}
                            </div>
                          </div>
                          <span className="task-row-chev">
                            <ChevronRightIcon size={14} strokeWidth={1.5} aria-hidden />
                          </span>
                        </button>
                      </li>
                    );
                  })}
                </ul>
              ) : (
                <div className="task-empty-block" data-testid="task-lessons-empty">
                  {t('taskPage.lessons.empty')}
                </div>
              )}
            </Section>
          </div>
        </div>
      </div>

      <footer className="task-coverage-footer" data-testid="task-coverage">
        {viewModel.freshness ? <FreshnessBadge freshness={viewModel.freshness} /> : null}
        <div data-testid="task-coverage-summary">
          {partialFailure
            ? t('taskPage.coverage.partial')
            : showUnknownNotice
              ? t('taskPage.coverage.unknown')
              : t('taskPage.coverage.complete')}
        </div>
      </footer>
    </div>
  );
}
