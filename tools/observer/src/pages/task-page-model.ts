import type { TFunction } from 'i18next';
import type { AcceptanceItemStatus } from '../components/display/AcceptanceList';
import {
  compactTimeText,
  deriveOwner,
  deriveWorkState,
  describeProgressReport,
  documentLabel,
  foldOperations,
  groupWorks,
  operationWord,
  readableNarrative,
  workLabel,
  checkLabel,
} from '../core/observation';
import { readsAsName } from '../core/observation/owner';
import { hasObservationValue, statementText } from '../core/observation/values';
import { deriveOverview } from '../core/overview';
import type {
  DerivedTask,
  DerivedTaskStatus,
  DerivedWait,
  OverviewDerivation,
  WaitKind,
} from '../core/overview';
import { humanText } from '../i18n/human-language';
import { mergeProjectFreshness } from '../components/projects/project-overview-model';
import type {
  AcceptanceEvidenceItem,
  AcceptanceEvidenceStatus,
  DependencyObservation,
  HandoffListData,
  ProjectDocument,
  ProjectGoalRecord,
  ProjectLessonRecord,
  ReviewData,
  ReviewRoundsData,
  TeamActivityItem,
  TeamFreshness,
  TeamOverviewModelResponse,
  TeamOverviewResponse,
  TeamWork,
  WorkSnapshotData,
} from '../lib/types';

export interface TaskCriterionView {
  key: string;
  title: string;
  evidence?: string;
  status: AcceptanceItemStatus | null;
}

export interface TaskReportView {
  summary: string | null;
  reporterName: string | null;
  reportedAtText: string | null;
  completed: string[];
  completedMore: number;
  blockers: string[];
  blockersMore: number;
  tests: Array<{ name: string; outcome: string; tone: 'positive' | 'negative' | 'warning' | 'neutral' | 'accent' }>;
  artifacts: string[];
  artifactsMore: number;
  contractNote: string | null;
  stale: boolean;
}

export interface TaskDeliveryRoundView {
  key: string;
  roundIndex: number;
  verdict: 'open' | 'approved' | 'rejected' | 'invalidated' | 'unknown';
  reviewerName: string;
  reviewerId: string | null;
  reviewerRole: 'independent_reviewer' | 'simulated_member_reviewer' | 'author_reviewer' | 'agent_reviewer' | 'unknown';
  reason: string | null;
  waitDays: number | null;
}

export interface TaskDeliveryView {
  versionLabel: string | null;
  roundCount: number;
  checks: Array<{ name: string; outcome: 'passed' | 'failed' | 'unknown' }>;
  integrationKind: 'integrated' | 'waitingReview' | 'notMerged' | 'pending' | 'unknown';
  rounds: TaskDeliveryRoundView[];
}

export interface TaskTimelineViewItem {
  key: string;
  timeText: string;
  who: string;
  what: string;
  next: string | null;
  note: string | null;
  tone: 'default' | 'current' | 'positive' | 'negative';
  reporterText: boolean;
}

export interface TaskBlockerWaitView {
  kind: WaitKind | 'reported';
  waitingOnName: string;
  waitingOnPersonId: string | null;
  waitingOnExternal: boolean;
  days: number | null;
  what: string | null;
  behindCount: number | null;
  behindItems: Array<{ id: string; title: string }>;
  inboxDetailKey: string | null;
}

export interface TaskBlockerAssignmentView {
  fromName: string;
  toName: string;
  toPersonId: string | null;
  atText: string | null;
  inboxDetailKey: string | null;
}

export interface TaskWhyView {
  goalId: string | null;
  goalTitle: string | null;
  projectTitle: string;
  workstreamTitle: string | null;
  weekCommitment: 'in' | 'out' | null;
}

export interface TaskDependencyItemView {
  key: string;
  direction: 'upstream' | 'downstream';
  taskId: string;
  title: string;
  meta: string;
}

export interface TaskDocItemView {
  key: string;
  docId: string | null;
  title: string;
  meta: string;
}

export interface TaskLessonItemView {
  key: string;
  lessonId: string;
  title: string;
  stage: 'candidate' | 'project' | 'group' | 'unknown';
  authorName: string | null;
}

export interface TaskPageModel {
  found: boolean;
  title: string;
  summaryText: string;
  status: DerivedTaskStatus;
  liveRunning: boolean;
  ownerName: string | null;
  ownerPersonId: string | null;
  agentName: string | null;
  nextStepText: string;
  nextStepReporterText: boolean;
  acceptance: {
    hasPerCriterionEvidence: boolean;
    items: TaskCriterionView[];
  };
  report: TaskReportView | null;
  delivery: TaskDeliveryView | null;
  timeline: TaskTimelineViewItem[];
  blocker:
    | { kind: 'wait'; wait: TaskBlockerWaitView }
    | { kind: 'assignment'; assignment: TaskBlockerAssignmentView }
    | { kind: 'none' }
    | { kind: 'unknown' };
  why: TaskWhyView;
  dependencies: TaskDependencyItemView[];
  documents: TaskDocItemView[];
  lessons: TaskLessonItemView[];
  hasUnknownFacts: boolean;
  freshness: TeamFreshness | undefined;
}

export interface TaskPageDeriveInput {
  workId: string;
  projectKey: string;
  projectTitle: string;
  overview?: TeamOverviewResponse;
  model?: TeamOverviewModelResponse;
  snapshot?: WorkSnapshotData;
  snapshotFreshness?: TeamFreshness;
  reviewRounds?: ReviewRoundsData;
  reviewInspect?: ReviewData;
  dependencies?: DependencyObservation[];
  handoffs?: HandoffListData;
  activity?: TeamActivityItem[];
  documents?: ProjectDocument[];
  goals?: ProjectGoalRecord[];
  lessons?: ProjectLessonRecord[];
  language: string;
  nowUnixMs: number;
  timeZone: string;
  t: TFunction;
}

