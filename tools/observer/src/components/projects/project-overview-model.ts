import type { CompletionCounts } from '../display/CompletionBar';
import { groupWorks } from '../../core/observation';
import { readsAsName } from '../../core/observation/owner';
import {
  deriveOverview,
  OVERVIEW_PARTS,
  type CountDetails,
  type CoverageIssue,
  type DerivedTask,
  type DerivedWait,
  type OverviewDecision,
  type OverviewDerivation,
  type OverviewMember,
  type OverviewPart,
  type PersonRef,
  type ReadSlice,
} from '../../core/overview';
import type {
  PlaneFreshness,
  TeamFreshness,
  TeamOverviewResponse,
} from '../../lib/types';
import {
  deriveProjectListItem,
  type ProjectHealth,
  type ProjectListEntry,
  type ProjectListModel,
  type ProjectPeriodUpdate,
} from './project-list-model';

const DAY_MS = 86_400_000;
const RECENT_UPDATE_LIMIT = 3;

export type ProjectOverviewKpiId = 'verified' | 'claimed' | 'review' | 'decisions';

export interface ProjectOpenDecision {
  id: string | null;
  taskId: string | null;
  taskTitle: string | null;
  question: string | null;
  responders: PersonRef[];
  responderNames: string[];
  createdAtUnixMs: number | null;
  durationDays: number | null;
  /** Matches InboxPage's `?detail=` key when an accountable member is identified. */
  inboxDetailKey: string | null;
}

export interface ProjectOverviewWorkstream {
  id: string;
  title: string | null;
  tasks: DerivedTask[];
  /** Four-segment counts; null whenever any task status in this workstream is unknown or tasks are incomplete. */
  counts: CompletionCounts | null;
  blockedTasks: DerivedTask[];
}

export interface ProjectPeriodUpdateView {
  id: string | null;
  periodLabel: string | null;
  startsOn: string | null;
  endsOn: string | null;
  status: 'ok' | 'risk' | 'off' | 'unknown';
  summary: string | null;
  authorName: string | null;
  recordedAtUnixMs: number | null;
}

export interface ProjectOverviewVerdict {
  kind: 'update' | 'stale-update' | 'facts' | 'unknown';
  health: ProjectHealth;
  latestUpdate: ProjectPeriodUpdateView | null;
  daysSinceLastUpdate: number | null;
  factCounts: {
    total: number | null;
    verified: number | null;
    review: number | null;
    decisions: number | null;
  };
}

export interface ProjectOverviewReviewMetric extends CountDetails<DerivedTask> {
  reviewerNames: string[];
  reviewerUnknown: boolean;
  oldestDays: number | null;
}

export interface ProjectOverviewDecisionMetric extends CountDetails<ProjectOpenDecision> {
  oldestDays: number | null;
}

export interface ProjectOverviewModel {
  health: ProjectHealth;
  lead: { id: string | null; name: string | null };
  groupName: string | null;
  taskCount: CountDetails<DerivedTask>;
  workstreamCount: number | null;
  verdict: ProjectOverviewVerdict;
  kpis: {
    verified: CountDetails<DerivedTask>;
    claimed: CountDetails<DerivedTask>;
    review: ProjectOverviewReviewMetric;
    decisions: ProjectOverviewDecisionMetric;
  };
  workstreams: ProjectOverviewWorkstream[];
  recentUpdates: {
    items: ProjectPeriodUpdateView[];
    total: number | null;
    coverage: 'complete' | 'partial' | 'unread' | 'absent';
  };
  coverage: {
    modelPartialParts: OverviewPart[];
    modelUnreadParts: OverviewPart[];
    orderingIncomparable: boolean;
    hasUnknownFacts: boolean;
  };
  derived: OverviewDerivation | null;
}

function cleanId(value: string | null | undefined): string | null {
  return typeof value === 'string' && value.trim().length > 0 ? value.trim() : null;
}

function cleanText(value: string | null | undefined): string | null {
  return typeof value === 'string' && value.trim().length > 0 ? value.trim() : null;
}

