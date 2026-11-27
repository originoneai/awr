import { deriveOwner, readsAsName } from '../../core/observation/owner';
import type { CountDetails, OverviewMember, OverviewReviewRound } from '../../core/overview';
import type {
  ReviewRoundsData,
  TeamFreshness,
  TeamOverviewResponse,
  TeamWork,
  WorkSnapshotData,
} from '../../lib/types';
import { mergeProjectFreshness } from './project-overview-model';
import type { ProjectListModel } from './project-list-model';

const DAY_MS = 86_400_000;

export type AcceptanceDetailFilter = 'all' | 'approved' | 'open' | 'rejected';

export const ACCEPTANCE_DETAIL_FILTERS: readonly AcceptanceDetailFilter[] = [
  'all',
  'approved',
  'open',
  'rejected',
];

export function normalizeAcceptanceDetail(value: string | null): AcceptanceDetailFilter {
  return (ACCEPTANCE_DETAIL_FILTERS as readonly string[]).includes(value ?? '')
    ? (value as AcceptanceDetailFilter)
    : 'all';
}

export type AcceptanceReviewState = 'approved' | 'open' | 'rejected' | 'invalidated' | 'unknown';

export type AcceptanceReviewerRole =
  | 'independent_reviewer'
  | 'author_reviewer'
  | 'agent_reviewer'
  | 'simulated_member_reviewer';

export type AcceptanceVersion =
  | { kind: 'numbered'; number: number }
  | { kind: 'current' }
  | { kind: 'historical' }
  | { kind: 'unknown' };

export type AcceptanceIntegrationStatus =
  | 'integrated'
  | 'awaiting_review'
  | 'not_integrated'
  | 'awaiting_integration'
  | 'dispatching'
  | 'rejected'
  | 'not_required'
  | 'unknown';

export interface AcceptanceCheckItem {
  rawCheck: string;
  outcome: 'passed' | 'failed' | 'unknown';
}

export interface AcceptanceLatestReview {
  roundId: string | null;
  roundIndex: number | null;
  state: AcceptanceReviewState;
  reviewerName: string | null;
  reviewerRole: AcceptanceReviewerRole | null;
  reasonFirstLine: string | null;
  waitDays: number | null;
  restricted: boolean;
  roundsFailed: boolean;
  roundsLoading: boolean;
}

export interface AcceptanceOutcomeRow {
  workKey: string;
  workstreamId: string;
  rawTitle: string;
  submitterName: string | null;
  version: AcceptanceVersion;
  checks: AcceptanceCheckItem[];
  latestReview: AcceptanceLatestReview;
  integration: AcceptanceIntegrationStatus;
  highlighted: boolean;
}

export interface ProjectAcceptanceCoverage {
  snapshotsLoaded: number;
  snapshotsTotal: number;
  snapshotsLoading: boolean;
  snapshotsFailed: number;
  allSnapshotsFailed: boolean;
  roundsFailed: number;
  overviewPartial: boolean;
  paginated: boolean;
  orderingIncomparable: boolean;
  hasUnknownFacts: boolean;
}

export interface ProjectAcceptanceModel {
  rows: AcceptanceOutcomeRow[];
  counts: {
    total: CountDetails<AcceptanceOutcomeRow>;
    approved: CountDetails<AcceptanceOutcomeRow>;
    open: CountDetails<AcceptanceOutcomeRow>;
    rejected: CountDetails<AcceptanceOutcomeRow>;
  };
  coverage: ProjectAcceptanceCoverage;
  freshness: TeamFreshness | undefined;
}

export type SnapshotReadState = 'loading' | 'ready' | 'failed';
export type RoundsReadState = 'idle' | 'loading' | 'ready' | 'failed' | 'forbidden';

function cleanText(value: unknown): string | null {
  return typeof value === 'string' && value.trim().length > 0 ? value.trim() : null;
}

function cleanName(value: unknown): string | null {
  const text = cleanText(value);
  return text && readsAsName(text) ? text : null;
}

function parseReviewState(value: unknown): AcceptanceReviewState {
  if (value === 'approved' || value === 'open' || value === 'rejected' || value === 'invalidated') {
    return value;
  }
  return 'unknown';
}

function parseReviewerRole(value: unknown): AcceptanceReviewerRole | null {
  if (
    value === 'independent_reviewer'
    || value === 'author_reviewer'
    || value === 'agent_reviewer'
    || value === 'simulated_member_reviewer'
  ) {
    return value;
  }
  return null;
}

export function hasDeliveredOutcome(snapshot: WorkSnapshotData | undefined): boolean {
  const collab = snapshot?.observation?.collaboration;
  if (!collab) return false;
  if (collab.candidate != null) return true;
  if (collab.review != null) return true;
  if (Array.isArray(collab.verification) && collab.verification.length > 0) return true;
  return false;
}