const DAY_MS = 86_400_000;

function safeName(value: unknown): string | null {
  if (typeof value !== 'string') return null;
  const trimmed = value.trim();
  return trimmed.length > 0 && readsAsName(trimmed) ? trimmed : null;
}

/** Accepts either a plain member/Agent name or a parenthesized attribution whose parts are both safe names (e.g. "Codex（陈宇的 Agent）"). */
function safeAttributionName(value: unknown): string | null {
  if (typeof value !== 'string') return null;
  const trimmed = value.trim();
  if (!trimmed) return null;
  if (readsAsName(trimmed)) return trimmed;
  const match = /^(.+?)（(.+?)）$/.exec(trimmed);
  if (match && readsAsName(match[1].trim()) && readsAsName(match[2].trim())) {
    return trimmed;
  }
  return null;
}

function asRecord(value: unknown): Record<string, unknown> | null {
  return value && typeof value === 'object' && !Array.isArray(value) ? value as Record<string, unknown> : null;
}

function cleanNarrative(value: unknown, language: string): string | null {
  const raw = statementText(value);
  if (!raw) return null;
  return readableNarrative(raw, language);
}

function mapAcceptanceEvidenceStatus(status: AcceptanceEvidenceStatus): AcceptanceItemStatus {
  if (status === 'supported') return 'supported';
  if (status === 'failed') return 'failed';
  if (status === 'waiting') return 'waiting';
  return 'notStarted';
}

function formatCriterionEvidence(item: AcceptanceEvidenceItem, language: string, t: TFunction): string | undefined {
  const checkNames = item.checks.map((c, i) => checkLabel(c.check, { language, position: i + 1 }).text);
  const passedChecks = item.checks
    .filter((c) => c.outcome === 'passed')
    .map((c, i) => checkLabel(c.check, { language, position: i + 1 }).text);
  const failedChecks = item.checks
    .filter((c) => c.outcome === 'failed')
    .map((c, i) => checkLabel(c.check, { language, position: i + 1 }).text);
  const sep = t('common.listSeparator');

  if (item.status === 'supported') {
    return passedChecks.length > 0
      ? t('taskPage.acceptance.evidence.supportedChecks', { checks: passedChecks.join(sep) })
      : t('taskPage.acceptance.evidence.supported');
  }
  if (item.status === 'failed') {
    if (item.reason === 'review_returned') return t('taskPage.acceptance.evidence.reviewReturned');
    if (failedChecks.length > 0) {
      return t('taskPage.acceptance.evidence.checkFailed', { checks: failedChecks.join(sep) });
    }
    return t('taskPage.acceptance.evidence.checkFailedGeneric');
  }
  if (item.status === 'waiting') {
    if (item.reason === 'awaiting_review_approval') {
      return passedChecks.length > 0
        ? t('taskPage.acceptance.evidence.awaitingReviewChecks', { checks: passedChecks.join(sep) })
        : t('taskPage.acceptance.evidence.awaitingReview');
    }
    if (item.reason === 'awaiting_current_candidate') {
      return t('taskPage.acceptance.evidence.awaitingCandidate');
    }
    return checkNames.length > 0
      ? t('taskPage.acceptance.evidence.awaitingChecksNamed', { checks: checkNames.join(sep) })
      : t('taskPage.acceptance.evidence.awaitingChecks');
  }
  if (item.status === 'no_bound_check') {
    return t('taskPage.acceptance.evidence.noBoundCheck');
  }
  return t('taskPage.acceptance.evidence.notStarted');
}

function taskTitleFor(
  id: string,
  overview: TeamOverviewResponse | undefined,
  derivation: OverviewDerivation | null,
  language: string,
  fallback?: string | null,
): string {
  const fromOverview = overview?.works.find((w) => w.key === id)?.title;
  const fromModel = derivation?.tasks.find((task) => task.id === id)?.title;
  const raw = fromOverview ?? fromModel ?? fallback ?? '';
  if (!raw) return humanText('workUnnamed', language);
  return workLabel(raw, { language, recordedTitle: true, fallback: humanText('workUnnamed', language) }).text;
}

function memberNameFor(
  personId: string | null | undefined,
  overview: TeamOverviewResponse | undefined,
  model: TeamOverviewModelResponse | undefined,
): string | null {
  if (!personId) return null;
  for (const member of model?.input.members.items ?? []) {
    if (member.personId === personId) {
      const name = safeName(member.displayName);
      if (name) return name;
    }
  }
  for (const member of overview?.members ?? []) {
    const idMatch = ['person_id', 'actor_id', 'id'].some((k) => member[k] === personId);
    if (idMatch) {
      const name = safeName(member.display_name ?? member.name);
      if (name) return name;
    }
  }
  return null;
}

function deriveStatusFallback(
  work: TeamWork | undefined,
  snapshot: WorkSnapshotData | undefined,
): DerivedTaskStatus {
  const runtimeState = snapshot?.runtime?.state ?? snapshot?.observation?.runtime?.state ?? work?.status ?? null;
  const progress = asRecord(snapshot?.observation?.progress);
  const phase = typeof progress?.phase === 'string' ? progress.phase : null;
  const reviewState = snapshot?.observation?.collaboration?.review?.state ?? null;
  const integrated = snapshot?.observation?.collaboration?.integration?.state === 'confirmed'
    || snapshot?.observation?.collaboration?.publication?.phase === 'confirmed';

  if (runtimeState === 'completed' && integrated) return 'verified';
  if (reviewState === 'rejected') return 'returned';
  if (reviewState === 'open') return 'review';
  if (phase === 'delivered' || phase === 'completed') return 'claimed';
  if (phase === 'waiting_for_answer') return 'stalled';
  if (phase === 'blocked') return 'waiting';
  if (phase === 'in_progress' || runtimeState === 'claimed') return 'running';
  if (phase === 'ready') return 'ready';
  if (phase === 'planned') return 'planned';
  return 'unknown';
}