function cleanName(value: string | null | undefined): string | null {
  const trimmed = cleanText(value);
  return trimmed && readsAsName(trimmed) ? trimmed : null;
}

function validIsoDate(value: string | null | undefined): value is string {
  return typeof value === 'string'
    && /^\d{4}-\d{2}-\d{2}$/.test(value)
    && Number.isFinite(Date.parse(`${value}T00:00:00Z`));
}

/** Deterministic ISO week number for a Mon–Sun 7-day period when no explicit periodLabel was supplied. */
export function isoWeekNumber(startsOn: string | null | undefined, endsOn: string | null | undefined): number | null {
  if (!validIsoDate(startsOn) || !validIsoDate(endsOn)) return null;
  const startMs = Date.parse(`${startsOn}T00:00:00Z`);
  const endMs = Date.parse(`${endsOn}T00:00:00Z`);
  if (Math.round((endMs - startMs) / DAY_MS) !== 6) return null;
  const date = new Date(startMs);
  if (date.getUTCDay() !== 1) return null;
  const thursday = new Date(startMs + 3 * DAY_MS);
  const yearStart = Date.UTC(thursday.getUTCFullYear(), 0, 1);
  return Math.floor((thursday.getTime() - yearStart) / (7 * DAY_MS)) + 1;
}

function validTimestamp(value: number | null | undefined, maxUnixMs?: number): number | null {
  if (typeof value !== 'number' || !Number.isFinite(value) || value < 0) return null;
  if (maxUnixMs !== undefined && value > maxUnixMs) return null;
  return value;
}

function elapsedDays(startedAtUnixMs: number | null, nowUnixMs: number | null): number | null {
  if (startedAtUnixMs === null || nowUnixMs === null || startedAtUnixMs > nowUnixMs) return null;
  return Math.floor((nowUnixMs - startedAtUnixMs) / DAY_MS);
}

function unknownDetails<T>(part: OverviewPart, reason: string, items: T[] = []): CountDetails<T> {
  return { count: null, items, unknownItems: [{ part, reason }] };
}

function resolveMemberName(members: readonly OverviewMember[], personId: string | null): string | null {
  if (!personId) return null;
  const found = members.find((member) => cleanId(member.personId) === personId);
  return cleanName(found?.displayName);
}

function normalizeUpdate(
  update: ProjectPeriodUpdate,
  members: readonly OverviewMember[],
  nowUnixMs: number | null,
): ProjectPeriodUpdateView {
  const status = update.status === 'ok' || update.status === 'risk' || update.status === 'off'
    ? update.status
    : 'unknown';
  const startsOn = validIsoDate(update.startsOn) ? update.startsOn : null;
  const endsOn = validIsoDate(update.endsOn) ? update.endsOn : null;
  const explicitLabel = cleanText(update.periodLabel);
  const authorName = cleanName(update.authorName) ?? resolveMemberName(members, cleanId(update.authorPersonId));
  return {
    id: cleanId(update.id),
    periodLabel: explicitLabel,
    startsOn,
    endsOn,
    status,
    summary: cleanText(update.summary),
    authorName,
    recordedAtUnixMs: validTimestamp(update.recordedAtUnixMs, nowUnixMs ?? undefined),
  };
}

function sortUpdatesDescending(
  updates: readonly ProjectPeriodUpdate[],
  nowUnixMs: number | null,
): ProjectPeriodUpdate[] {
  return [...updates].sort((left, right) => {
    const endCompare = (right.endsOn ?? '').localeCompare(left.endsOn ?? '');
    if (endCompare !== 0) return endCompare;
    const startCompare = (right.startsOn ?? '').localeCompare(left.startsOn ?? '');
    if (startCompare !== 0) return startCompare;
    return (validTimestamp(right.recordedAtUnixMs, nowUnixMs ?? undefined) ?? -1)
      - (validTimestamp(left.recordedAtUnixMs, nowUnixMs ?? undefined) ?? -1);
  });
}

