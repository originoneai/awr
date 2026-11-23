import type { UseQueryOptions } from '@tanstack/react-query';
import type { CompletionCounts } from '../display/CompletionBar';
import { groupWorks } from '../../core/observation';
import { readsAsName } from '../../core/observation/owner';
import { statementText } from '../../core/observation/values';
import {
  deriveOverview,
  OVERVIEW_PARTS,
  type CountDetails,
  type CoverageIssue,
  type DerivedTask,
  type DerivedTaskStatus,
  type OverviewApproval,
  type OverviewDerivation,
  type OverviewPart,
} from '../../core/overview';
import { ApiError, apiPost, isAuthError } from '../../lib/api';
import { readQueryDefaults, SNAPSHOT_POLL_INTERVAL_MS } from '../../lib/queries';
import type {
  PlanningCandidatesData,
  TeamOverviewResponse,
  TeamReadResponse,
  TeamWork,
} from '../../lib/types';
import type { ProjectListModel } from './project-list-model';

export const WORK_PLAN_FILTERS = ['all', 'open', 'review', 'claimed', 'done'] as const;
export type WorkPlanFilterId = (typeof WORK_PLAN_FILTERS)[number];

export interface ProjectGoalEntry {
  goal_id: string;
  project_id?: string;
  title: string;
  status?: string | null;
  health?: string | null;
}

export interface ProjectGoalsData {
  items: ProjectGoalEntry[];
  total: number;
  next_cursor: string | null;
  truncated?: boolean;
}

export interface WorkPlanGoalRef {
  id: string;
  title: string;
}

export interface WorkPlanPendingBreakdown {
  goalTitle: string | null;
  taskCount: number | null;
  authorName: string | null;
  proposedAtUnixMs: number | null;
  inboxDetailKey: string | null;
}

export interface WorkPlanTaskRow {
  id: string;
  title: string | null;
  workstreamId: string;
  status: DerivedTaskStatus;
  agentActive: boolean | null;
  ownerName: string | null;
  ownerId: string | null;
  nextStep: string | null;
  highlighted: boolean;
}

export interface WorkPlanGroup {
  id: string;
  title: string | null;
  allTasks: WorkPlanTaskRow[];
  visibleTasks: WorkPlanTaskRow[];
  /** Null whenever task statuses are incomplete or any task in the group has an unknown status. */
  counts: CompletionCounts | null;
  /** Null when scope coverage is incomplete; otherwise allTasks.length. */
  totalCount: number | null;
}

export interface WorkPlanModel {
  allTasks: WorkPlanTaskRow[];
  filters: Record<WorkPlanFilterId, CountDetails<WorkPlanTaskRow>>;
  groups: WorkPlanGroup[];
  visibleGroups: WorkPlanGroup[];
  totalVisibleTasks: number;
  pendingBreakdown: WorkPlanPendingBreakdown | null;
  goal: WorkPlanGoalRef | null;
  activeStreamId: string | null;
  activeStreamTitle: string | null;
  coverage: {
    overviewScopeFailed: boolean;
    modelPartialParts: OverviewPart[];
    modelUnreadParts: OverviewPart[];
    orderingIncomparable: boolean;
    hasUnknownStatus: boolean;
    hasUnknownFacts: boolean;
  };
  derived: OverviewDerivation | null;
}

export function asWorkPlanFilter(value: string | null): WorkPlanFilterId {
  return value && (WORK_PLAN_FILTERS as readonly string[]).includes(value)
    ? (value as WorkPlanFilterId)
    : 'all';
}

export function projectGoalsQueryOptions(project: string, revoked: boolean) {
  return {
    queryKey: ['team', 'goals.list', project] as const,
    meta: { failureScope: 'object' },
    queryFn: ({ signal }: { signal: AbortSignal }) =>
      apiPost<TeamReadResponse<ProjectGoalsData>>(
        '/api/team/read',
        { project, query: { protocol_version: 1, op: 'goals.list', limit: 20, max_context_bytes: 262144 } },
        signal,
      ),
    enabled: Boolean(project) && !revoked,
    ...readQueryDefaults,
    retry: (count: number, error: unknown) =>
      !isAuthError(error)
      && !(error instanceof ApiError && (error.code === 'Unsupported' || error.code === 'CursorExpired'))
      && count < 1,
    refetchInterval: SNAPSHOT_POLL_INTERVAL_MS,
  } satisfies UseQueryOptions<TeamReadResponse<ProjectGoalsData>>;
}

