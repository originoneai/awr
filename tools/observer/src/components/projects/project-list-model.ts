import type { HealthStatus } from '../display/StatusTag';
import type { CompletionCounts } from '../display/CompletionBar';
import {
  deriveOverview,
  type CountDetails,
  type CoverageIssue,
  type DerivedTask,
  type OverviewInput,
  type ReadSlice,
} from '../../core/overview';
import type { PlaneFreshness, TeamFreshness, TeamOverviewModelResponse, TeamProject } from '../../lib/types';

const DAY_MS = 86_400_000;
const UPDATE_LATE_AFTER_DAYS = 7;

/** Optional project-list facts. Their absence is not silently replaced with a health judgement. */
export interface ProjectPeriodUpdate {
  id?: string | null;
  startsOn?: string | null;
  endsOn?: string | null;
  status?: string | null;
  summary?: string | null;
  recordedAtUnixMs?: number | null;
}

export interface ProjectTakeoverFact {
  resultId: string;
  recordedAtUnixMs?: number | null;
  verifiedTasks?: number | null;
  workstreams?: number | null;
  questions?: number | null;
}

export interface ProjectListEntry extends TeamProject {
  leadPersonId?: string | null;
  /** A display name, never a group identifier. */
  groupName?: string | null;
  /** Undefined means the read did not say; null is a recorded non-takeover project. */
  takeover?: ProjectTakeoverFact | null;
}

export interface ProjectListModel extends TeamOverviewModelResponse {
  periodUpdates?: ReadSlice<ProjectPeriodUpdate>;
}

export type ProjectHealth = HealthStatus | 'unknown';

export type ProjectSummary =
  | { kind: 'update'; text: string }
  | { kind: 'last-update'; days: number }
  | { kind: 'facts' }
  | { kind: 'unknown' }
  | { kind: 'takeover'; workstreams: number | null; verified: number | null; questions: number | null };

export interface ProjectCompletion {
  counts: CompletionCounts | null;
  all: CountDetails<DerivedTask>;
  verified: CountDetails<DerivedTask>;
  reported: CountDetails<DerivedTask>;
  inProgress: CountDetails<DerivedTask>;
  notStarted: CountDetails<DerivedTask>;
}

export interface ProjectListItem {
  entry: ProjectListEntry;
  health: ProjectHealth;
  summary: ProjectSummary;
  late: boolean;
  updatedAtUnixMs: number | null;
  updateId: string | null;
  leadName: string | null;
  groupName: string | null;
  showTakeoverCompletion: boolean;
  completion: ProjectCompletion;
}

export type ProjectSummaryCounts = {
  all: CountDetails<ProjectListItem>;
  risk: CountDetails<ProjectListItem>;
  noUpdate: CountDetails<ProjectListItem>;
  takeover: CountDetails<ProjectListItem>;
};

function issue(reason: string): CoverageIssue {
  return { part: 'tasks', reason };
}

function unknownDetails(items: ProjectListItem[], unknownItems: CoverageIssue[]): CountDetails<ProjectListItem> {
  return { count: unknownItems.length > 0 ? null : items.length, items, unknownItems };
}

function validDate(value: string | null | undefined): value is string {
  return typeof value === 'string' && /^\d{4}-\d{2}-\d{2}$/.test(value) && Number.isFinite(Date.parse(`${value}T00:00:00Z`));
}

function localDateKey(unixMs: number, timeZone: string): string | null {
  if (!Number.isFinite(unixMs) || unixMs < 0) return null;
  const parts = new Intl.DateTimeFormat('en-US', {
    year: 'numeric', month: '2-digit', day: '2-digit', timeZone,
  }).formatToParts(new Date(unixMs));
  const values = Object.fromEntries(parts.map((part) => [part.type, part.value]));
  return `${values.year}-${values.month}-${values.day}`;
}

function recordedTime(update: ProjectPeriodUpdate, nowUnixMs: number): number | null {
  const value = update.recordedAtUnixMs;
  return typeof value === 'number' && Number.isFinite(value) && value >= 0 && value <= nowUnixMs ? value : null;
}

function emptyCompletion(): ProjectCompletion {
  const unknownItems = [issue('project-facts-unread')];
  const empty = <T,>(): CountDetails<T> => ({ count: null, items: [], unknownItems });
  return {
    counts: null,
    all: empty(),
    verified: empty(),
    reported: empty(),
    inProgress: empty(),
    notStarted: empty(),
  };
}