function deriveDecisionsMetric(
  slice: ReadSlice<OverviewDecision> | undefined,
  tasksById: ReadonlyMap<string, DerivedTask>,
  members: readonly OverviewMember[],
  nowUnixMs: number | null,
): ProjectOverviewDecisionMetric {
  if (!slice || slice.coverage === 'unread') {
    return { ...unknownDetails<ProjectOpenDecision>('decisions', 'decisions-unread'), oldestDays: null };
  }
  const issues: CoverageIssue[] = [];
  if (slice.coverage !== 'complete') {
    issues.push({ part: 'decisions', reason: slice.reason });
  }
  const items: ProjectOpenDecision[] = [];
  for (const [index, decision] of slice.items.entries()) {
    const state = decision?.state ?? '';
    if (!['open', 'replied', 'cancelled'].includes(state)) {
      issues.push({ part: 'decisions', reason: 'decision-state-unknown', index, taskId: cleanId(decision?.taskId) });
      continue;
    }
    if (state !== 'open') continue;
    if (decision.current == null) {
      issues.push({ part: 'decisions', reason: 'decision-current-unknown', index, taskId: cleanId(decision?.taskId) });
      continue;
    }
    if (decision.current === false) continue;

    const decisionId = cleanId(decision.id);
    const taskId = cleanId(decision.taskId);
    const task = taskId ? tasksById.get(taskId) ?? null : null;
    const responderIds = Array.isArray(decision.allowedResponderIds)
      ? decision.allowedResponderIds.map(cleanId).filter((value): value is string => value !== null)
      : [];
    const responders: PersonRef[] = responderIds.map((personId) => ({
      id: personId,
      displayName: resolveMemberName(members, personId),
    }));
    const responderNames = responders
      .map((person) => person.displayName)
      .filter((name): name is string => name !== null);
    const createdAtUnixMs = validTimestamp(decision.createdAtUnixMs, nowUnixMs ?? undefined);
    const durationDays = elapsedDays(createdAtUnixMs, nowUnixMs);
    const firstResponderId = responders[0]?.id ?? null;
    const isOwnTask = task?.owner?.id != null && firstResponderId != null && task.owner.id === firstResponderId;
    const kind = isOwnTask ? 'answer' : 'decision';
    const inboxDetailKey = firstResponderId != null && (decisionId != null || taskId != null)
      ? `${kind}:${decisionId ?? `task:${taskId}`}:${firstResponderId}`
      : null;

    items.push({
      id: decisionId,
      taskId,
      taskTitle: task?.title ?? null,
      question: cleanText(decision.question),
      responders,
      responderNames,
      createdAtUnixMs,
      durationDays,
      inboxDetailKey,
    });
  }

  // Longest-waiting decisions rise to the top; unknown ages stay at the end in recorded order.
  const ordered = items
    .map((item, index) => ({ item, index }))
    .sort((left, right) => {
      const a = left.item.durationDays;
      const b = right.item.durationDays;
      if (a === null && b === null) return left.index - right.index;
      if (a === null) return 1;
      if (b === null) return -1;
      return b - a || left.index - right.index;
    })
    .map((entry) => entry.item);

  const days = ordered.map((item) => item.durationDays).filter((value): value is number => value !== null);
  const oldestDays = ordered.length > 0 && days.length === ordered.length ? Math.max(...days) : null;

  return {
    count: issues.length > 0 ? null : ordered.length,
    items: ordered,
    unknownItems: issues,
    oldestDays,
  };
}

