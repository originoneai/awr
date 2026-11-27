import { useQueries, useQuery } from '@tanstack/react-query';
import { Button, Card, List, Pagination, Space, Table, Tag, Typography, theme } from 'antd';
import type { TFunction } from 'i18next';
import { useMemo, type ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import { useNavigate, useSearchParams } from 'react-router';
import { ArtifactReader } from '../components/ArtifactReader';
import { DeliverySteps, type DeliveryStep } from '../components/DeliverySteps';
import { FreshnessBadge } from '../components/FreshnessBadge';
import { OutcomeActivity, OutcomeReviewHistory } from '../components/OutcomeObservations';
import { RecordedTime } from '../components/RecordedTime';
import { DataTable, type DataTableRow } from '../components/display/DataTable';
import { DisplayIcon } from '../components/display/DisplayIcon';
import { Notice } from '../components/display/Notice';
import { Section } from '../components/display/Section';
import { StatusLamp, type TaskLampState } from '../components/display/StatusTag';
import { ArrowLeftIcon, ArrowRightIcon, CheckIcon, CircleXIcon, ClockIcon } from '../components/icons/prototype-icons';
import {
  deriveProjectAcceptance,
  hasDeliveredOutcome,
  normalizeAcceptanceDetail,
  type AcceptanceDetailFilter,
  type AcceptanceOutcomeRow,
  type AcceptanceReviewState,
  type RoundsReadState,
  type SnapshotReadState,
} from '../components/projects/project-acceptance-model';
import type { ProjectListModel } from '../components/projects/project-list-model';
import { ObservationForbidden } from '../components/states';
import { checkLabel, workLabel } from '../core/observation';
import { formatObservationValue, notProvided } from '../i18n/craft';
import { isAuthError } from '../lib/api';
import { readClockOffsetUnixMs, referenceNowUnixMs } from '../lib/display-clock';
import { asArray, asObject, asText, at } from '../lib/json';
import { outcomeQueryOptions, reviewRoundsQueryOptions } from '../lib/outcome';
import {
  SNAPSHOT_POLL_INTERVAL_MS,
  overviewModelQueryOptions,
  overviewQueryOptions,
  workSnapshotQueryOptions,
} from '../lib/queries';
import { useRevocation } from '../lib/revocation';
import { taskPath } from '../lib/task-path';
import type { ReviewRoundsData, TeamFreshness, WorkSnapshotData } from '../lib/types';
import './ProjectAcceptance.css';

const PAGE_SIZE = 60;

const FAILED_FRESHNESS: TeamFreshness = {
  report_received: { status: 'failed' },
  authoritative: { status: 'failed' },
  page_read: { status: 'failed' },
};

function validPage(value: string | null) {
  if (!value || !/^[1-9]\d*$/.test(value)) return 1;
  const page = Number(value);
  return Number.isSafeInteger(page) ? page : 1;
}

function reviewTagTone(state: AcceptanceReviewState): 'ok' | 'risk' | 'off' | undefined {
  if (state === 'approved') return 'ok';
  if (state === 'open') return 'risk';
  if (state === 'rejected') return 'off';
  return undefined;
}

function reviewTagLamp(state: AcceptanceReviewState): TaskLampState {
  if (state === 'approved') return 'done';
  if (state === 'open') return 'stalled';
  if (state === 'rejected') return 'blocked';
  return 'none';
}

function formatVersion(row: AcceptanceOutcomeRow, t: TFunction): string {
  if (row.version.kind === 'numbered') {
    return t('projectAcceptance.versionNumber', { count: row.version.number });
  }
  if (row.version.kind === 'current') return t('projectAcceptance.versionCurrent');
  if (row.version.kind === 'historical') return t('projectAcceptance.versionHistorical');
  return t('projectAcceptance.versionUnknown');
}

function formatReviewMeta(row: AcceptanceOutcomeRow, t: TFunction, language: string): string | null {
  const rev = row.latestReview;
  if (rev.restricted) return t('projectAcceptance.roundRestricted');
  const parts: string[] = [];
  if (rev.roundIndex !== null) {
    parts.push(t('projectAcceptance.roundNumber', { count: rev.roundIndex }));
  }
  const roleLabel = rev.reviewerRole ? t(`outcomeReviews.roles.${rev.reviewerRole}`) : null;
  if (rev.reviewerName && roleLabel) {
    parts.push(t('projectAcceptance.reviewerWithRole', { name: rev.reviewerName, role: roleLabel }));
  } else if (rev.reviewerName) {
    parts.push(rev.reviewerName);
  } else if (roleLabel) {
    parts.push(roleLabel);
  }
  if (rev.reasonFirstLine) {
    parts.push(formatObservationValue(rev.reasonFirstLine, language));
  } else if (rev.roundsFailed) {
    parts.push(t('projectAcceptance.roundReadFailed'));
  } else if (rev.state === 'open') {
    if (rev.waitDays !== null && rev.waitDays >= 1) {
      parts.push(t('projectAcceptance.openWaitDays', { days: rev.waitDays }));
    } else if (rev.waitDays === 0) {
      parts.push(t('projectAcceptance.openWaitToday'));
    } else {
      parts.push(t('projectAcceptance.openNoConclusion'));
    }
  }
  if (parts.length === 0) return null;
  let joined = parts[0];
  for (let i = 1; i < parts.length; i += 1) {
    joined += joined.endsWith('）') ? `· ${parts[i]}` : ` · ${parts[i]}`;
  }
  return joined;
}

/** Prototype pDelivery(): lead counts, outcomes/checks/review table, independent-review note,
 * and preserved per-work delivery/evidence/review/artifact detail view when a row is opened. */
export function ProjectAcceptance({ project }: { project: string }) {
  const { t, i18n } = useTranslation();
  const language = i18n.language;
  const { token } = theme.useToken();
  const navigate = useNavigate();
  const { revoked } = useRevocation();
  const [searchParams, setSearchParams] = useSearchParams();

  const overview = useQuery(overviewQueryOptions(project, revoked));
  const modelQuery = useQuery(overviewModelQueryOptions(project || null, revoked));
  const modelData = modelQuery.data?.project === project && modelQuery.data.input && modelQuery.data.input.options
    ? (modelQuery.data as ProjectListModel)
    : undefined;
  const nowUnixMs = referenceNowUnixMs(readClockOffsetUnixMs(modelData));

  const works = overview.data?.works ?? [];
  const requestedWork = searchParams.get('work');
  const activeDetail = normalizeAcceptanceDetail(searchParams.get('detail'));
  const requestedIndex = requestedWork ? works.findIndex((work) => work.key === requestedWork) : -1;
  const lastPage = Math.max(1, Math.ceil(works.length / PAGE_SIZE));
  const requestedPage = validPage(searchParams.get('page'));
  const page = requestedIndex >= 0 ? Math.floor(requestedIndex / PAGE_SIZE) + 1 : Math.min(requestedPage, lastPage);
  const visibleWorks = works.slice((page - 1) * PAGE_SIZE, page * PAGE_SIZE);

  const snapshots = useQueries({
    queries: visibleWorks.map((work) => (
      workSnapshotQueryOptions(project, work.key, work.workstream_id, revoked, 'list')
    )),
  });

  const reviewRoundQueries = useQueries({
    queries: visibleWorks.map((work, index) => {
      const snapshot = snapshots[index]?.data?.result.data;
      const hasReview = hasDeliveredOutcome(snapshot) && Boolean(snapshot?.observation?.collaboration?.review?.id);
      const base = reviewRoundsQueryOptions(project, work.key, work.workstream_id, null, revoked);
      return {
        ...base,
        enabled: base.enabled && hasReview,
        refetchInterval: SNAPSHOT_POLL_INTERVAL_MS,
      };
    }),
  });

  const selectedWork = requestedWork ? visibleWorks.find((work) => work.key === requestedWork) : undefined;
  const selectedSnapshot = useQuery(workSnapshotQueryOptions(
    project, selectedWork?.key ?? '', selectedWork?.workstream_id ?? '', revoked,
  ));
  const snapshotData = selectedSnapshot?.data?.result.data;
  const collaboration = snapshotData?.observation?.collaboration;
  const currentRound = collaboration?.review?.id;
  const selectedRound = searchParams.get('round') ?? currentRound;
  const workId = selectedWork?.key ?? '';
  const enabled = Boolean(selectedWork);

  const review = useQuery(outcomeQueryOptions(project, 'review.inspect', {
    work_id: workId, review_round_id: selectedRound ?? '',
  }, revoked, enabled && Boolean(selectedRound)));
  const currentReview = useQuery(outcomeQueryOptions(project, 'review.inspect', {
    work_id: workId, review_round_id: currentRound ?? '',
  }, revoked, enabled && Boolean(currentRound)));
  const reviewObject = asObject(at(review.data?.result.data, ['review']));
  const currentReviewObject = asObject(at(currentReview.data?.result.data, ['review']));
  const evidenceId = asText(reviewObject?.evidence_id);
  const executionId = asText(reviewObject?.execution_id);
  const evidence = useQuery(outcomeQueryOptions(project, 'evidence.inspect', {
    work_id: workId, evidence_id: evidenceId ?? '',
  }, revoked, enabled && Boolean(evidenceId)));
  const execution = useQuery(outcomeQueryOptions(project, 'execution.inspect', {
    work_id: workId, execution_id: executionId ?? '',
  }, revoked, enabled && Boolean(executionId)));
  const claimId = asText(execution.data?.result.data.claim_id);
  const claim = useQuery(outcomeQueryOptions(project, 'claim.inspect', {
    work_id: workId, claim_id: claimId ?? '',
  }, revoked, enabled && Boolean(claimId)));
  const selectedCompletionId = snapshotData?.observation?.runtime?.selected_completion_id;
  const completion = useQuery(outcomeQueryOptions(project, 'completion.inspect', {
    work_id: workId,
  }, revoked, enabled && Boolean(selectedCompletionId)));
  const delivery = useQuery(outcomeQueryOptions(project, 'delivery.inspect', { work_id: workId }, revoked, enabled));
  const neutral = useQuery(outcomeQueryOptions(project, 'delivery.neutral.inspect', { work_id: workId }, revoked, enabled));
  const submission = useQuery(outcomeQueryOptions(project, 'delivery.submission.describe', { work_id: workId }, revoked, enabled));
  const integrationId = collaboration?.integration?.id;
  const integration = useQuery(outcomeQueryOptions(project, 'delivery.integration.inspect', {
    work_id: workId, request_id: integrationId ?? '',
  }, revoked, enabled && Boolean(integrationId)));
  const source = useQuery(outcomeQueryOptions(project, 'delivery.source.status', {
    work_id: workId,
  }, revoked, enabled && Boolean(collaboration?.source_state)));
  const exports = useQuery(outcomeQueryOptions(project, 'delivery.exports', {
    work_id: workId, limit: 20,
  }, revoked, enabled));

  const derived = useMemo(() => {
    const snapshotsByKey = new Map<string, WorkSnapshotData | undefined>();
    const snapshotStatusByKey = new Map<string, SnapshotReadState>();
    const reviewRoundsByKey = new Map<string, ReviewRoundsData | undefined>();
    const reviewRoundsStatusByKey = new Map<string, RoundsReadState>();

    for (let index = 0; index < visibleWorks.length; index += 1) {
      const work = visibleWorks[index];
      const snapQuery = snapshots[index];
      if (snapQuery?.data?.result?.data) {
        snapshotsByKey.set(work.key, snapQuery.data.result.data);
        snapshotStatusByKey.set(work.key, 'ready');
      } else if (snapQuery?.isError) {
        snapshotStatusByKey.set(work.key, 'failed');
      } else {
        snapshotStatusByKey.set(work.key, 'loading');
      }

      const roundQuery = reviewRoundQueries[index];
      if (roundQuery?.data?.result?.data) {
        reviewRoundsByKey.set(work.key, roundQuery.data.result.data);
        reviewRoundsStatusByKey.set(work.key, 'ready');
      } else if (roundQuery?.isError) {
        reviewRoundsStatusByKey.set(work.key, isAuthError(roundQuery.error) ? 'forbidden' : 'failed');
      } else if (roundQuery?.isFetching) {
        reviewRoundsStatusByKey.set(work.key, 'loading');
      } else {
        reviewRoundsStatusByKey.set(work.key, 'idle');
      }
    }

    return deriveProjectAcceptance({
      projectKey: project,
      overview: overview.data,
      visibleWorks,
      snapshotsByKey,
      snapshotStatusByKey,
      snapshotFreshness: selectedSnapshot?.data?.freshness,
      reviewRoundsByKey,
      reviewRoundsStatusByKey,
      model: modelData,
      nowUnixMs,
    });
  }, [
    project,
    overview.data,
    visibleWorks,
    snapshots,
    reviewRoundQueries,
    selectedSnapshot?.data?.freshness,
    modelData,
    nowUnixMs,
  ]);

  const missing = notProvided(language);
  const value = (input: unknown, field?: string) => formatObservationValue(input, language, field);
  const readError = (testId: string) => (
    <Typography.Text type="warning" data-testid={testId}>
      {t('acceptance.readFailed')}
    </Typography.Text>
  );
  const loadingText = <Typography.Text type="secondary">{t('common.loading')}</Typography.Text>;
  const emptyText = <Typography.Text type="secondary">{t('acceptance.notObserved')}</Typography.Text>;

  if (revoked || isAuthError(overview.error)) return <ObservationForbidden />;

  function retryReads() {
    if (overview.isError) void overview.refetch();
    for (const query of snapshots) if (query.isError) void query.refetch();
    for (const query of reviewRoundQueries) if (query.isError) void query.refetch();
  }

  function selectWork(key: string) {
    const next = new URLSearchParams(searchParams);
    next.set('tab', 'acceptance');
    next.set('work', key);
    next.delete('round');
    const index = works.findIndex((work) => work.key === key);
    const targetPage = Math.floor(Math.max(0, index) / PAGE_SIZE) + 1;
    if (targetPage > 1) next.set('page', String(targetPage));
    else next.delete('page');
    setSearchParams(next);
  }

  function closeDetail() {
    const next = new URLSearchParams(searchParams);
    next.delete('work');
    next.delete('round');
    setSearchParams(next);
  }

  function selectFilter(filter: AcceptanceDetailFilter) {
    const next = new URLSearchParams(searchParams);
    next.set('tab', 'acceptance');
    next.delete('work');
    next.delete('round');
    if (filter === 'all') next.delete('detail');
    else next.set('detail', filter);
    setSearchParams(next);
  }

  function selectRound(round: string) {
    const next = new URLSearchParams(searchParams);
    next.set('work', workId);
    if (round === currentRound) next.delete('round');
    else next.set('round', round);
    setSearchParams(next);
  }

  if (overview.isPending) {
    return (
      <div className="acceptance-page" data-testid="acceptance-page">
        <div className="acceptance-state" data-testid="acceptance-loading">
          {t('projectAcceptance.loading')}
        </div>
      </div>
    );
  }

  if (overview.isError && !overview.data) {
    return (
      <div className="acceptance-page" data-testid="acceptance-page">
        <Notice
          tone="warning"
          title={t('projectAcceptance.failedTitle')}
          extra={<Button size="small" onClick={retryReads}>{t('projectAcceptance.retry')}</Button>}
          className="acceptance-notice"
        >
          <span data-testid="acceptance-failed">{t('projectAcceptance.failedDescription')}</span>
        </Notice>
        <footer className="acceptance-coverage-footer" data-testid="acceptance-coverage">
          <FreshnessBadge freshness={FAILED_FRESHNESS} />
        </footer>
      </div>
    );
  }

  // Detail view calculations (preserved verbatim from AWR-OBS-006 / AWR-OBS-042).
  let detailView: ReactNode = null;
  if (selectedWork) {
    const reviewCandidateCurrent = reviewObject?.candidate_current ?? at(reviewObject, ['submission', 'candidate_current']);
    const currentCandidateCurrent = currentReviewObject?.candidate_current ?? at(currentReviewObject, ['submission', 'candidate_current']);
    const reportOnly = collaboration?.delivery_required === false
      && at(currentReviewObject, ['submission', 'delivery_required']) === false
      && !collaboration?.candidate && !integrationId;
    const reviewIsCurrent = (currentCandidateCurrent === true
        || (reportOnly && at(currentReviewObject, ['evidence_provenance', 'review_contract_matches_current']) === true))
      && at(currentReviewObject, ['evidence_provenance', 'review_contract_matches_current']) === true;
    const completionData = selectedCompletionId ? completion.data?.result.data : undefined;
    const completionObject = asObject(completionData?.completion);
    const completionReceipt = asText(completionObject?.receipt_id);
    const completionSelected = asText(completionData?.selected_completion_id);
    const completionReviewRound = at(completionObject, ['approved_by', 'review_round_id']) ?? completionObject?.review_round_id;
    const completionReviewKind = asText(at(completionObject, ['approved_by', 'independence_kind']));
    const completionReviewLabels: Record<string, string> = {
      team_independent: t('outcomeReviews.roles.independent_reviewer'),
      personal_self_review: t('outcomeReviews.roles.author_reviewer'),
      agent_review: t('outcomeReviews.roles.agent_reviewer'),
      simulated_member_independent: value('simulated_member_independent', 'independence_kind'),
    };

    const deliveryObject = asObject(at(delivery.data?.result.data, ['delivery']));
    const deliverySelected = asText(at(deliveryObject, ['awr_acceptance', 'selected_completion_id']));
    const completionBindingsPresent = [
      completionObject?.receipt_id, completionData?.selected_completion_id, selectedCompletionId,
      at(deliveryObject, ['awr_acceptance', 'selected_completion_id']),
      completionObject?.contract_hash, snapshotData?.contract_hash, currentReviewObject?.contract_hash,
      completionObject?.evidence_id, currentReviewObject?.evidence_id,
      completionReviewRound, currentRound, currentReviewObject?.round_id,
    ].every((binding) => typeof binding === 'string' && binding.trim() !== '')
      // Human-reviewed reports can explicitly record a null execution.
      && completionObject != null && Object.hasOwn(completionObject, 'execution_id')
      && currentReviewObject != null && Object.hasOwn(currentReviewObject, 'execution_id');
    const completionBindingsMatch = completionReceipt === completionSelected
      && completionSelected === selectedCompletionId && deliverySelected === completionSelected
      && completionObject?.contract_hash === snapshotData?.contract_hash
      && currentReviewObject?.contract_hash === snapshotData?.contract_hash
      && completionObject?.evidence_id === currentReviewObject?.evidence_id
      && completionObject?.execution_id === currentReviewObject?.execution_id
      && completionReviewRound === currentRound && currentReviewObject?.round_id === currentRound
      && (completionObject?.review_round_id == null || completionObject.review_round_id === completionReviewRound);
    const completionConflict = Boolean(completionData && completionBindingsPresent && !completionBindingsMatch);
    const completionBindingMissing = Boolean(completionData && !completionBindingsPresent);
    const integrationData = integration.data?.result.data;
    const observedAt = at(integrationData, ['confirmation', 'envelope', 'record', 'data', 'provenance', 'observed_at_unix_ms']);
    const recordedAt = at(integrationData, ['confirmation', 'envelope', 'record', 'data', 'provenance', 'recorded_at_unix_ms']);

    const submittedArtifacts = asArray(at(reviewObject, ['submission', 'artifacts']));
    const primaryEvidence = asObject(at(evidence.data?.result.data, ['evidence']));
    const primaryArtifactId = asText(primaryEvidence?.artifact_id);
    const primaryArtifactDigest = asText(primaryEvidence?.artifact_digest);
    const artifacts = submittedArtifacts.length > 0 ? submittedArtifacts
      : primaryArtifactId && primaryArtifactDigest && primaryEvidence?.evidence_id === evidenceId ? [{
        query: { op: 'artifact.content', work_id: workId, artifact_id: primaryArtifactId, expected_sha256: primaryArtifactDigest },
        segment_query: { op: 'artifact.segment', work_id: workId, artifact_id: primaryArtifactId, expected_sha256: primaryArtifactDigest },
      }] : [];

    const verification = asArray(collaboration?.verification);
    const reviewDecisions = asArray(reviewObject?.decisions);
    const reviewFindings = asArray(at(reviewObject, ['findings', 'findings']));
    const requiredDependencies = snapshotData?.visible_contract?.required_dependencies
      ?? snapshotData?.published_contract?.required_dependencies
      ?? asArray(at(snapshotData, ['adopted_dependencies']))
      ?? [];

    const candidatePresent = collaboration?.candidate != null || collaboration?.review != null || currentReviewObject != null;
    const reviewState = asText(currentReviewObject?.state ?? collaboration?.review?.state);
    const reviewApproved = reviewState === 'approved' && reviewIsCurrent;
    const reviewIsHistorical = (reviewCandidateCurrent === false && at(reviewObject, ['submission', 'delivery_required']) !== false)
      || at(reviewObject, ['evidence_provenance', 'review_contract_matches_current']) === false;
    const integrationState = asText(integrationData?.state ?? collaboration?.integration?.state);
    const integrationOutcome = asText(at(integrationData, ['confirmation', 'envelope', 'record', 'data', 'outcome']));
    const integrationMatches = Boolean(integrationId) && integrationData?.integration_id === integrationId;
    const integrationIsCurrent = integrationMatches
      && collaboration?.integration?.current_binding === true && integrationData?.current_binding === true;
    const integrationIsHistorical = integrationMatches
      && collaboration?.integration?.current_binding === false && integrationData?.current_binding === false;
    const integrationPrepared = Boolean(integrationId) && !integrationIsHistorical;
    const integrationConfirmed = integrationIsCurrent && integrationData?.confirmation_current === true
      && integrationState === 'confirmed' && integrationOutcome === 'applied';
    const integrationRejected = integrationIsCurrent && (integrationState === 'rejected' || integrationOutcome === 'rejected');
    const integrationUnknown = Boolean(integrationId) && (
      (!integrationIsCurrent && !integrationIsHistorical)
      || integrationState === 'unknown'
      || (integrationIsHistorical && integrationState === 'dispatched')
      || (integrationIsCurrent && integrationState === 'confirmed' && integrationData?.confirmation_current !== true)
    );
    const deliveryComplete = at(deliveryObject, ['awr_acceptance', 'complete']) === true
      && at(deliveryObject, ['awr_acceptance', 'runtime_state']) === 'completed';
    const completionKnown = completionBindingsPresent && completionBindingsMatch;
    const snapshotLoading = selectedSnapshot == null || selectedSnapshot.isPending;
    const snapshotFailed = selectedSnapshot?.isError === true;
    const reviewLoading = Boolean(currentRound) && currentReview.isPending;
    const integrationLoading = Boolean(integrationId) && integration.isPending;
    const completionLoading = Boolean(selectedCompletionId) && completion.isPending;
    const acceptanceLoading = snapshotLoading || reviewLoading || integrationLoading || completionLoading || delivery.isPending;
    const acceptanceUnknown = snapshotFailed || currentReview.isError || integration.isError || completion.isError || delivery.isError || completionBindingMissing;
    const integrationSatisfied = integrationConfirmed || reportOnly;
    const completionConfirmed = deliveryComplete && completionKnown && integrationSatisfied && reviewApproved;

    const steps: DeliveryStep[] = [
      {
        key: 'candidate', title: t('acceptance.stepCandidate'),
        text: snapshotLoading ? t('common.loading') : snapshotFailed ? t('acceptance.statusUnknown') : candidatePresent ? t('acceptance.statusRecorded') : t('acceptance.statusNotGenerated'),
        tone: snapshotFailed ? 'warning' : snapshotLoading ? 'neutral' : candidatePresent ? 'positive' : 'neutral',
      },
      {
        key: 'verification', title: t('acceptance.stepVerification'),
        text: snapshotLoading
          ? t('common.loading')
          : snapshotFailed
            ? t('acceptance.statusUnknown')
            : verification.length === 0
              ? t('acceptance.statusNoRecord')
              : verification.every((entry) => at(entry, ['outcome']) === 'passed')
                ? t('acceptance.statusPassRecord')
                : t('acceptance.statusCheckRecord'),
        tone: snapshotFailed ? 'warning' : snapshotLoading || verification.length === 0 ? 'neutral' : verification.every((entry) => at(entry, ['outcome']) === 'passed') ? 'positive' : 'warning',
      },
      {
        key: 'review', title: t('acceptance.stepReview'),
        text: snapshotLoading || reviewLoading
          ? t('common.loading')
          : snapshotFailed || currentReview.isError
            ? t('acceptance.statusUnknown')
            : reviewApproved
              ? t('acceptance.statusApproved')
              : reviewState === 'approved' && currentCandidateCurrent == null
                ? t('acceptance.statusUnknown')
                : reviewState === 'open'
                  ? t('acceptance.statusReviewPending')
                  : reviewState
                    ? t('acceptance.statusNotApproved')
                    : t('acceptance.statusNotStarted'),
        tone: snapshotFailed || currentReview.isError ? 'warning' : snapshotLoading || reviewLoading ? 'neutral' : reviewApproved ? 'positive' : reviewState === 'open' ? 'warning' : reviewState ? 'negative' : 'neutral',
        testId: reviewApproved && !currentReview.isError ? 'acceptance-current-approval' : reviewIsHistorical ? 'acceptance-review-history' : undefined,
      },
      {
        key: 'integration', title: t('acceptance.stepIntegration'),
        text: snapshotLoading || integrationLoading
          ? t('common.loading')
          : snapshotFailed || integration.isError || integrationUnknown
            ? t('acceptance.statusUnknown')
            : reportOnly
              ? t('acceptance.integrationNotRequired')
              : integrationConfirmed
                ? t('acceptance.statusIntegrated')
                : integrationRejected
                  ? t('acceptance.statusIntegrationRejected')
                  : integrationPrepared
                    ? t('acceptance.statusIntegrationPending')
                    : t('acceptance.statusIntegrationNotPrepared'),
        tone: snapshotLoading || integrationLoading ? 'neutral' : snapshotFailed || integration.isError || integrationUnknown ? 'warning' : reportOnly ? 'neutral' : integrationConfirmed ? 'positive' : integrationRejected ? 'negative' : integrationPrepared ? 'warning' : 'neutral',
        testId: integrationRejected && !snapshotFailed && !integration.isError ? 'acceptance-integration-rejected' : undefined,
      },
      {
        key: 'acceptance', title: t('acceptance.stepAcceptance'),
        text: completionConflict
          ? t('acceptance.statusConflict')
          : acceptanceLoading
            ? t('common.loading')
            : acceptanceUnknown
              ? t('acceptance.statusUnknown')
              : completionConfirmed
                ? t('acceptance.statusCompleted')
                : deliveryComplete
                  ? t('acceptance.statusCompletionUnconfirmed')
                  : completionKnown
                    ? t('acceptance.statusIntegrationPending')
                    : t('acceptance.noCompletionReceipt'),
        tone: completionConflict ? 'negative' : acceptanceUnknown ? 'warning' : acceptanceLoading ? 'neutral' : completionConfirmed ? 'positive' : deliveryComplete || completionKnown ? 'warning' : 'neutral',
        testId: completionConflict ? 'acceptance-completion-conflict' : undefined,
      },
    ];

    const currentVersionText = collaboration?.candidate?.current === false
      ? t('acceptance.historyVersions')
      : t('acceptance.currentVersion');
    const selectedReviewState = asText(reviewObject?.state);
    const selectedReviewStatus = selectedReviewState === 'open' || selectedReviewState === 'approved'
      || selectedReviewState === 'rejected' || selectedReviewState === 'invalidated' ? selectedReviewState : 'unknown';
    const selectedReviewLabel = t(`outcomeReviews.states.${selectedReviewStatus}`);
    const acceptanceCriteria = snapshotData?.visible_contract?.acceptance ?? snapshotData?.published_contract?.acceptance;
    const sourceData = source.data?.result.data;
    const sourceUpdated = sourceData?.source_synchronized === true
      && sourceData?.cursor_current === true && sourceData?.projection_current === true;
    const verificationRows = verification.map((entry, index) => ({
      key: index,
      check: checkLabel(at(entry, ['check']), { language, position: index + 1 }).text,
      outcome: at(entry, ['outcome']) === 'passed' ? t('acceptance.statusPassed') : at(entry, ['outcome']) === 'failed' ? t('acceptance.statusFailed') : t('acceptance.statusUnknown'),
      version: currentVersionText,
    }));

    const selectedTitle = workLabel(selectedWork.title, { language, recordedTitle: true }).text;

    detailView = (
      <section className="acceptance-detail" data-testid="acceptance-detail">
        <div className="acceptance-detail-bar">
          <Typography.Text strong>{t('projectAcceptance.detailHeader')}</Typography.Text>
          <div className="acceptance-detail-actions">
            <button
              type="button"
              className="acceptance-link-btn"
              data-testid="acceptance-detail-task-link"
              onClick={() => navigate(taskPath(project, selectedWork.key))}
            >
              {t('projectAcceptance.detailOpenTask')}
              <DisplayIcon icon={ArrowRightIcon} />
            </button>
            <button
              type="button"
              className="acceptance-link-btn"
              data-testid="acceptance-detail-back"
              onClick={closeDetail}
            >
              <DisplayIcon icon={ArrowLeftIcon} />
              {t('projectAcceptance.detailBack')}
            </button>
          </div>
        </div>

        <Card size="small" title={selectedTitle}>
          <DeliverySteps steps={steps} testId="acceptance-evidence-chain" />
          {integrationId ? (
            <Space wrap data-testid="acceptance-times">
              <Typography.Text type="secondary">{t('acceptance.externalObserved')} <RecordedTime value={observedAt} /></Typography.Text>
              <Typography.Text type="secondary">{t('acceptance.awrRecorded')} <RecordedTime value={recordedAt} /></Typography.Text>
            </Space>
          ) : null}
          {integrationIsHistorical ? (
            <Typography.Text type="secondary" data-testid="acceptance-integration-history">{t('acceptance.integrationHistorical')}</Typography.Text>
          ) : null}
          {acceptanceCriteria && acceptanceCriteria.length > 0 ? (
            <div style={{ marginTop: token.margin }} data-testid="outcome-acceptance-requirements">
              <Typography.Text strong>{t('acceptance.currentRequirements')}</Typography.Text>
              <List size="small" dataSource={acceptanceCriteria} renderItem={(criterion) => <List.Item>{value(criterion)}</List.Item>} />
            </div>
          ) : null}
        </Card>

        <OutcomeReviewHistory
          key={`${project}:${workId}`}
          project={project}
          workId={workId}
          stream={selectedWork.workstream_id}
          currentRound={currentRound}
          selectedRound={selectedRound ?? undefined}
          onSelect={selectRound}
          revoked={revoked}
        />

        {verificationRows.length > 0 ? (
          <Card size="small" title={t('acceptance.verificationTitle')} data-testid="acceptance-verification">
            <Table
              size="small"
              rowKey="key"
              pagination={false}
              dataSource={verificationRows}
              columns={[
                { title: t('acceptance.verificationContent'), dataIndex: 'check' },
                { title: t('acceptance.verificationResult'), dataIndex: 'outcome', render: (text) => <Tag color={text === t('acceptance.statusPassed') ? 'success' : text === t('acceptance.statusFailed') ? 'error' : 'default'}>{text}</Tag> },
                { title: t('acceptance.boundVersion'), dataIndex: 'version' },
              ]}
            />
            <Typography.Text type="secondary">{t('acceptance.verificationNote')}</Typography.Text>
          </Card>
        ) : null}

        <Card size="small" title={t('acceptance.reviewChanges')} data-testid={`acceptance-review-${selectedRound ?? 'none'}`}>
          <Space direction="vertical" size={8} style={{ width: '100%' }}>
            <Space style={{ display: 'flex', justifyContent: 'space-between', width: '100%' }}>
              <Typography.Text>{t('outcomeReviews.selected')}</Typography.Text>
              <Typography.Text>{selectedRound === currentRound ? currentVersionText : t('acceptance.historyVersions')} · {selectedReviewLabel}</Typography.Text>
            </Space>
            {selectedRound && selectedRound !== currentRound ? <Typography.Text type="secondary">{t('outcomeReviews.browsingEarlier')}</Typography.Text> : null}
            {reviewCandidateCurrent == null ? (
              <Typography.Text type="secondary" data-testid="acceptance-review-currentness">{missing}</Typography.Text>
            ) : null}
            {reviewIsHistorical ? <Tag color="warning">{t('acceptance.historical')}</Tag> : null}
            <Typography.Text type="secondary">{t('acceptance.reviewChangeNote')}</Typography.Text>
            {selectedRound === currentRound && completionKnown && at(completionObject, ['approved_by', 'independence_kind']) ? (
              <Typography.Text type="secondary" data-testid="completion-independence">
                {completionReviewLabels[completionReviewKind ?? ''] ?? t('outcomeReviews.methodUnknown')}
              </Typography.Text>
            ) : null}
            {at(reviewObject, ['evidence_provenance', 'author_self_report']) === true ? <Typography.Text type="secondary">{t('acceptance.selfReport')}</Typography.Text> : null}
            {at(reviewObject, ['evidence_provenance', 'execution_consistency', 'independent_verification']) === false ? <Typography.Text type="secondary">{t('acceptance.nonIndependent')}</Typography.Text> : null}
            {review.isError ? readError('acceptance-review-explanation-error')
              : selectedRound && review.isPending ? loadingText
                : reviewObject ? (
                  <>
                    {reviewDecisions.map((decision, index) => (
                      <Space direction="vertical" size={4} key={asText(at(decision, ['decision_id'])) ?? index} data-testid="review-explanation" style={{ width: '100%' }}>
                        {typeof at(decision, ['human_approval']) === 'boolean' ? (
                          <Typography.Text type="secondary" data-testid="decision-human-approval">
                            {t(at(decision, ['human_approval']) === true ? 'acceptance.humanApproved' : 'acceptance.noHumanApproval')}
                          </Typography.Text>
                        ) : null}
                        <Typography.Text strong>{t('acceptance.reviewReason')}</Typography.Text>
                        <Typography.Paragraph ellipsis={{ rows: 4, expandable: true, symbol: t('acceptance.readMore') }} style={{ marginBottom: 0, whiteSpace: 'pre-wrap', overflowWrap: 'anywhere' }}>
                          {value(at(decision, ['reason']))}
                        </Typography.Paragraph>
                      </Space>
                    ))}
                    {reviewFindings.map((finding, index) => (
                      <Space key={asText(at(finding, ['finding_key'])) ?? index} direction="vertical" size={4} style={{ width: '100%', overflowWrap: 'anywhere' }} data-testid="review-finding">
                        <Space wrap>
                          <Typography.Text strong>{t('acceptance.reviewFinding')}</Typography.Text>
                          <Tag color={at(finding, ['severity']) === 'blocking' && at(finding, ['state']) === 'open' ? 'error' : 'default'}>{value(at(finding, ['severity']), 'severity')}</Tag>
                          <Tag>{at(finding, ['state']) === 'open' ? t('acceptance.findingOpen') : value(at(finding, ['state']), 'state')}</Tag>
                        </Space>
                        <Typography.Text>{value(at(finding, ['summary']))}</Typography.Text>
                        {asText(at(finding, ['criterion'])) ? <Typography.Text type="secondary">{t('acceptance.findingCriterion')}: {value(at(finding, ['criterion']))}</Typography.Text> : null}
                        {(['given', 'when', 'then'] as const).map((part) => (
                          asText(at(finding, ['counterexample', part])) ? (
                            <Typography.Text key={part} type="secondary">
                              {t(`acceptance.counterexample.${part}`)}: {value(at(finding, ['counterexample', part]))}
                            </Typography.Text>
                          ) : null
                        ))}
                      </Space>
                    ))}
                    {reviewDecisions.length === 0 && reviewFindings.length === 0 ? (
                      <Typography.Text type="secondary">{t('acceptance.noReviewExplanation')}</Typography.Text>
                    ) : null}
                  </>
                ) : null}
          </Space>
        </Card>

        {requiredDependencies.length > 0 ? (
          <Card size="small" title={t('acceptance.requiredWork')}>
            <List
              size="small"
              dataSource={requiredDependencies}
              renderItem={(dependency) => {
                const key = asText(dependency);
                const related = works.find((work) => work.key === key);
                return (
                  <List.Item>
                    <Typography.Text>{related ? workLabel(related.title, { language, recordedTitle: true }).text : t('acceptance.relatedWork')}</Typography.Text>
                  </List.Item>
                );
              }}
            />
          </Card>
        ) : null}

        {artifacts.length > 0 ? (
          <Card size="small" title={t('acceptance.artifacts')}>
            {artifacts.map((artifact, index) => {
              const artifactId = asText(at(artifact, ['query', 'artifact_id']));
              return (
                <div key={artifactId ?? index} data-testid={`acceptance-artifact-${artifactId}`}>
                  <ArtifactReader project={project} artifact={artifact} revoked={revoked} />
                </div>
              );
            })}
          </Card>
        ) : null}

        {collaboration?.source_state ? (
          <Card size="small" title={t('acceptance.projectRecords')} data-testid="outcome-project-records">
            <Space direction="vertical" size={8}>
              <Typography.Text>{source.isError ? t('acceptance.readFailed') : source.isPending ? t('common.loading')
                : sourceUpdated ? t('acceptance.recordsUpdated') : sourceData?.source_synchronized === false
                  ? t('acceptance.recordsPending') : t('acceptance.notObserved')}</Typography.Text>
              <Typography.Text type="secondary">{t('acceptance.recordsNote')}</Typography.Text>
            </Space>
          </Card>
        ) : null}

        <OutcomeActivity key={`activity:${project}:${workId}`} project={project} workId={workId} revoked={revoked} />

        {review.isError ? readError('acceptance-review-error') : null}
        {selectedRound !== currentRound && currentReview.isError ? readError('acceptance-current-review-error') : null}
        {evidence.isError ? readError('acceptance-evidence-error') : null}
        {execution.isError ? readError('acceptance-execution-error') : null}
        {claim.isError ? readError('acceptance-claim-error') : null}
        {completion.isError ? readError('acceptance-completion-error') : null}
        {integration.isError ? readError('acceptance-integration-error') : null}
        {source.isError ? readError('acceptance-source-error') : null}
        {delivery.isError ? readError('acceptance-delivery-error') : null}
        {neutral.isError ? readError('acceptance-connectors-error') : null}
        {exports.isError ? readError('acceptance-exports-error') : null}
        {selectedSnapshot?.isError ? readError('acceptance-snapshot-error') : null}
        {selectedSnapshot?.isPending || reviewLoading || (Boolean(evidenceId) && evidence.isPending) || (Boolean(executionId) && execution.isPending) || (Boolean(claimId) && claim.isPending) || completionLoading || integrationLoading || delivery.isPending || neutral.isPending || submission.isPending || (Boolean(collaboration?.source_state) && source.isPending) || exports.isPending ? loadingText : null}
        {!acceptanceLoading && !acceptanceUnknown && !selectedRound && !selectedCompletionId ? emptyText : null}
      </section>
    );
  }

  const { counts, coverage } = derived;
  const activeCountSlice = activeDetail === 'all' ? counts.total : counts[activeDetail];
  const visibleRows = activeCountSlice.items;
  const isPartial = !coverage.allSnapshotsFailed && (
    coverage.snapshotsFailed > 0
    || coverage.roundsFailed > 0
    || coverage.overviewPartial
  );
  const showUnknown = !coverage.snapshotsLoading && !coverage.allSnapshotsFailed && !isPartial && coverage.hasUnknownFacts;

  const tableRows: DataTableRow[] = visibleRows.map((row) => {
    const titleText = workLabel(row.rawTitle, { language, recordedTitle: true }).text;
    const submitterText = row.submitterName
      ? t('projectAcceptance.submittedBy', { name: row.submitterName })
      : t('projectAcceptance.submitterUnknown');
    const reviewMeta = formatReviewMeta(row, t, language);
    const reviewStateLabel = t(`projectAcceptance.roundStates.${row.latestReview.state}`);

    return {
      key: row.workKey,
      testId: `acceptance-work-${row.workKey}`,
      highlighted: row.highlighted,
      onOpen: () => selectWork(row.workKey),
      cells: [
        <div>
          <div className="acceptance-task-title">{titleText}</div>
          <div className="acceptance-sm">{submitterText}</div>
        </div>,
        formatVersion(row, t),
        row.checks.length > 0 ? (
          <div className="acceptance-chips">
            {row.checks.map((check, index) => (
              <span className="acceptance-chip" key={`${row.workKey}-check-${index}`}>
                <span className="acceptance-check-icon" data-outcome={check.outcome}>
                  {check.outcome === 'failed'
                    ? <CircleXIcon size={12} strokeWidth={1.5} aria-hidden="true" />
                    : check.outcome === 'unknown'
                      ? <ClockIcon size={12} strokeWidth={1.5} aria-hidden="true" />
                      : <CheckIcon size={12} strokeWidth={1.5} aria-hidden="true" />}
                </span>
                {checkLabel(check.rawCheck, { language, position: index + 1 }).text}
              </span>
            ))}
          </div>
        ) : (
          <span className="acceptance-sm">{t('projectAcceptance.noChecks')}</span>
        ),
        <div>
          <span className="obs-tag" data-tone={reviewTagTone(row.latestReview.state)}>
            <StatusLamp lamp={reviewTagLamp(row.latestReview.state)} />
            {reviewStateLabel}
          </span>
          {reviewMeta ? <div className="acceptance-review-meta">{reviewMeta}</div> : null}
        </div>,
        <span className="acceptance-sm">{t(`projectAcceptance.integration.${row.integration}`)}</span>,
      ],
    };
  });

  const renderLeadCount = (filter: AcceptanceDetailFilter, count: number | null, label: string) => {
    if (count === null) {
      return <span data-testid={`acceptance-summary-${filter}`}>{label}</span>;
    }
    return (
      <button
        type="button"
        className="acceptance-lead-link"
        data-testid={`acceptance-summary-${filter}`}
        aria-pressed={activeDetail === filter}
        onClick={() => selectFilter(filter)}
      >
        {label}
      </button>
    );
  };

  const sep = t('projectAcceptance.leadSeparator');
  const sectionCount = activeCountSlice.count === null ? (
    <span data-testid="acceptance-section-count">—</span>
  ) : (
    <button
      type="button"
      className="acceptance-count-link"
      data-testid="acceptance-section-count"
      onClick={() => selectFilter(activeDetail)}
    >
      {activeCountSlice.count}
    </button>
  );

  const freshness = coverage.allSnapshotsFailed ? FAILED_FRESHNESS : derived.freshness;

  return (
    <div className="acceptance-page" data-testid="acceptance-page">
      {coverage.allSnapshotsFailed ? (
        <Notice
          tone="warning"
          title={t('projectAcceptance.failedTitle')}
          extra={<Button size="small" onClick={retryReads}>{t('projectAcceptance.retry')}</Button>}
          className="acceptance-notice"
        >
          <span data-testid="acceptance-failed">{t('projectAcceptance.failedDescription')}</span>
        </Notice>
      ) : null}

      {isPartial ? (
        <Notice
          tone="warning"
          title={t('projectAcceptance.partialTitle')}
          extra={<Button size="small" onClick={retryReads}>{t('projectAcceptance.retry')}</Button>}
          className="acceptance-notice"
        >
          <span data-testid="acceptance-partial">{t('projectAcceptance.partialDescription')}</span>
        </Notice>
      ) : null}

      {showUnknown ? (
        <Notice
          tone="info"
          title={t('projectAcceptance.unknownTitle')}
          className="acceptance-notice"
        >
          <span data-testid="acceptance-unknown">{t('projectAcceptance.unknownDescription')}</span>
        </Notice>
      ) : null}

      <p className="acceptance-lead" data-testid="acceptance-lead">
        {coverage.snapshotsLoading ? (
          <span data-testid="acceptance-loading">{t('projectAcceptance.loading')}</span>
        ) : (
          <>
            {renderLeadCount(
              'all',
              counts.total.count,
              counts.total.count === null
                ? t('projectAcceptance.leadTotalUnknown')
                : t('projectAcceptance.leadTotal', { count: counts.total.count }),
            )}
            <span className="acceptance-lead-rest">
              {sep}
              {renderLeadCount('approved', counts.approved.count, t('projectAcceptance.leadApproved', { count: counts.approved.count ?? '—' }))}
              {sep}
              {renderLeadCount('open', counts.open.count, t('projectAcceptance.leadOpen', { count: counts.open.count ?? '—' }))}
              {sep}
              {renderLeadCount('rejected', counts.rejected.count, t('projectAcceptance.leadRejected', { count: counts.rejected.count ?? '—' }))}
            </span>
          </>
        )}
      </p>

      <Section
        className="acceptance-section"
        title={t('projectAcceptance.sectionTitle')}
        count={coverage.snapshotsLoading ? undefined : sectionCount}
        extra={activeDetail !== 'all' ? (
          <button
            type="button"
            className="acceptance-link-btn"
            data-testid="acceptance-clear-filter"
            onClick={() => selectFilter('all')}
          >
            {t('projectAcceptance.showAll')}
          </button>
        ) : undefined}
      >
        {coverage.snapshotsLoading && tableRows.length === 0 ? (
          <div className="acceptance-empty" data-testid="acceptance-table-loading">
            {t('projectAcceptance.loading')}
          </div>
        ) : tableRows.length > 0 ? (
          <div className="acceptance-table" data-testid="acceptance-table" data-filter={activeDetail}>
            <DataTable
              columns={[
                { key: 'task', title: t('projectAcceptance.columns.task'), width: '26%' },
                { key: 'version', title: t('projectAcceptance.columns.version'), width: 80 },
                { key: 'checks', title: t('projectAcceptance.columns.checks') },
                { key: 'latestReview', title: t('projectAcceptance.columns.latestReview'), width: '26%' },
                { key: 'integration', title: t('projectAcceptance.columns.integration'), width: 160, hideOnSmall: true },
              ]}
              rows={tableRows}
              navigationWidth={36}
              stickyNavigation={false}
            />
          </div>
        ) : (
          <div className="acceptance-empty" data-testid={activeDetail === 'all' ? 'acceptance-empty' : 'acceptance-filter-empty'}>
            {activeDetail === 'all' ? t('projectAcceptance.emptyOutcomes') : t('projectAcceptance.emptyFilter')}
          </div>
        )}

        {works.length > PAGE_SIZE ? (
          <div className="acceptance-pagination" data-testid="acceptance-pagination">
            <Pagination
              current={page}
              total={works.length}
              pageSize={PAGE_SIZE}
              showSizeChanger={false}
              hideOnSinglePage
              onChange={(nextPage) => {
                const next = new URLSearchParams(searchParams);
                if (nextPage > 1) next.set('page', String(nextPage));
                else next.delete('page');
                next.delete('work');
                next.delete('round');
                setSearchParams(next);
              }}
            />
          </div>
        ) : null}

        <div className="acceptance-pnote" data-testid="acceptance-independent-note">
          {t('projectAcceptance.independentNote')}
        </div>
      </Section>

      {detailView}

      <footer className="acceptance-coverage-footer" data-testid="acceptance-coverage">
        <div data-testid="acceptance-from-works">
          {t('projectAcceptance.coverageFromWorks', { count: coverage.snapshotsLoaded, total: works.length })}
        </div>
        {freshness ? <FreshnessBadge freshness={freshness} /> : null}
      </footer>
    </div>
  );
}