function buildOneSentence(params: {
  status: DerivedTaskStatus;
  criteria: TaskCriterionView[];
  hasPerCriterionEvidence: boolean;
  delivery: TaskDeliveryView | null;
  wait: TaskBlockerWaitView | null;
  assignment: TaskBlockerAssignmentView | null;
  agentName: string | null;
  agentActive: boolean | null;
  upstreamProviderTitle: string | null;
  t: TFunction;
}): string {
  const {
    status, criteria, hasPerCriterionEvidence, delivery, wait, assignment,
    agentName, agentActive, upstreamProviderTitle, t,
  } = params;
  const total = criteria.length;
  const supported = criteria.filter((c) => c.status === 'supported').length;
  const waitingCriteria = criteria.filter((c) => c.status === 'waiting').length;

  switch (status) {
    case 'review': {
      const days = wait?.days ?? delivery?.rounds.find((r) => r.verdict === 'open')?.waitDays ?? null;
      if (days !== null && hasPerCriterionEvidence && total > 0 && waitingCriteria > 0) {
        return t('taskPage.summary.reviewWithCriteria', { days, total, supported, waiting: waitingCriteria });
      }
      if (days !== null) return t('taskPage.summary.reviewWithDays', { days });
      return t('taskPage.summary.review');
    }
    case 'verified': {
      const rounds = delivery?.rounds.length ?? 0;
      if (hasPerCriterionEvidence && total > 0) {
        return rounds > 1
          ? t('taskPage.summary.verifiedMultiRound', { total, rounds })
          : t('taskPage.summary.verifiedSingleRound', { total });
      }
      return t('taskPage.summary.verified');
    }
    case 'claimed':
      return t('taskPage.summary.claimed');
    case 'returned': {
      const latestRejected = [...(delivery?.rounds ?? [])].reverse().find((r) => r.verdict === 'rejected');
      const reason = latestRejected?.reason ?? null;
      if (reason && wait?.kind === 'decision' && wait.waitingOnName) {
        return t('taskPage.summary.returnedDecision', { reason, who: wait.waitingOnName });
      }
      if (reason) return t('taskPage.summary.returnedReason', { reason });
      return t('taskPage.summary.returned');
    }
    case 'running':
      return agentName ? t('taskPage.summary.runningAgent', { agent: agentName }) : t('taskPage.summary.running');
    case 'stalled':
      if (wait?.kind === 'decision' && wait.days !== null && wait.waitingOnName) {
        return t('taskPage.summary.stalledDecision', { days: wait.days, who: wait.waitingOnName });
      }
      if (wait?.days !== null && wait?.days !== undefined) {
        return t('taskPage.summary.stalledDays', { days: wait.days });
      }
      return t('taskPage.summary.stalled');
    case 'waiting':
      if (wait?.kind === 'external' && wait.waitingOnName && wait.what) {
        return wait.days !== null
          ? t('taskPage.summary.waitingExternalDays', { party: wait.waitingOnName, what: wait.what, days: wait.days })
          : t('taskPage.summary.waitingExternal', { party: wait.waitingOnName, what: wait.what });
      }
      if (upstreamProviderTitle) {
        return agentActive
          ? t('taskPage.summary.waitingDepActive', { provider: upstreamProviderTitle })
          : t('taskPage.summary.waitingDep', { provider: upstreamProviderTitle });
      }
      return t('taskPage.summary.waiting');
    case 'ready':
      if (assignment) {
        return t('taskPage.summary.readyAssigned', { from: assignment.fromName, to: assignment.toName });
      }
      return t('taskPage.summary.ready');
    case 'planned':
      if (upstreamProviderTitle) {
        return t('taskPage.summary.plannedDep', { provider: upstreamProviderTitle });
      }
      return t('taskPage.summary.planned');
    default:
      return t('taskPage.summary.unknown');
  }
}

function parseDocIdFromPath(path: string | null | undefined): string | null {
  if (!path) return null;
  const match = /([^/]+?)(?:\.[^./]+)?$/.exec(path);
  return match ? match[1] : null;
}

function inboxKeyForDecision(decisionId: string | null, responderId: string | null): string | null {
  if (!decisionId || !responderId) return null;
  return `decision:${decisionId}:${responderId}`;
}

function inboxKeyForAnswer(decisionId: string | null, responderId: string | null): string | null {
  if (!decisionId || !responderId) return null;
  return `answer:${decisionId}:${responderId}`;
}