function deriveReviewMetric(
  derived: OverviewDerivation | null,
): ProjectOverviewReviewMetric {
  if (!derived) {
    return {
      ...unknownDetails<DerivedTask>('reviewRounds', 'overview-model-unread'),
      reviewerNames: [],
      reviewerUnknown: false,
      oldestDays: null,
    };
  }
  const base = derived.taskCounts.byStatus.review;
  const reviewWaits = derived.waits.filter((wait) => wait.kind === 'review');
  const reviewerNames: string[] = [];
  const seenReviewers = new Set<string>();
  let reviewerUnknown = false;
  for (const wait of reviewWaits) {
    if (wait.waitingOnUnknown || wait.waitingOn.length === 0) {
      reviewerUnknown = true;
      continue;
    }
    for (const person of wait.waitingOn) {
      const name = cleanName(person.displayName);
      if (name) {
        if (!seenReviewers.has(name)) {
          seenReviewers.add(name);
          reviewerNames.push(name);
        }
      } else {
        reviewerUnknown = true;
      }
    }
  }
  const durations = reviewWaits.map((wait) => wait.durationDays).filter((value): value is number => value !== null);
  const oldestDays = reviewWaits.length > 0 && durations.length === reviewWaits.length
    ? Math.max(...durations)
    : null;
  return {
    ...base,
    reviewerNames,
    reviewerUnknown,
    oldestDays,
  };
}

function completionCountsForTasks(
  tasks: readonly DerivedTask[],
  tasksComplete: boolean,
): CompletionCounts | null {
  if (!tasksComplete) return null;
  if (tasks.some((task) => task.status === 'unknown')) return null;
  let verified = 0;
  let reported = 0;
  let inProgress = 0;
  let notStarted = 0;
  for (const task of tasks) {
    if (task.status === 'verified') verified += 1;
    else if (task.status === 'claimed') reported += 1;
    else if (task.status === 'ready' || task.status === 'planned') notStarted += 1;
    else inProgress += 1;
  }
  return { verified, reported, inProgress, notStarted };
}

function deriveWorkstreams(
  overview: TeamOverviewResponse | undefined,
  derived: OverviewDerivation | null,
  tasksComplete: boolean,
): ProjectOverviewWorkstream[] {
  if (!overview) return [];
  const grouped = groupWorks(overview.works ?? [], overview.workstreams ?? []);
  const derivedById = new Map<string, DerivedTask>();
  for (const task of derived?.tasks ?? []) {
    if (task.id) derivedById.set(task.id, task);
  }
  const waitsByTaskId = new Map<string, DerivedWait[]>();
  for (const wait of derived?.waits ?? []) {
    if (!wait.task.id) continue;
    const list = waitsByTaskId.get(wait.task.id) ?? [];
    list.push(wait);
    waitsByTaskId.set(wait.task.id, list);
  }

  const streamOrder: string[] = [];
  const streamTasks = new Map<string, DerivedTask[]>();
  for (const stream of overview.workstreams ?? []) {
    if (!stream.id || streamTasks.has(stream.id)) continue;
    streamOrder.push(stream.id);
    streamTasks.set(stream.id, []);
  }
  for (const work of grouped.works) {
    const streamId = work.workstream_id;
    if (!streamTasks.has(streamId)) {
      streamOrder.push(streamId);
      streamTasks.set(streamId, []);
    }
    const derivedTask = derivedById.get(work.key) ?? {
      id: work.key,
      title: cleanText(work.title),
      owner: null,
      agentName: null,
      agentActive: null,
      projectId: overview.project,
      groupId: null,
      goalIds: null,
      commitments: null,
      verifiedAtUnixMs: null,
      status: 'unknown' as const,
      unknownReasons: ['overview-model-unread'],
      verification: {
        everyCriterionSupported: null,
        independentReviewApproved: null,
        integrated: null,
      },
    };
    streamTasks.get(streamId)!.push(derivedTask);
  }

  return streamOrder.map((streamId) => {
    const tasks = streamTasks.get(streamId) ?? [];
    const blockedTasks = tasks.filter((task) => task.id !== null && (waitsByTaskId.get(task.id)?.length ?? 0) > 0);
    return {
      id: streamId,
      title: grouped.titles[streamId] ?? null,
      tasks,
      counts: completionCountsForTasks(tasks, tasksComplete && derived !== null),
      blockedTasks,
    };
  });
}

const PLANE_KEYS = ['report_received', 'authoritative', 'page_read'] as const;