function completionOf(input: OverviewInput): ProjectCompletion {
  let derived: ReturnType<typeof deriveOverview>;
  try {
    derived = deriveOverview(input);
  } catch {
    return emptyCompletion();
  }
  const byStatus = derived.taskCounts.byStatus;
  const inProgress: CountDetails<DerivedTask> = {
    count: [byStatus.review, byStatus.returned, byStatus.stalled, byStatus.running, byStatus.waiting]
      .every((part) => part.count !== null)
      ? [byStatus.review, byStatus.returned, byStatus.stalled, byStatus.running, byStatus.waiting]
        .reduce((sum, part) => sum + (part.count ?? 0), 0)
      : null,
    items: [byStatus.review, byStatus.returned, byStatus.stalled, byStatus.running, byStatus.waiting]
      .flatMap((part) => part.items),
    unknownItems: [byStatus.review, byStatus.returned, byStatus.stalled, byStatus.running, byStatus.waiting]
      .flatMap((part) => part.unknownItems),
  };
  const parts = [byStatus.verified, byStatus.claimed, inProgress, byStatus.ready, byStatus.planned];
  const counts = parts.every((part) => part.count !== null) ? {
    verified: byStatus.verified.count ?? 0,
    reported: byStatus.claimed.count ?? 0,
    inProgress: inProgress.count ?? 0,
    notStarted: (byStatus.ready.count ?? 0) + (byStatus.planned.count ?? 0),
  } : null;
  const notStarted: CountDetails<DerivedTask> = {
    count: byStatus.ready.count !== null && byStatus.planned.count !== null
      ? (byStatus.ready.count ?? 0) + (byStatus.planned.count ?? 0) : null,
    items: [...byStatus.ready.items, ...byStatus.planned.items],
    unknownItems: [...byStatus.ready.unknownItems, ...byStatus.planned.unknownItems],
  };
  return {
    counts,
    all: derived.taskCounts.all,
    verified: byStatus.verified,
    reported: byStatus.claimed,
    inProgress,
    notStarted,
  };
}

function statusOf(model: ProjectListModel, nowUnixMs: number, timeZone: string): Pick<ProjectListItem, 'health' | 'summary' | 'late' | 'updatedAtUnixMs' | 'updateId'> {
  const updates = model.periodUpdates;
  if (updates === undefined) {
    return { health: 'none', summary: { kind: 'facts' }, late: false, updatedAtUnixMs: null, updateId: null };
  }
  if (updates.coverage !== 'complete') {
    return { health: 'unknown', summary: { kind: 'unknown' }, late: false, updatedAtUnixMs: null, updateId: null };
  }
  const today = localDateKey(nowUnixMs, timeZone);
  const dated = updates.items.filter((update) => validDate(update.startsOn) && validDate(update.endsOn));
  const ordered = [...dated].sort((left, right) => (left.endsOn ?? '').localeCompare(right.endsOn ?? '')
    || (left.startsOn ?? '').localeCompare(right.startsOn ?? '')
    || (recordedTime(left, nowUnixMs) ?? -1) - (recordedTime(right, nowUnixMs) ?? -1));
  const latest = ordered.at(-1);
  const current = today === null ? undefined : ordered.filter((update) => (update.startsOn ?? '') <= today && today <= (update.endsOn ?? '')).at(-1);
  if (current) {
    if (!['ok', 'risk', 'off'].includes(current.status ?? '')) {
      return { health: 'unknown', summary: { kind: 'unknown' }, late: false, updatedAtUnixMs: null, updateId: null };
    }
    const summary = typeof current.summary === 'string' && current.summary.trim()
      ? { kind: 'update' as const, text: current.summary.trim() }
      : { kind: 'facts' as const };
    return {
      health: current.status as 'ok' | 'risk' | 'off',
      summary,
      late: false,
      updatedAtUnixMs: recordedTime(current, nowUnixMs),
      updateId: typeof current.id === 'string' && current.id ? current.id : null,
    };
  }
  const updatedAtUnixMs = latest ? recordedTime(latest, nowUnixMs) : null;
  const days = updatedAtUnixMs === null ? null : Math.floor((nowUnixMs - updatedAtUnixMs) / DAY_MS);
  return {
    health: 'none',
    summary: days === null ? { kind: 'facts' } : { kind: 'last-update', days },
    late: days !== null && days > UPDATE_LATE_AFTER_DAYS,
    updatedAtUnixMs,
    updateId: typeof latest?.id === 'string' && latest.id ? latest.id : null,
  };
}