function cleanText(value: unknown): string | null {
  return typeof value === 'string' && value.trim().length > 0 ? value.trim() : null;
}

function cleanName(value: unknown): string | null {
  const trimmed = cleanText(value);
  return trimmed && readsAsName(trimmed) ? trimmed : null;
}

function validTimestamp(value: number | null | undefined, maxUnixMs?: number): number | null {
  if (typeof value !== 'number' || !Number.isFinite(value) || value < 0) return null;
  if (maxUnixMs !== undefined && value > maxUnixMs) return null;
  return value;
}

/** Extract the quoted goal title from a plan-approval summary like '“完善异常恢复”拆成了 4 件事'. */
function extractBreakdownGoalTitle(rawTitle: string | null): string | null {
  if (!rawTitle) return null;
  const quoted = /^[“"「]([^”"」]+)[”"」]/.exec(rawTitle);
  if (quoted && quoted[1].trim().length > 0) return quoted[1].trim();
  return rawTitle;
}

function matchesFilter(task: WorkPlanTaskRow, filter: WorkPlanFilterId): boolean {
  switch (filter) {
    case 'all':
      return true;
    case 'open':
      return task.status !== 'verified' && task.status !== 'unknown';
    case 'review':
      return task.status === 'review';
    case 'claimed':
      return task.status === 'claimed';
    case 'done':
      return task.status === 'verified';
  }
}