export function deriveTaskPageModel(input: TaskPageDeriveInput): TaskPageModel {
  const {
    workId, projectKey, projectTitle, overview, model, snapshot, snapshotFreshness,
    reviewRounds, reviewInspect, dependencies, handoffs, activity, documents, goals, lessons,
    language, nowUnixMs, timeZone, t,
  } = input;

  const overviewWork = overview?.works.find((w) => w.key === workId);
  const derivation: OverviewDerivation | null = model?.input && model.input.options
    ? deriveOverview(model.input)
    : null;
  const refNowUnixMs = model?.input?.options?.nowUnixMs ?? nowUnixMs;
  const derivedTask: DerivedTask | undefined = derivation?.tasks.find((task) => task.id === workId);
  const modelRawTask = (model?.input.tasks.items ?? []).find((task) => task.id === workId);

  const found = Boolean(overviewWork || derivedTask || (snapshot && snapshot.work_id === workId));

  // Title
  const rawTitle = overviewWork?.title ?? derivedTask?.title ?? '';
  const title = rawTitle
    ? workLabel(rawTitle, { language, recordedTitle: true, fallback: humanText('workUnnamed', language) }).text
    : humanText('workUnnamed', language);

  // Status
  const status: DerivedTaskStatus = derivedTask?.status ?? deriveStatusFallback(overviewWork, snapshot);
  const liveRunning = status === 'running' || (status === 'waiting' && derivedTask?.agentActive === true);

  // Owner: strictly use explicit person IDs from the read; never treat an unspaced display name as an ID.
  const resp = asRecord(snapshot?.observation?.responsibility);
  const ownerPersonId = derivedTask?.owner?.id
    ?? (typeof resp?.owner_person_id === 'string' && resp.owner_person_id.trim().length > 0
      ? resp.owner_person_id.trim()
      : null);
  const derivedOwnerInfo = deriveOwner({
    rowOwner: overviewWork?.owner_person ?? null,
    snapshot,
    readState: snapshot ? 'ready' : 'loading',
  });
  const ownerName = safeName(derivedTask?.owner?.displayName)
    ?? safeName(resp?.owner_name)
    ?? memberNameFor(ownerPersonId, overview, model)
    ?? (derivedOwnerInfo.source !== 'none' ? safeName(derivedOwnerInfo.name) : null);

  // Agent
  const sessionObj = asRecord(snapshot?.observation?.session);
  const agentName = safeName(derivedTask?.agentName)
    ?? safeName(snapshot?.observation?.client?.product)
    ?? safeName(sessionObj?.actor_name)
    ?? safeName(overviewWork?.agent);

  // Next step (through deriveWorkState so we never leak guidance.action.op)
  const workState = deriveWorkState({
    language,
    readState: snapshot ? 'ready' : 'failed',
    snapshot,
    workNextStep: overviewWork?.next_step ?? snapshot?.next_step ?? null,
    workStatus: overviewWork?.status ?? null,
  });
  const nextStepText = workState.next.kind === 'reported'
    ? workState.next.text
    : status === 'verified'
      ? t('taskPage.nextNone')
      : workState.next.text;
  const nextStepReporterText = workState.next.kind === 'reported';

  // Acceptance criteria: strictly read verbatim PPD-040 acceptance_evidence object shape;
  // when absent or status === 'not_provided', fall back to the contract criteria with explicit "尚无逐条依据".
  const contract = snapshot?.visible_contract ?? snapshot?.published_contract;
  const rawEvidence = snapshot?.acceptance_evidence;
  const evidenceItems = rawEvidence
    && !Array.isArray(rawEvidence)
    && rawEvidence.status === 'provided'
    && Array.isArray(rawEvidence.items)
    && rawEvidence.items.length > 0
    ? rawEvidence.items
    : null;
  const hasPerCriterionEvidence = evidenceItems !== null;
  const criteria: TaskCriterionView[] = evidenceItems
    ? evidenceItems.map((item, idx) => ({
      key: `crit-${idx}`,
      title: cleanNarrative(item.criterion, language) ?? humanText('contentUnavailable', language),
      evidence: formatCriterionEvidence(item, language, t),
      status: mapAcceptanceEvidenceStatus(item.status),
    }))
    : (contract?.acceptance ?? []).flatMap((raw, idx): TaskCriterionView[] => {
      if (!hasObservationValue(raw)) return [];
      const text = cleanNarrative(raw, language);
      if (!text) return [];
      return [{ key: `crit-${idx}`, title: text, status: null }];
    });

  // Latest progress report (using describeProgressReport for full verbatim-sample guard!)
  const progressObj = asRecord(snapshot?.observation?.progress);
  const progressResult = describeProgressReport(progressObj, {
    language,
    finished: status === 'verified' || workState.lastReport.finished,
  });
  let report: TaskReportView | null = null;
  if (progressResult.kind === 'reported') {
    const rawOriginName = statementText(asRecord(progressObj?.origin)?.actor_name);
    const reporterName = safeAttributionName(rawOriginName)
      ?? humanText('reporterUnnamed', language);
    const reportedAtMs = typeof progressObj?.reported_at_unix_ms === 'number' && Number.isFinite(progressObj.reported_at_unix_ms)
      ? progressObj.reported_at_unix_ms
      : null;
    const reportedAtText = reportedAtMs !== null
      ? compactTimeText(reportedAtMs, { language, nowUnixMs, timeZone })
      : null;
    report = {
      summary: progressResult.summary,
      reporterName,
      reportedAtText,
      completed: progressResult.completed.shown,
      completedMore: progressResult.completed.more,
      blockers: progressResult.blockers.shown,
      blockersMore: progressResult.blockers.more,
      tests: progressResult.tests.shown.map((test) => ({
        name: test.name,
        outcome: test.tone === 'positive'
          ? t('taskPage.report.passedOutcome')
          : test.tone === 'negative'
            ? t('taskPage.report.failedOutcome')
            : test.outcome,
        tone: test.tone,
      })),
      artifacts: progressResult.artifacts.shown.map((a) => a.text),
      artifactsMore: progressResult.artifacts.more,
      contractNote: progressResult.contractNote,
      stale: progressObj?.stale === true && status !== 'verified',
    };
  }

  // Delivery & Review
  const collab = snapshot?.observation?.collaboration ?? null;
  const modelRounds = (model?.input.reviewRounds.items ?? []).filter((r) => r.taskId === workId);
  const apiRounds = reviewRounds?.items ?? [];
  const hasDelivery = Boolean(
    collab?.delivery_required
    || collab?.candidate
    || collab?.review
    || collab?.integration
    || apiRounds.length > 0
    || modelRounds.length > 0,
  );
  let delivery: TaskDeliveryView | null = null;
  if (hasDelivery) {
    const checks = (collab?.verification ?? []).map((entry, idx) => {
      const outcome: 'passed' | 'failed' | 'unknown' = entry.outcome === 'passed' || entry.outcome === 'pass'
        ? 'passed'
        : entry.outcome === 'failed' || entry.outcome === 'fail'
          ? 'failed'
          : 'unknown';
      return {
        name: checkLabel(entry.check, { language, position: idx + 1 }).text,
        outcome,
      };
    });

    const indepReviewerObj = asRecord(resp?.independent_reviewer);
    const fallbackReviewerId = typeof indepReviewerObj?.person_id === 'string' ? indepReviewerObj.person_id : null;
    const fallbackReviewerName = safeName(indepReviewerObj?.display_name) ?? memberNameFor(fallbackReviewerId, overview, model);

    // Build rounds from apiRounds when available, else modelRounds, else collab.review
    const sourceRounds: TaskDeliveryRoundView[] = [];
    if (apiRounds.length > 0) {
      for (const round of apiRounds) {
        const matchingModelRound = modelRounds.find((mr) => mr.id === round.round_id || mr.roundIndex === round.round_index);
        const inspectDecision = reviewInspect?.review && round.round_id === collab?.review?.id
          ? reviewInspect.review.decisions.at(-1)
          : undefined;
        const reviewerId = round.reviewer_person_id ?? matchingModelRound?.reviewerPersonId ?? fallbackReviewerId;
        const reviewerName = memberNameFor(reviewerId, overview, model)
          ?? (reviewerId === fallbackReviewerId ? fallbackReviewerName : null)
          ?? t('taskPage.delivery.reviewerFallback');
        const rawRole = round.reviewer_role
          ?? (inspectDecision?.independence_kind === 'simulated_member_independent' ? 'independent_reviewer' : null)
          ?? (matchingModelRound?.independent === true ? 'independent_reviewer' : null)
          ?? (fallbackReviewerId && reviewerId === fallbackReviewerId ? 'independent_reviewer' : null);
        const reviewerRole: TaskDeliveryRoundView['reviewerRole'] =
          rawRole === 'independent_reviewer' || rawRole === 'simulated_member_reviewer'
            || rawRole === 'author_reviewer' || rawRole === 'agent_reviewer'
            ? rawRole
            : 'unknown';
        const verdict: TaskDeliveryRoundView['verdict'] =
          round.state === 'open' || round.state === 'approved' || round.state === 'rejected' || round.state === 'invalidated'
            ? round.state
            : 'unknown';
        const openedAt = matchingModelRound?.openedAtUnixMs ?? null;
        const waitDays = verdict === 'open' && typeof openedAt === 'number' && Number.isFinite(openedAt) && openedAt <= refNowUnixMs
          ? Math.floor((refNowUnixMs - openedAt) / DAY_MS)
          : null;
        const reason = round.reason_first_line ? cleanNarrative(round.reason_first_line, language) : null;
        sourceRounds.push({
          key: round.round_id,
          roundIndex: round.round_index,
          verdict,
          reviewerName,
          reviewerId,
          reviewerRole,
          reason,
          waitDays,
        });
      }
    } else if (modelRounds.length > 0) {
      for (const mr of modelRounds) {
        const reviewerId = mr.reviewerPersonId ?? fallbackReviewerId;
        const reviewerName = memberNameFor(reviewerId, overview, model)
          ?? (reviewerId === fallbackReviewerId ? fallbackReviewerName : null)
          ?? t('taskPage.delivery.reviewerFallback');
        const verdict: TaskDeliveryRoundView['verdict'] =
          mr.state === 'open' || mr.state === 'approved' || mr.state === 'rejected' || mr.state === 'invalidated'
            ? mr.state
            : 'unknown';
        const waitDays = verdict === 'open' && typeof mr.openedAtUnixMs === 'number' && Number.isFinite(mr.openedAtUnixMs) && mr.openedAtUnixMs <= refNowUnixMs
          ? Math.floor((refNowUnixMs - mr.openedAtUnixMs) / DAY_MS)
          : null;
        sourceRounds.push({
          key: mr.id ?? `round-${mr.roundIndex ?? sourceRounds.length + 1}`,
          roundIndex: mr.roundIndex ?? sourceRounds.length + 1,
          verdict,
          reviewerName,
          reviewerId,
          reviewerRole: mr.independent === true ? 'independent_reviewer' : 'unknown',
          reason: null,
          waitDays,
        });
      }
    } else if (collab?.review) {
      const state = collab.review.state;
      const verdict: TaskDeliveryRoundView['verdict'] =
        state === 'open' || state === 'approved' || state === 'rejected' || state === 'invalidated'
          ? state
          : 'unknown';
      const decision = reviewInspect?.review?.decisions.at(-1);
      sourceRounds.push({
        key: collab.review.id ?? 'round-1',
        roundIndex: 1,
        verdict,
        reviewerName: fallbackReviewerName ?? t('taskPage.delivery.reviewerFallback'),
        reviewerId: fallbackReviewerId,
        reviewerRole: decision?.independence_kind === 'simulated_member_independent' || fallbackReviewerId ? 'independent_reviewer' : 'unknown',
        reason: null,
        waitDays: null,
      });
    }

    const latestVerdict = sourceRounds.at(-1)?.verdict ?? null;
    const integrated = collab?.integration?.state === 'confirmed'
      || collab?.publication?.phase === 'confirmed'
      || modelRawTask?.integrated === true;
    const integrationKind: TaskDeliveryView['integrationKind'] = integrated
      ? 'integrated'
      : latestVerdict === 'rejected'
        ? 'notMerged'
        : latestVerdict === 'open'
          ? 'waitingReview'
          : collab?.integration?.state === 'dispatched' || collab?.integration?.state === 'prepared'
            ? 'pending'
            : sourceRounds.length > 0
              ? 'notMerged'
              : 'unknown';

    // No current Team read records a human-facing delivery version label; never guess from review round count.
    const versionLabel: string | null = null;

    delivery = {
      versionLabel,
      roundCount: sourceRounds.length,
      checks,
      integrationKind,
      rounds: sourceRounds,
    };
  }

  // Timeline: built exclusively from real audit.development activity for this task, sorted newest-first by timestamp.
  const timeline: TaskTimelineViewItem[] = [];
  const taskActivity = (activity ?? []).filter((item) => item.work_id == null || item.work_id === workId);
  if (taskActivity.length > 0) {
    const folded = foldOperations(taskActivity, {
      language,
      gapThresholdMs: 30 * 60 * 1000,
      order: 'newest-first',
    });
    for (const [idx, event] of folded.uniqueEvents.entries()) {
      const hasFiniteTime = typeof event.created_at_unix_ms === 'number' && Number.isFinite(event.created_at_unix_ms);
      const timeText = hasFiniteTime
        ? compactTimeText(event.created_at_unix_ms, { language, nowUnixMs, timeZone })
        : humanText('notProvided', language);
      const actorBase = safeAttributionName(event.actor?.display_name)
        ?? memberNameFor(event.actor?.person_id ?? event.actor_id, overview, model)
        ?? humanText('operatorMissing', language);
      const clientName = safeName(event.client?.display_name);
      const who = clientName
        ? (event.client_id
          ? t('taskPage.timeline.actorClient', { client: clientName, actor: actorBase })
          : t('taskPage.timeline.actorThroughClient', { actor: actorBase, client: clientName }))
        : actorBase;
      const actionKey = typeof event.action === 'string' ? event.action : '';
      const action = operationWord(actionKey, { language }).text;
      // Only mark the newest timestamped event as current after explicit newest-first sorting.
      const tone: TaskTimelineViewItem['tone'] = idx === 0 && hasFiniteTime
        ? 'current'
        : actionKey.includes('approve')
          ? 'positive'
          : actionKey.includes('reject')
            ? 'negative'
            : 'default';
      timeline.push({
        key: event.id ?? `act-${idx}`,
        timeText,
        who,
        what: action,
        next: null,
        note: event.result === 'unknown' ? t('recentChanges.unknown') : null,
        tone,
        reporterText: false,
      });
    }
  }

  // Blocker (Waits from deriveOverview + verbatim PPD-050 structured_waits + pending handoffs)
  const taskWaits: DerivedWait[] = (derivation?.waits ?? []).filter((w) => w.task.id === workId);
  // Prioritize: decision > review > external > dependency
  const waitPriority: Record<WaitKind, number> = { decision: 0, review: 1, external: 2, dependency: 3 };
  const primaryWait = [...taskWaits].sort((a, b) => waitPriority[a.kind] - waitPriority[b.kind])[0] ?? null;

  const structuredWaitsObs = snapshot?.observation?.structured_waits;
  const primaryStructuredWait = structuredWaitsObs?.status === 'provided'
    && Array.isArray(structuredWaitsObs.items)
    && structuredWaitsObs.items.length > 0
    ? structuredWaitsObs.items[0]
    : null;

  const pendingHandoff = (handoffs?.items ?? []).find((h) => h.pending)
    ?? (model?.input.handoffs.items ?? []).find((h) => h.taskId === workId && h.pending);

  let blocker: TaskPageModel['blocker'];
  let blockerWaitView: TaskBlockerWaitView | null = null;
  let blockerAssignView: TaskBlockerAssignmentView | null = null;

  if (primaryWait) {
    const firstPerson = primaryWait.waitingOn[0] ?? null;
    const waitingOnExternal = primaryWait.kind === 'external';
    const waitingOnName = waitingOnExternal
      ? (primaryWait.externalParty ?? t('taskPage.blocker.kind.external'))
      : (safeName(firstPerson?.displayName)
        ?? memberNameFor(firstPerson?.id, overview, model)
        ?? (primaryWait.kind === 'review' ? t('taskPage.delivery.reviewerFallback') : humanText('operatorMissing', language)));
    const waitingOnPersonId = waitingOnExternal ? null : (firstPerson?.id ?? null);

    const whatText: string | null = primaryWait.what
      ? cleanNarrative(primaryWait.what, language)
      : (primaryWait.kind === 'review' ? t('taskPage.blocker.kind.review') : null);

    const behindItems = primaryWait.behind.items
      .filter((item): item is DerivedTask & { id: string } => typeof item.id === 'string' && item.id.length > 0)
      .map((item) => ({
        id: item.id,
        title: taskTitleFor(item.id, overview, derivation, language, item.title),
      }));
    // Use the reported count directly rather than the filtered list length; null remains unknown.
    const behindCount = typeof primaryWait.behind.count === 'number' && Number.isFinite(primaryWait.behind.count)
      ? primaryWait.behind.count
      : null;

    let inboxDetailKey: string | null = null;
    if (primaryWait.kind === 'decision' && primaryWait.sourceId && firstPerson?.id) {
      const isOwnTask = ownerPersonId !== null && firstPerson.id === ownerPersonId;
      inboxDetailKey = isOwnTask
        ? inboxKeyForAnswer(primaryWait.sourceId, firstPerson.id)
        : inboxKeyForDecision(primaryWait.sourceId, firstPerson.id);
    }

    blockerWaitView = {
      kind: primaryWait.kind,
      waitingOnName,
      waitingOnPersonId,
      waitingOnExternal,
      days: primaryWait.durationDays,
      what: whatText,
      behindCount,
      behindItems,
      inboxDetailKey,
    };
    blocker = { kind: 'wait', wait: blockerWaitView };
  } else if (primaryStructuredWait) {
    const waitingOnExternal = primaryStructuredWait.waiting_on.kind === 'external_party';
    const waitingOnName = cleanNarrative(primaryStructuredWait.waiting_on.name, language)
      ?? (waitingOnExternal ? t('taskPage.blocker.kind.external') : humanText('operatorMissing', language));
    const days = Number.isSafeInteger(primaryStructuredWait.since_unix_ms)
      && primaryStructuredWait.since_unix_ms <= refNowUnixMs
      ? Math.floor((refNowUnixMs - primaryStructuredWait.since_unix_ms) / DAY_MS)
      : null;
    blockerWaitView = {
      kind: waitingOnExternal ? 'external' : 'reported',
      waitingOnName,
      waitingOnPersonId: null,
      waitingOnExternal,
      days,
      what: cleanNarrative(primaryStructuredWait.what, language),
      behindCount: null,
      behindItems: [],
      inboxDetailKey: null,
    };
    blocker = { kind: 'wait', wait: blockerWaitView };
  } else if (pendingHandoff) {
    const isApiHandoff = 'handoff_id' in pendingHandoff;
    const fromName = isApiHandoff
      ? (safeName(pendingHandoff.from_display_name) ?? humanText('operatorMissing', language))
      : (memberNameFor(pendingHandoff.fromPersonId, overview, model) ?? humanText('operatorMissing', language));
    const toPersonId = isApiHandoff ? null : (pendingHandoff.toPersonId ?? null);
    const toName = isApiHandoff
      ? (safeName(pendingHandoff.to_display_name) ?? ownerName ?? humanText('operatorMissing', language))
      : (memberNameFor(pendingHandoff.toPersonId, overview, model) ?? ownerName ?? humanText('operatorMissing', language));
    const atMs = isApiHandoff ? pendingHandoff.updated_at_unix_ms : (pendingHandoff.updatedAtUnixMs ?? null);
    const atText = typeof atMs === 'number' && Number.isFinite(atMs)
      ? compactTimeText(atMs, { language, nowUnixMs, timeZone })
      : null;
    const handoffId = isApiHandoff ? pendingHandoff.handoff_id : (pendingHandoff.id ?? null);
    const resolvedToId = toPersonId ?? ownerPersonId;
    const inboxDetailKey = handoffId && resolvedToId ? `assignment:${handoffId}:${resolvedToId}` : null;

    blockerAssignView = {
      fromName,
      toName,
      toPersonId: resolvedToId,
      atText,
      inboxDetailKey,
    };
    blocker = { kind: 'assignment', assignment: blockerAssignView };
  } else if (derivation && !derivation.coverage.sections.waits.complete) {
    blocker = { kind: 'unknown' };
  } else {
    blocker = { kind: 'none' };
  }

  // Why (Goal + Project + Workstream): never fabricate a goal from goals[0] when unlinked,
  // and never infer weekly commitment membership from commitment ID substrings.
  const firstGoalId = derivedTask?.goalIds?.[0] ?? null;
  const matchedGoal = firstGoalId ? goals?.find((g) => g.goal_id === firstGoalId) : undefined;
  const contractGoalText = contract?.goals && contract.goals.length > 0
    ? cleanNarrative(contract.goals[0], language)
    : null;
  const goalTitle = matchedGoal?.title
    ? (cleanNarrative(matchedGoal.title, language) ?? matchedGoal.title)
    : contractGoalText;
  const goalId = matchedGoal?.goal_id ?? firstGoalId ?? null;

  const streamTitles = overview ? groupWorks(overview.works, overview.workstreams).titles : {};
  const workstreamId = overviewWork?.workstream_id ?? null;
  const rawStreamTitle = workstreamId ? streamTitles[workstreamId] : null;
  const workstreamTitle = rawStreamTitle
    ? workLabel(rawStreamTitle, { language, recordedTitle: true, fallback: humanText('workstreamUnnamed', language) }).text
    : null;

  const weekCommitment: TaskWhyView['weekCommitment'] = null;

  const why: TaskWhyView = {
    goalId,
    goalTitle,
    projectTitle,
    workstreamTitle,
    weekCommitment,
  };

  // Dependencies (upstreams + downstreams) strictly from dependencies.list and overview-model.
  const depItems: TaskDependencyItemView[] = [];
  const seenUpstreams = new Set<string>();

  for (const dep of dependencies ?? []) {
    if (dep.work_id !== workId) continue;
    seenUpstreams.add(dep.provider_work_id);
    const providerTitle = taskTitleFor(dep.provider_work_id, overview, derivation, language, dep.provider_title);
    const need = t('taskPage.deps.needApproved');
    const have = dep.adopted_version ? t('collaboration.acceptedDelivery') : t('taskPage.deps.haveNone');
    const avail = dep.validity === 'upstream_changed'
      ? t('taskPage.deps.availChanged')
      : dep.validity === 'valid'
        ? t('taskPage.deps.availReady')
        : t('taskPage.deps.availUnavailable');
    depItems.push({
      key: `up-${dep.provider_work_id}`,
      direction: 'upstream',
      taskId: dep.provider_work_id,
      title: providerTitle,
      meta: t('taskPage.deps.upstreamDetail', { need, have, avail }),
    });
  }
  for (const dep of model?.input.dependencies.items ?? []) {
    if (dep.consumerTaskId !== workId || !dep.providerTaskId || seenUpstreams.has(dep.providerTaskId)) continue;
    seenUpstreams.add(dep.providerTaskId);
    const providerTitle = taskTitleFor(dep.providerTaskId, overview, derivation, language);
    const need = dep.what ? (cleanNarrative(dep.what, language) ?? t('taskPage.deps.needApproved')) : t('taskPage.deps.needApproved');
    const have = t('taskPage.deps.haveNone');
    const avail = dep.satisfied === true ? t('taskPage.deps.availReady') : t('taskPage.deps.availUnavailable');
    depItems.push({
      key: `up-${dep.providerTaskId}`,
      direction: 'upstream',
      taskId: dep.providerTaskId,
      title: providerTitle,
      meta: t('taskPage.deps.upstreamDetail', { need, have, avail }),
    });
  }

  // Downstreams: tasks depending on this task
  const seenDownstreams = new Set<string>();
  for (const dep of model?.input.dependencies.items ?? []) {
    if (dep.providerTaskId !== workId || !dep.consumerTaskId || seenDownstreams.has(dep.consumerTaskId)) continue;
    seenDownstreams.add(dep.consumerTaskId);
    const consumer = derivation?.tasks.find((task) => task.id === dep.consumerTaskId);
    const consumerTitle = taskTitleFor(dep.consumerTaskId, overview, derivation, language, consumer?.title);
    const consumerStatus = consumer?.status ?? 'unknown';
    const consumerStatusText = consumerStatus === 'verified'
      ? t('taskPage.status.verified')
      : t(`display.taskStatus.${consumerStatus}.label`);
    const consumerOwner = safeName(consumer?.owner?.displayName)
      ?? memberNameFor(consumer?.owner?.id, overview, model)
      ?? t('taskPage.ownerUnknown');
    depItems.push({
      key: `down-${dep.consumerTaskId}`,
      direction: 'downstream',
      taskId: dep.consumerTaskId,
      title: consumerTitle,
      meta: `${consumerStatusText} · ${consumerOwner}`,
    });
  }

  // Required documents
  const docItems: TaskDocItemView[] = [];
  if (documents && documents.length > 0) {
    for (const [idx, doc] of documents.entries()) {
      const docTitle = doc.name
        ? (cleanNarrative(doc.name, language) ?? doc.name)
        : documentLabel(doc.path ?? '', { language, position: idx + 1 }).text;
      const docMeta = doc.summary
        ? (cleanNarrative(doc.summary, language) ?? t('taskPage.docs.defaultMeta'))
        : t('taskPage.docs.defaultMeta');
      docItems.push({
        key: `doc-${doc.document_position}-${idx}`,
        docId: parseDocIdFromPath(doc.path),
        title: docTitle,
        meta: docMeta,
      });
    }
  } else if (snapshot?.required_specs && snapshot.required_specs.length > 0) {
    for (const [idx, spec] of snapshot.required_specs.entries()) {
      docItems.push({
        key: `spec-${idx}`,
        docId: parseDocIdFromPath(spec.path),
        title: documentLabel(spec.path, { language, position: idx + 1 }).text,
        meta: t('taskPage.docs.defaultMeta'),
      });
    }
  }

  // Lessons applied to this task
  const lessonItems: TaskLessonItemView[] = (lessons ?? [])
    .filter((lesson) => Array.isArray(lesson.tasks) && lesson.tasks.includes(workId))
    .map((lesson) => {
      const stage: TaskLessonItemView['stage'] =
        lesson.stage === 'candidate' || lesson.stage === 'project' || lesson.stage === 'group'
          ? lesson.stage
          : 'unknown';
      return {
        key: lesson.lesson_id,
        lessonId: lesson.lesson_id,
        title: cleanNarrative(lesson.title, language) ?? lesson.title,
        stage,
        authorName: safeName(lesson.author?.display_name),
      };
    });

  // Programmatic one-sentence summary
  const upstreamProviderTitle = depItems.find((d) => d.direction === 'upstream')?.title ?? null;
  const summaryText = buildOneSentence({
    status,
    criteria,
    hasPerCriterionEvidence,
    delivery,
    wait: blockerWaitView,
    assignment: blockerAssignView,
    agentName,
    agentActive: derivedTask?.agentActive ?? null,
    upstreamProviderTitle,
    t,
  });

  // Unknown facts check
  const hasUnknownFacts = status === 'unknown'
    || model?.ordering === 'incomparable'
    || blocker.kind === 'unknown'
    || (blockerWaitView !== null && blockerWaitView.behindCount === null)
    || (activity?.some((item) => item.result === 'unknown') ?? false);

  // Merge freshness across overview, model, and snapshot
  const baseFreshness = mergeProjectFreshness(overview?.freshness, model?.freshness);
  const freshness = mergeProjectFreshness(baseFreshness, snapshotFreshness);

  // Unused param lint guard
  void projectKey;

  return {
    found,
    title,
    summaryText,
    status,
    liveRunning,
    ownerName,
    ownerPersonId,
    agentName,
    nextStepText,
    nextStepReporterText,
    acceptance: {
      hasPerCriterionEvidence,
      items: criteria,
    },
    report,
    delivery,
    timeline,
    blocker,
    why,
    dependencies: depItems,
    documents: docItems,
    lessons: lessonItems,
    hasUnknownFacts,
    freshness,
  };
}