/** Merge the existing overview and overview-model freshness planes without inventing measurements. */
export function mergeProjectFreshness(
  overviewFreshness: TeamFreshness | undefined,
  modelFreshness: TeamFreshness | undefined,
): TeamFreshness | undefined {
  const sources = [overviewFreshness, modelFreshness].filter((item): item is TeamFreshness => Boolean(item));
  if (sources.length === 0) return undefined;
  if (sources.length === 1) return sources[0];
  const plane = (key: (typeof PLANE_KEYS)[number]): PlaneFreshness => {
    const planes = sources.map((source) => source[key]);
    const status = planes.some((entry) => entry?.status === 'failed') ? 'failed'
      : planes.some((entry) => entry?.status === 'stale') ? 'stale'
        : planes.some((entry) => entry?.status === 'measured') ? 'measured'
          : 'not_measured';
    const times = planes
      .map((entry) => entry?.at_unix_ms)
      .filter((value): value is number => typeof value === 'number' && Number.isFinite(value));
    return { status, ...(times.length > 0 ? { at_unix_ms: Math.max(...times) } : {}) };
  };
  return {
    report_received: plane('report_received'),
    authoritative: plane('authoritative'),
    page_read: plane('page_read'),
  };
}

/**
 * Derive the complete Project Overview view model from already-read responses.
 * Pure, deterministic, no wall-clock reads: every count shares the exact array rendered by its detail view.
 */