function deriveVersion(snapshot: WorkSnapshotData): AcceptanceVersion {
  const collab = snapshot.observation?.collaboration;
  const selectionVersion = cleanText(collab?.candidate?.selection_version);
  if (selectionVersion && /^[1-9]\d*$/.test(selectionVersion)) {
    const parsed = Number(selectionVersion);
    if (Number.isSafeInteger(parsed)) {
      return { kind: 'numbered', number: parsed };
    }
  }
  if (collab?.candidate?.current === false) return { kind: 'historical' };
  if (collab?.candidate?.current === true || (collab?.delivery_required === false && collab?.review != null)) {
    return { kind: 'current' };
  }
  return { kind: 'unknown' };
}

function deriveChecks(snapshot: WorkSnapshotData): AcceptanceCheckItem[] {
  const verification = snapshot.observation?.collaboration?.verification;
  if (!Array.isArray(verification)) return [];
  return verification.map((entry) => {
    const rawCheck = cleanText(entry?.check) ?? '';
    const outcome = entry?.outcome === 'passed'
      ? 'passed'
      : entry?.outcome === 'failed'
        ? 'failed'
        : 'unknown';
    return { rawCheck, outcome };
  });
}

function deriveIntegration(
  snapshot: WorkSnapshotData,
  reviewState: AcceptanceReviewState,
): AcceptanceIntegrationStatus {
  const collab = snapshot.observation?.collaboration;
  if (!collab) return 'unknown';
  const reportOnly = collab.delivery_required === false && !collab.candidate && !collab.integration;
  if (reportOnly) return 'not_required';
  const integ = collab.integration;
  if (integ) {
    const state = cleanText(integ.state);
    if (integ.current_binding === true) {
      if (state === 'confirmed') return 'integrated';
      if (state === 'rejected') return 'rejected';
      if (state === 'dispatched') return 'dispatching';
      if (state === 'unknown') return 'unknown';
    } else if (integ.current_binding === false) {
      if (state === 'dispatched' || state === 'unknown') return 'unknown';
      // Historical settled integration does not decide the current version.
    } else {
      return 'unknown';
    }
  }
  if (reviewState === 'open') return 'awaiting_review';
  if (reviewState === 'rejected' || reviewState === 'invalidated') return 'not_integrated';
  if (reviewState === 'approved') return 'awaiting_integration';
  return 'unknown';
}

function elapsedDays(startedAtUnixMs: number | null | undefined, nowUnixMs: number | null): number | null {
  if (
    typeof startedAtUnixMs !== 'number'
    || !Number.isFinite(startedAtUnixMs)
    || startedAtUnixMs < 0
    || nowUnixMs === null
    || !Number.isFinite(nowUnixMs)
    || startedAtUnixMs > nowUnixMs
  ) {
    return null;
  }
  return Math.floor((nowUnixMs - startedAtUnixMs) / DAY_MS);
}

function latestRoundFromList(
  roundsData: ReviewRoundsData | undefined,
  currentRoundId: string | null,
): ReviewRoundsData['items'][number] | null {
  const items = roundsData?.items;
  if (!Array.isArray(items) || items.length === 0) return null;
  if (currentRoundId) {
    const matched = items.find((item) => item.round_id === currentRoundId);
    if (matched) return matched;
  }
  return items.reduce((latest, item) => (
    item.round_index >= latest.round_index ? item : latest
  ), items[0]);
}

function latestModelRoundForWork(
  modelRounds: readonly OverviewReviewRound[],
  workKey: string,
  currentRoundId: string | null,
): OverviewReviewRound | null {
  const matching = modelRounds.filter((round) => round.taskId === workKey);
  if (matching.length === 0) return null;
  if (currentRoundId) {
    const exact = matching.find((round) => round.id === currentRoundId);
    if (exact) return exact;
  }
  const current = matching.find((round) => round.current === true);
  if (current) return current;
  return matching.reduce((latest, item) => (
    (item.roundIndex ?? -1) >= (latest.roundIndex ?? -1) ? item : latest
  ), matching[0]);
}

function memberNameMap(members: readonly OverviewMember[]): Map<string, string> {
  const map = new Map<string, string>();
  for (const member of members) {
    const id = cleanText(member.personId);
    const name = cleanName(member.displayName);
    if (id && name) map.set(id, name);
  }
  return map;
}

function makeCountDetails(
  items: AcceptanceOutcomeRow[],
  known: boolean,
  reason: string,
): CountDetails<AcceptanceOutcomeRow> {
  return {
    count: known ? items.length : null,
    items,
    unknownItems: known ? [] : [{ part: 'reviewRounds', reason }],
  };
}

/**
 * Pure, deterministic derivation of the Project Acceptance ("成果与验收") view model.
 * Every summary count shares the exact filtered row array shown by its detail view.
 */