function completionCountsForTasks(
  tasks: readonly WorkPlanTaskRow[],
  statusesComplete: boolean,
): CompletionCounts | null {
  if (!statusesComplete) return null;
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

function isPendingCandidate(item: PlanningCandidatesData['items'][number]): boolean {
  return item.state === 'drafting'
    && !item.approval.value
    && !item.publication.value
    && !item.activation.value;
}

function derivePendingBreakdown({
  projectKey,
  planningCandidates,
  approvals,
  nowUnixMs,
}: {
  projectKey: string;
  planningCandidates?: PlanningCandidatesData;
  approvals: readonly OverviewApproval[];
  nowUnixMs: number | null;
}): WorkPlanPendingBreakdown | null {
  const candidate = planningCandidates?.items.find(isPendingCandidate) ?? null;
  const approval = approvals.find((entry) =>
    entry.kind === 'plan-approval' && (entry.projectId == null || entry.projectId === projectKey)) ?? null;

  if (!candidate && !approval) return null;

  const candidateTitle = candidate && 'title' in candidate
    ? cleanText((candidate as { title?: unknown }).title)
    : null;
  const approvalTitle = extractBreakdownGoalTitle(cleanText(approval?.title));
  const goalTitle = candidateTitle ?? approvalTitle ?? null;

  let taskCount: number | null = null;
  if (candidate) {
    const added = candidate.current_diff.counts.added;
    const itemLen = candidate.current_diff.items.length;
    if (added > 0) taskCount = added;
    else if (itemLen > 0) taskCount = itemLen;
  }

  const authorName = cleanName(candidate?.author?.display_name);
  const proposedAtUnixMs = validTimestamp(candidate?.updated_at_unix_ms, nowUnixMs ?? undefined)
    ?? validTimestamp(approval?.sinceUnixMs, nowUnixMs ?? undefined);

  const approvalId = cleanText(approval?.id);
  const recipientId = cleanText(approval?.recipientPersonId);
  const inboxDetailKey = approvalId && recipientId
    ? `plan-approval:${approvalId}:${recipientId}`
    : null;

  return {
    goalTitle,
    taskCount,
    authorName,
    proposedAtUnixMs,
    inboxDetailKey,
  };
}

function deriveGoalRef(goals?: readonly ProjectGoalEntry[]): WorkPlanGoalRef | null {
  if (!Array.isArray(goals)) return null;
  for (const goal of goals) {
    const id = cleanText(goal?.goal_id);
    const title = cleanText(goal?.title);
    if (id && title) return { id, title };
  }
  return null;
}

function buildRowFromWork(
  work: TeamWork,
  derivedTask: DerivedTask | undefined,
  orderingIncomparable: boolean,
): WorkPlanTaskRow {
  const status: DerivedTaskStatus = orderingIncomparable
    ? 'unknown'
    : (derivedTask?.status ?? 'unknown');
  const ownerName = cleanName(derivedTask?.owner?.displayName)
    ?? cleanName(statementText(work.owner_person));
  return {
    id: work.key,
    title: cleanText(work.title) ?? cleanText(derivedTask?.title),
    workstreamId: work.workstream_id,
    status,
    agentActive: derivedTask?.agentActive ?? null,
    ownerName,
    ownerId: derivedTask?.owner?.id ?? null,
    nextStep: cleanText(work.next_step),
    highlighted: status === 'returned',
  };
}

function buildRowFromDerived(
  derivedTask: DerivedTask,
  orderingIncomparable: boolean,
): WorkPlanTaskRow | null {
  const id = cleanText(derivedTask.id);
  if (!id) return null;
  const status: DerivedTaskStatus = orderingIncomparable ? 'unknown' : derivedTask.status;
  return {
    id,
    title: cleanText(derivedTask.title),
    workstreamId: '__ungrouped__',
    status,
    agentActive: derivedTask.agentActive ?? null,
    ownerName: cleanName(derivedTask.owner?.displayName),
    ownerId: derivedTask.owner?.id ?? null,
    nextStep: null,
    highlighted: status === 'returned',
  };
}

/**
 * Derive the complete Project Work Plan view model from already-read responses.
 * Pure, deterministic, no wall-clock reads: every filter count shares the exact task array
 * rendered when that filter is active.
 */
export function deriveWorkPlan({
  projectKey,
  overview,
  model,
  planningCandidates,
  goals,
  streamFilter = null,
  activeFilter = 'all',
  referenceUnixMs = null,
}: {
  projectKey: string;
  overview?: TeamOverviewResponse;
  model?: ProjectListModel;
  planningCandidates?: PlanningCandidatesData;
  goals?: readonly ProjectGoalEntry[];
  streamFilter?: string | null;
  activeFilter?: WorkPlanFilterId;
  referenceUnixMs?: number | null;
}): WorkPlanModel {
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

  const derivedById = new Map<string, DerivedTask>();
  for (const task of derived?.tasks ?? []) {
    if (task.id) derivedById.set(task.id, task);
  }

  const normalizedStreamFilter = cleanText(streamFilter);
  const overviewScopeFailed = Boolean(
    overview && (overview.coverage.scope.status === 'failed' || overview.coverage.failed > 0 || overview.coverage.truncated),
  );
  const modelTasksComplete = Boolean(
    derived && usableModel?.input.tasks.coverage === 'complete' && !orderingIncomparable,
  );

  // Build ordered workstream groups from overview (or fall back to derived tasks when overview failed).
  const streamOrder: string[] = [];
  const streamTitleById = new Map<string, string | null>();
  const streamRowsById = new Map<string, WorkPlanTaskRow[]>();

  if (overview) {
    const grouped = groupWorks(overview.works ?? [], overview.workstreams ?? []);
    for (const stream of overview.workstreams ?? []) {
      if (!stream.id || streamRowsById.has(stream.id)) continue;
      streamOrder.push(stream.id);
      streamTitleById.set(stream.id, cleanText(grouped.titles[stream.id]));
      streamRowsById.set(stream.id, []);
    }
    for (const work of grouped.works) {
      const wsId = work.workstream_id;
      if (!streamRowsById.has(wsId)) {
        streamOrder.push(wsId);
        streamTitleById.set(wsId, cleanText(grouped.titles[wsId]));
        streamRowsById.set(wsId, []);
      }
      streamRowsById.get(wsId)!.push(buildRowFromWork(work, derivedById.get(work.key), orderingIncomparable));
    }
  } else if (derived) {
    const fallbackRows = derived.tasks
      .map((task) => buildRowFromDerived(task, orderingIncomparable))
      .filter((row): row is WorkPlanTaskRow => row !== null);
    if (fallbackRows.length > 0) {
      streamOrder.push('__ungrouped__');
      streamTitleById.set('__ungrouped__', null);
      streamRowsById.set('__ungrouped__', fallbackRows);
    }
  }

  // Apply optional ?stream= scope when the requested stream exists (or filter to empty if explicitly scoped).
  const scopedStreamIds = normalizedStreamFilter
    ? streamOrder.filter((id) => id === normalizedStreamFilter)
    : streamOrder;
  const activeStreamId = normalizedStreamFilter;
  const activeStreamTitle = normalizedStreamFilter
    ? (streamTitleById.get(normalizedStreamFilter) ?? null)
    : null;

  const allTasks = scopedStreamIds.flatMap((id) => streamRowsById.get(id) ?? []);
  const hasUnknownStatus = allTasks.some((task) => task.status === 'unknown');
  const allCountComplete = Boolean(
    !orderingIncomparable
    && !overviewScopeFailed
    && (overview !== undefined || modelTasksComplete)
    && (!usableModel || usableModel.input.tasks.coverage === 'complete'),
  );
  const statusesComplete = allCountComplete && modelTasksComplete && !hasUnknownStatus;

  const makeFilterDetails = (filterId: WorkPlanFilterId): CountDetails<WorkPlanTaskRow> => {
    const items = allTasks.filter((task) => matchesFilter(task, filterId));
    const complete = filterId === 'all' ? allCountComplete : statusesComplete;
    const unknownItems: CoverageIssue[] = complete
      ? []
      : [{
        part: 'tasks',
        reason: orderingIncomparable
          ? 'ordering-incomparable'
          : overviewScopeFailed
            ? 'overview-scope-incomplete'
            : hasUnknownStatus
              ? 'task-status-unknown'
              : 'tasks-incomplete',
      }];
    return {
      count: complete ? items.length : null,
      items,
      unknownItems,
    };
  };

  const filters: Record<WorkPlanFilterId, CountDetails<WorkPlanTaskRow>> = {
    all: makeFilterDetails('all'),
    open: makeFilterDetails('open'),
    review: makeFilterDetails('review'),
    claimed: makeFilterDetails('claimed'),
    done: makeFilterDetails('done'),
  };

  const groups: WorkPlanGroup[] = scopedStreamIds.map((streamId) => {
    const tasks = streamRowsById.get(streamId) ?? [];
    const visibleTasks = tasks.filter((task) => matchesFilter(task, activeFilter));
    return {
      id: streamId,
      title: streamTitleById.get(streamId) ?? null,
      allTasks: tasks,
      visibleTasks,
      counts: completionCountsForTasks(tasks, statusesComplete),
      totalCount: allCountComplete ? tasks.length : null,
    };
  });

  const visibleGroups = groups.filter((group) => group.visibleTasks.length > 0);
  const totalVisibleTasks = visibleGroups.reduce((sum, group) => sum + group.visibleTasks.length, 0);

  const approvals = usableModel?.input.approvals.coverage === 'unread'
    ? []
    : (usableModel?.input.approvals.items ?? []);
  const pendingBreakdown = derivePendingBreakdown({
    projectKey,
    planningCandidates,
    approvals,
    nowUnixMs,
  });

  const goal = deriveGoalRef(goals);

  const trackedParts = OVERVIEW_PARTS.filter((part) => part !== 'approvals');
  const modelPartialParts = usableModel
    ? trackedParts.filter((part) =>
      usableModel.coverage?.[part]?.status === 'partial' || usableModel.input[part]?.coverage === 'partial')
    : [];
  const modelUnreadParts = usableModel
    ? trackedParts.filter((part) =>
      usableModel.coverage?.[part]?.status === 'unread' || usableModel.input[part]?.coverage === 'unread')
    : [];

  const hasUnknownFacts = orderingIncomparable
    || hasUnknownStatus
    || !statusesComplete
    || (usableModel
      ? usableModel.freshness.authoritative.status === 'not_measured'
      : overview?.freshness.authoritative.status === 'not_measured');

  return {
    allTasks,
    filters,
    groups,
    visibleGroups,
    totalVisibleTasks,
    pendingBreakdown,
    goal,
    activeStreamId,
    activeStreamTitle,
    coverage: {
      overviewScopeFailed,
      modelPartialParts,
      modelUnreadParts,
      orderingIncomparable,
      hasUnknownStatus,
      hasUnknownFacts,
    },
    derived,
  };
}