export function deriveProjectOverview({
  projectKey,
  projectEntry,
  overview,
  model,
  timeZone = 'UTC',
  referenceUnixMs,
}: {
  projectKey: string;
  projectEntry?: ProjectListEntry;
  overview?: TeamOverviewResponse;
  model?: ProjectListModel;
  timeZone?: string;
  referenceUnixMs?: number | null;
}): ProjectOverviewModel {
  const entry: ProjectListEntry = projectEntry ?? { key: projectKey };
  const usableModel = model?.project === projectKey && model.input && model.input.options ? model : undefined;
  const orderingIncomparable = usableModel?.ordering === 'incomparable';
  const nowUnixMs = typeof referenceUnixMs === 'number' && Number.isFinite(referenceUnixMs)
    ? referenceUnixMs
    : usableModel?.input.options.nowUnixMs ?? null;

  let derived: OverviewDerivation | null = null;
  if (usableModel) {
    try {
      derived = deriveOverview(usableModel.input);
    } catch {
      derived = null;
    }
  }

  const members = usableModel?.input.members.coverage === 'unread' ? [] : (usableModel?.input.members.items ?? []);
  const tasksById = new Map<string, DerivedTask>();
  for (const task of derived?.tasks ?? []) {
    if (task.id) tasksById.set(task.id, task);
  }

  const listItem = deriveProjectListItem(entry, usableModel, timeZone, nowUnixMs);
  const health: ProjectHealth = orderingIncomparable ? 'unknown' : listItem.health;

  const leadId = cleanId(entry.leadPersonId);
  const leadName = listItem.leadName ?? resolveMemberName(members, leadId);
  const groupName = listItem.groupName;

  const tasksComplete = Boolean(derived && usableModel?.input.tasks.coverage === 'complete' && !orderingIncomparable);
  const taskCount: CountDetails<DerivedTask> = derived && !orderingIncomparable
    ? derived.taskCounts.all
    : unknownDetails<DerivedTask>('tasks', orderingIncomparable ? 'ordering-incomparable' : 'tasks-unread', derived?.tasks ?? []);

  const verified: CountDetails<DerivedTask> = derived && !orderingIncomparable
    ? derived.taskCounts.byStatus.verified
    : unknownDetails<DerivedTask>('tasks', orderingIncomparable ? 'ordering-incomparable' : 'tasks-unread', derived?.taskCounts.byStatus.verified.items ?? []);

  const claimed: CountDetails<DerivedTask> = derived && !orderingIncomparable
    ? derived.taskCounts.byStatus.claimed
    : unknownDetails<DerivedTask>('tasks', orderingIncomparable ? 'ordering-incomparable' : 'tasks-unread', derived?.taskCounts.byStatus.claimed.items ?? []);

  const review = orderingIncomparable
    ? {
      ...deriveReviewMetric(derived),
      count: null,
      unknownItems: [{ part: 'reviewRounds' as const, reason: 'ordering-incomparable' }],
    }
    : deriveReviewMetric(derived);

  const decisionsRaw = deriveDecisionsMetric(usableModel?.input.decisions, tasksById, members, nowUnixMs);
  const decisions: ProjectOverviewDecisionMetric = orderingIncomparable
    ? {
      ...decisionsRaw,
      count: null,
      unknownItems: [{ part: 'decisions', reason: 'ordering-incomparable' }],
    }
    : decisionsRaw;

  const workstreams = deriveWorkstreams(overview, derived, tasksComplete);
  const overviewScopeFailed = Boolean(
    overview && (overview.coverage.scope.status === 'failed' || overview.coverage.failed > 0 || overview.coverage.truncated),
  );
  const workstreamCount = !overview || overviewScopeFailed ? null : workstreams.length;

  // Recent period updates (up to 3, sorted newest first).
  const rawUpdates = usableModel?.periodUpdates;
  let recentUpdates: ProjectOverviewModel['recentUpdates'];
  if (!usableModel || rawUpdates === undefined) {
    recentUpdates = { items: [], total: null, coverage: usableModel ? 'absent' : 'unread' };
  } else if (rawUpdates.coverage === 'unread') {
    recentUpdates = { items: [], total: null, coverage: 'unread' };
  } else {
    const sorted = sortUpdatesDescending(rawUpdates.items, nowUnixMs)
      .map((item) => normalizeUpdate(item, members, nowUnixMs));
    recentUpdates = {
      items: sorted.slice(0, RECENT_UPDATE_LIMIT),
      total: rawUpdates.coverage === 'complete' ? sorted.length : null,
      coverage: rawUpdates.coverage,
    };
  }

  const latestUpdate = recentUpdates.items[0] ?? null;
  let verdictKind: ProjectOverviewVerdict['kind'];
  if (orderingIncomparable || listItem.summary.kind === 'unknown') {
    verdictKind = 'unknown';
  } else if (listItem.summary.kind === 'update' && latestUpdate) {
    verdictKind = 'update';
  } else if (listItem.summary.kind === 'last-update') {
    verdictKind = 'stale-update';
  } else {
    verdictKind = 'facts';
  }

  const verdict: ProjectOverviewVerdict = {
    kind: verdictKind,
    health,
    latestUpdate,
    daysSinceLastUpdate: listItem.summary.kind === 'last-update' ? listItem.summary.days : null,
    factCounts: {
      total: taskCount.count,
      verified: verified.count,
      review: review.count,
      decisions: decisions.count,
    },
  };

  // Approvals are intentionally unread on live bridges; avoid flagging them as an overview error.
  const trackedParts = OVERVIEW_PARTS.filter((part) => part !== 'approvals');
  const modelPartialParts = usableModel
    ? trackedParts.filter((part) => usableModel.coverage?.[part]?.status === 'partial' || usableModel.input[part]?.coverage === 'partial')
    : [];
  const modelUnreadParts = usableModel
    ? trackedParts.filter((part) => usableModel.coverage?.[part]?.status === 'unread' || usableModel.input[part]?.coverage === 'unread')
    : [];

  const hasUnknownFacts = orderingIncomparable
    || health === 'unknown'
    || verified.count === null
    || claimed.count === null
    || review.count === null
    || decisions.count === null
    || workstreams.some((stream) => stream.counts === null)
    || usableModel?.freshness.authoritative.status === 'not_measured';

  return {
    health,
    lead: { id: leadId, name: leadName },
    groupName,
    taskCount,
    workstreamCount,
    verdict,
    kpis: { verified, claimed, review, decisions },
    workstreams,
    recentUpdates,
    coverage: {
      modelPartialParts,
      modelUnreadParts,
      orderingIncomparable,
      hasUnknownFacts,
    },
    derived,
  };
}