export function deriveProjectAcceptance({
  projectKey,
  overview,
  visibleWorks,
  snapshotsByKey,
  snapshotStatusByKey,
  snapshotFreshness,
  reviewRoundsByKey,
  reviewRoundsStatusByKey,
  model,
  nowUnixMs,
}: {
  projectKey: string;
  overview?: TeamOverviewResponse;
  visibleWorks: readonly TeamWork[];
  snapshotsByKey: ReadonlyMap<string, WorkSnapshotData | undefined>;
  snapshotStatusByKey: ReadonlyMap<string, SnapshotReadState>;
  snapshotFreshness?: TeamFreshness;
  reviewRoundsByKey: ReadonlyMap<string, ReviewRoundsData | undefined>;
  reviewRoundsStatusByKey: ReadonlyMap<string, RoundsReadState>;
  model?: ProjectListModel;
  nowUnixMs: number | null;
}): ProjectAcceptanceModel {
  const usableModel = model?.project === projectKey && model.input && model.input.options ? model : undefined;
  const orderingIncomparable = usableModel?.ordering === 'incomparable';
  const members = usableModel?.input.members.coverage === 'unread'
    ? []
    : (usableModel?.input.members.items ?? []);
  const memberNames = memberNameMap(members);
  const modelRounds = usableModel?.input.reviewRounds.coverage === 'unread'
    ? []
    : (usableModel?.input.reviewRounds.items ?? []);
  const modelTaskOrder = new Map<string, number>();
  if (usableModel && usableModel.input.tasks.coverage !== 'unread') {
    usableModel.input.tasks.items.forEach((task, index) => {
      const taskId = cleanText(task.id);
      if (taskId && !modelTaskOrder.has(taskId)) modelTaskOrder.set(taskId, index);
    });
  }

  let snapshotsLoaded = 0;
  let snapshotsFailed = 0;
  let snapshotsLoading = false;
  let roundsFailed = 0;

  const rows: AcceptanceOutcomeRow[] = [];

  for (const work of visibleWorks) {
    const status = snapshotStatusByKey.get(work.key) ?? 'loading';
    const snapshot = snapshotsByKey.get(work.key);
    if (status === 'ready' && snapshot) {
      snapshotsLoaded += 1;
    } else if (status === 'failed') {
      snapshotsFailed += 1;
      continue;
    } else {
      snapshotsLoading = true;
      continue;
    }

    if (!hasDeliveredOutcome(snapshot)) continue;

    const collab = snapshot.observation?.collaboration;
    const currentRoundId = cleanText(collab?.review?.id);
    const roundsStatus = reviewRoundsStatusByKey.get(work.key) ?? 'idle';
    if (roundsStatus === 'failed') roundsFailed += 1;

    const roundsData = reviewRoundsByKey.get(work.key);
    const latestRound = latestRoundFromList(roundsData, currentRoundId);
    const modelRound = latestModelRoundForWork(modelRounds, work.key, currentRoundId);

    const restricted = latestRound?.readable === false || roundsStatus === 'forbidden';

    // Review state comes from snapshot.collaboration.review.state first, falling back to latest round.
    const rawState = collab?.review?.state ?? (latestRound?.readable ? latestRound.state : null) ?? modelRound?.state;
    const reviewState = parseReviewState(rawState);

    const roundIndex = typeof latestRound?.round_index === 'number' && Number.isSafeInteger(latestRound.round_index) && latestRound.round_index > 0
      ? latestRound.round_index
      : typeof modelRound?.roundIndex === 'number' && Number.isSafeInteger(modelRound.roundIndex) && modelRound.roundIndex > 0
        ? modelRound.roundIndex
        : null;

    // Resolve reviewer display name from recorded member directory or snapshot responsibility (never from raw IDs).
    const snapshotIndReviewer = snapshot.observation?.responsibility?.independent_reviewer as
      | { person_id?: unknown; display_name?: unknown }
      | null
      | undefined;
    const roundReviewerPersonId = cleanText(latestRound?.reviewer_person_id);
    const modelReviewerPersonId = cleanText(modelRound?.reviewerPersonId);
    const snapshotIndReviewerId = cleanText(snapshotIndReviewer?.person_id);
    const snapshotIndReviewerName = cleanName(snapshotIndReviewer?.display_name);

    let reviewerName: string | null = null;
    if (!restricted) {
      if (roundReviewerPersonId) {
        reviewerName = memberNames.get(roundReviewerPersonId)
          ?? (snapshotIndReviewerId === roundReviewerPersonId ? snapshotIndReviewerName : null);
      } else if (modelReviewerPersonId) {
        reviewerName = memberNames.get(modelReviewerPersonId)
          ?? (snapshotIndReviewerId === modelReviewerPersonId ? snapshotIndReviewerName : null);
      } else if (snapshotIndReviewerName) {
        reviewerName = snapshotIndReviewerName;
      }
    }

    // Reviewer independence role is ONLY from explicit server-recorded role/flag, never inferred from names.
    let reviewerRole: AcceptanceReviewerRole | null = null;
    if (!restricted) {
      reviewerRole = parseReviewerRole(latestRound?.reviewer_role);
      if (reviewerRole === null && reviewState === 'open') {
        if (modelRound?.independent === true || snapshotIndReviewer != null) {
          reviewerRole = 'independent_reviewer';
        }
      }
    }

    const reasonFirstLine = !restricted ? cleanText(latestRound?.reason_first_line) : null;
    const waitDays = !restricted && reviewState === 'open'
      ? elapsedDays(modelRound?.openedAtUnixMs, nowUnixMs)
      : null;

    const owner = deriveOwner({
      rowOwner: work.owner_person,
      snapshot,
      readState: 'ready',
    });

    const integration = deriveIntegration(snapshot, reviewState);

    rows.push({
      workKey: work.key,
      workstreamId: work.workstream_id,
      rawTitle: work.title,
      submitterName: owner.name,
      version: deriveVersion(snapshot),
      checks: deriveChecks(snapshot),
      latestReview: {
        roundId: currentRoundId ?? cleanText(latestRound?.round_id),
        roundIndex,
        state: reviewState,
        reviewerName,
        reviewerRole,
        reasonFirstLine,
        waitDays,
        restricted,
        roundsFailed: roundsStatus === 'failed',
        roundsLoading: roundsStatus === 'loading',
      },
      integration,
      highlighted: reviewState === 'rejected',
    });
  }

  if (modelTaskOrder.size > 0 && rows.length > 1) {
    const fallbackIndex = new Map<string, number>();
    rows.forEach((row, index) => fallbackIndex.set(row.workKey, index));
    rows.sort((a, b) => {
      const aPos = modelTaskOrder.get(a.workKey);
      const bPos = modelTaskOrder.get(b.workKey);
      if (aPos !== undefined && bPos !== undefined) return aPos - bPos;
      if (aPos !== undefined) return -1;
      if (bPos !== undefined) return 1;
      return (fallbackIndex.get(a.workKey) ?? 0) - (fallbackIndex.get(b.workKey) ?? 0);
    });
  }

  const totalWorks = overview?.works.length ?? visibleWorks.length;
  const paginated = totalWorks > visibleWorks.length;
  const overviewCoverage = overview?.coverage;
  const overviewPartial = Boolean(
    overviewCoverage
    && (overviewCoverage.scope.status === 'failed'
      || overviewCoverage.failed > 0
      || overviewCoverage.truncated
      || overviewCoverage.scope.truncated
      || overviewCoverage.streams.some((stream) => stream.truncated)),
  );
  const allSnapshotsFailed = visibleWorks.length > 0 && snapshotsFailed === visibleWorks.length;

  const approvedRows = rows.filter((row) => row.latestReview.state === 'approved');
  const openRows = rows.filter((row) => row.latestReview.state === 'open');
  const rejectedRows = rows.filter((row) => row.latestReview.state === 'rejected');

  const hasUnknownRowState = rows.some(
    (row) => row.latestReview.state === 'unknown' || row.integration === 'unknown' || row.version.kind === 'unknown',
  );
  const countsComplete = !snapshotsLoading
    && snapshotsFailed === 0
    && !overviewPartial
    && !paginated
    && !orderingIncomparable;
  const breakdownComplete = countsComplete && !rows.some((row) => row.latestReview.state === 'unknown');

  const countReason = orderingIncomparable
    ? 'ordering-incomparable'
    : snapshotsLoading
      ? 'snapshots-loading'
      : snapshotsFailed > 0
        ? 'snapshots-partial'
        : overviewPartial
          ? 'overview-partial'
          : paginated
            ? 'paginated'
            : 'review-state-unknown';

  const freshness = mergeProjectFreshness(
    mergeProjectFreshness(overview?.freshness, snapshotFreshness),
    usableModel?.freshness,
  );

  const hasUnknownFacts = orderingIncomparable || hasUnknownRowState;

  return {
    rows,
    counts: {
      total: makeCountDetails(rows, countsComplete, countReason),
      approved: makeCountDetails(approvedRows, breakdownComplete, countReason),
      open: makeCountDetails(openRows, breakdownComplete, countReason),
      rejected: makeCountDetails(rejectedRows, breakdownComplete, countReason),
    },
    coverage: {
      snapshotsLoaded,
      snapshotsTotal: visibleWorks.length,
      snapshotsLoading,
      snapshotsFailed,
      allSnapshotsFailed,
      roundsFailed,
      overviewPartial,
      paginated,
      orderingIncomparable,
      hasUnknownFacts,
    },
    freshness,
  };
}