/** Combine the canonical task derivation with the optional phase-update/takeover facts. */
export function deriveProjectListItem(
  entry: ProjectListEntry,
  model: ProjectListModel | undefined,
  timeZone = 'UTC',
  /** The display clock (AWR-PPO-079): the model read's derivation clock walked forward.
   * Unset keeps the reply's own recorded clock, exactly the pre-PPO-079 behavior. */
  referenceUnixMs?: number | null,
): ProjectListItem {
  if (entry.takeover) {
    return {
      entry,
      health: 'info',
      summary: {
        kind: 'takeover',
        workstreams: entry.takeover.workstreams ?? null,
        verified: entry.takeover.verifiedTasks ?? null,
        questions: entry.takeover.questions ?? null,
      },
      late: false,
      updatedAtUnixMs: typeof entry.takeover.recordedAtUnixMs === 'number' && Number.isFinite(entry.takeover.recordedAtUnixMs)
        ? entry.takeover.recordedAtUnixMs : null,
      updateId: null,
      leadName: null,
      groupName: null,
      showTakeoverCompletion: entry.takeover.verifiedTasks === 0,
      completion: model ? completionOf(model.input) : emptyCompletion(),
    };
  }
  const completion = model ? completionOf(model.input) : emptyCompletion();
  const nowUnixMs = typeof referenceUnixMs === 'number' && Number.isFinite(referenceUnixMs)
    ? referenceUnixMs
    : model?.input.options.nowUnixMs;
  const status = model && Number.isFinite(nowUnixMs)
    ? statusOf(model, nowUnixMs as number, timeZone)
    : { health: 'unknown' as const, summary: { kind: 'unknown' as const }, late: false, updatedAtUnixMs: null, updateId: null };
  const lead = (model?.input.members.items ?? []).find((member) => member.personId === entry.leadPersonId);
  return {
    entry,
    ...status,
    leadName: typeof lead?.displayName === 'string' && lead.displayName.trim() ? lead.displayName.trim() : null,
    groupName: typeof entry.groupName === 'string' && entry.groupName.trim() ? entry.groupName.trim() : null,
    showTakeoverCompletion: false,
    completion,
  };
}

export function summarizeProjects(items: readonly ProjectListItem[]): ProjectSummaryCounts {
  const category = (predicate: (item: ProjectListItem) => boolean) => unknownDetails(
    items.filter(predicate),
    items.filter((item) => item.health === 'unknown').map(() => issue('project-status-unread')),
  );
  return {
    all: unknownDetails([...items], []),
    risk: category((item) => item.health === 'risk'),
    noUpdate: category((item) => item.health === 'none'),
    takeover: unknownDetails(
      items.filter((item) => item.health === 'info'),
      items.filter((item) => item.entry.takeover === undefined).map(() => issue('takeover-facts-unread')),
    ),
  };
}

const PLANE_KEYS = ['report_received', 'authoritative', 'page_read'] as const;

/** Aggregate only the existing read freshness; this says nothing about project health. */
export function projectListFreshness(models: readonly ProjectListModel[]): TeamFreshness {
  const plane = (key: (typeof PLANE_KEYS)[number]): PlaneFreshness => {
    const planes = models.map((model) => model.freshness[key]);
    const status = planes.some((plane) => plane?.status === 'failed') ? 'failed'
      : planes.some((plane) => plane?.status === 'stale') ? 'stale'
        : planes.every((plane) => plane?.status === 'measured') ? 'measured' : 'not_measured';
    const times = planes.map((plane) => plane?.at_unix_ms)
      .filter((value): value is number => typeof value === 'number' && Number.isFinite(value));
    return { status, ...(times.length ? { at_unix_ms: Math.max(...times) } : {}) };
  };
  return {
    report_received: plane('report_received'),
    authoritative: plane('authoritative'),
    page_read: plane('page_read'),
  };
}
