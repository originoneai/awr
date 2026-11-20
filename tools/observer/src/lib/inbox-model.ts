/**
 * The cross-project inbox read model (AWR-PPO-030): the waits that really sit with one
 * member — decisions they can answer, plan breakdowns awaiting their approval, work
 * assigned to them and not yet accepted, and candidate lessons awaiting their
 * confirmation. Chain blockers (review queues, upstream dependencies, outside parties)
 * never enter the inbox; they stay on the chain views.
 *
 * Pure derivation over already-read overview models; no I/O, no wall clock, no identity
 * guesses. The viewer is only ever `options.views.me.personId` — never an actor id, an
 * owner field, or any other plane.
 */
import { deriveOverview, OVERVIEW_PARTS } from '../core/overview';
import type { DerivedTask, DerivedWait, OverviewInput, OverviewPart, OverviewTask, PersonRef } from '../core/overview';
import type { HandbackKind } from '../i18n/handback';
import { taskPath } from './task-path';
import type { PlaneFreshness, TeamFreshness, TeamOverviewModelResponse } from './types';

export type InboxKind = HandbackKind;
export type InboxBasisTone = 'ok' | 'bad' | 'info';

/** A basis fact the page words through the vocabulary; never a raw record. */
export type InboxBasisFact =
  | { fact: 'task'; title: string }
  | { fact: 'reportAge'; days: number; stale: boolean }
  | { fact: 'noReport' }
  | { fact: 'independentStart' }
  | { fact: 'blockedByDependencies'; count: number };

export interface InboxBasisItem {
  tone: InboxBasisTone;
  fact: InboxBasisFact;
}

export interface InboxEntry {
  key: string;
  kind: InboxKind;
  /** True when this entry waits for the viewer; only those enter the queue. */
  mine: boolean;
  /** Untrusted display text from the records; the page owns fallbacks and truncation. */
  title: string | null;
  projectName: string | null;
  projectKey: string;
  taskName: string | null;
  taskId: string | null;
  executionAgentName: string | null;
  /** The decision question, for the answer and decision kinds. */
  question: string | null;
  /** The impact the decision record itself states, when it states one. */
  impactText: string | null;
  /** How many tasks sit behind the waiting task; null is unknown, never zero-coerced. */
  impactBehindCount: number | null;
  /** Whole days since the wait began; null is unknown and sorts last. */
  durationDays: number | null;
  sinceUnixMs: number | null;
  /** The member this entry is waiting for. */
  recipient: PersonRef;
  /** Who dispatched the assignment; null for the other kinds. */
  fromPerson: PersonRef | null;
  /** Owner of the task a decision waits on; used to tell an answer from a decision. */
  taskOwner: PersonRef | null;
  basis: InboxBasisItem[];
  /** The derived wait, present for the answer and decision kinds. */
  wait: DerivedWait | null;
}

export interface InboxProjectRef {
  key: string;
  title: string | null;
}

export interface InboxModel {
  viewer: PersonRef | null;
  /** The viewer's pending entries, longest wait first; unknown ages last. */
  entries: InboxEntry[];
  /** Every pending entry in the composed planes, including other members'. */
  allEntries: InboxEntry[];
  /** Confirmed count of the viewer's entries; null when no read established a basis. */
  count: number | null;
  /** True when parts of the pending planes stayed unread; the count is a lower bound. */
  partial: boolean;
  /** Whole project reads that failed; those projects' pending items are unknown. */
  failedReads: number;
  /** True when the read model does not say which member is viewing. */
  viewerUnknown: boolean;
}

/** Reads the models that produced a usable overview input. */
export function inboxUsableModels(models: readonly TeamOverviewModelResponse[]): TeamOverviewModelResponse[] {
  // An incomparable ordering means the reply could not confirm its own currency.
  return models.filter((model) => model?.ok === true && model.ordering !== 'incomparable');
}

/**
 * A reply counts as a usable inbox source only when it carries the whole composed
 * plane: every part slice and the derivation options. The derivation is strict; an
 * envelope that fails this check is a failed read, never an application error.
 */
export function isUsableInboxModel(value: unknown): value is TeamOverviewModelResponse {
  if (value == null || typeof value !== 'object') return false;
  const model = value as Partial<TeamOverviewModelResponse>;
  if (model.ok !== true || model.ordering === 'incomparable') return false;
  const input = model.input as Partial<Record<OverviewPart, unknown>> & { options?: unknown } | undefined;
  if (input == null || typeof input !== 'object') return false;
  for (const part of OVERVIEW_PARTS) {
    const slice = input[part] as { coverage?: unknown } | undefined;
    if (slice == null || typeof slice !== 'object' || typeof slice.coverage !== 'string') return false;
  }
  const options = input.options as { nowUnixMs?: unknown; stalledAfterDays?: unknown } | undefined;
  return options != null && typeof options === 'object'
    && typeof options.nowUnixMs === 'number' && Number.isFinite(options.nowUnixMs) && (options.nowUnixMs as number) >= 0
    && typeof options.stalledAfterDays === 'number' && Number.isFinite(options.stalledAfterDays) && (options.stalledAfterDays as number) >= 0;
}

function validId(value: unknown): string | null {
  return typeof value === 'string' && value.trim().length > 0 ? value : null;
}

function personNamed(members: readonly { personId?: string | null; displayName?: string | null }[], personId: string): PersonRef {
  const member = members.find((candidate) => candidate.personId === personId);
  const displayName = member && typeof member.displayName === 'string' && member.displayName.trim() !== ''
    ? member.displayName : null;
  return { id: personId, displayName };
}

function compareEntries(left: InboxEntry, right: InboxEntry): number {
  const leftAge = left.durationDays;
  const rightAge = right.durationDays;
  if (leftAge == null && rightAge == null) return left.key.localeCompare(right.key);
  if (leftAge == null) return 1;
  if (rightAge == null) return -1;
  if (rightAge !== leftAge) return rightAge - leftAge;
  return left.key.localeCompare(right.key);
}

function reportBasis(task: OverviewTask | undefined, agentName: string | null, stalledAfterDays: number, nowUnixMs: number): InboxBasisItem[] {
  if (agentName == null) return [];
  const reportedAt = task?.report?.reportedAtUnixMs;
  if (typeof reportedAt !== 'number' || !Number.isFinite(reportedAt) || reportedAt < 0 || reportedAt > nowUnixMs) {
    return [{ tone: 'bad', fact: { fact: 'noReport' } }];
  }
  const days = Math.floor((nowUnixMs - reportedAt) / 86_400_000);
  const stale = days >= stalledAfterDays;
  return [{ tone: stale ? 'bad' : 'info', fact: { fact: 'reportAge', days, stale } }];
}

interface ProjectEntries {
  entries: InboxEntry[];
  /** Any pending plane (or a pending attribution) stayed unread or partial. */
  pendingPlanesIncomplete: boolean;
  /** Every pending plane is unread: the project established no basis at all. */
  pendingPlanesAllUnread: boolean;
}

function projectEntries(model: TeamOverviewModelResponse, projectName: string | null): ProjectEntries {
  const input = model.input;
  const derived = deriveOverview(input);
  const projectKey = validId(model.project) ?? '';
  const viewerPersonId = validId(input.options.views?.me?.personId);
  const members = input.members.coverage === 'unread' ? [] : input.members.items;
  const tasksById = new Map<string, DerivedTask>();
  for (const task of derived.tasks) {
    if (task.id != null && !tasksById.has(task.id)) tasksById.set(task.id, task);
  }
  const inputTaskById = new Map<string, OverviewTask>();
  if (input.tasks.coverage !== 'unread') {
    for (const task of input.tasks.items) {
      const id = validId(task?.id);
      if (id != null && !inputTaskById.has(id)) inputTaskById.set(id, task);
    }
  }

  const entries: InboxEntry[] = [];
  let attributionGap = false;

  // Decisions the responder can answer: an "answer" on the responder's own task,
  // a "decision" on somebody else's. Review, external and dependency waits are chain
  // blockers and never enter the inbox.
  for (const wait of derived.waits) {
    if (wait.kind !== 'decision') continue;
    if (wait.waitingOnUnknown || wait.waitingOn.length === 0) {
      attributionGap = true;
      continue;
    }
    for (const responder of wait.waitingOn) {
      const responderId = validId(responder.id);
      if (responderId == null) continue;
      const task = wait.task;
      const taskId = task.id;
      const owner = taskOwner(task, responderId);
      const kind: InboxKind = owner === true ? 'answer' : owner === false ? 'decision' : 'decision';
      const inputTask = taskId == null ? undefined : inputTaskById.get(taskId);
      const agentName = task.agentName;
      const basis: InboxBasisItem[] = taskId == null || task.title == null ? [] : [{ tone: 'info', fact: { fact: 'task', title: task.title } }];
      basis.push(...reportBasis(inputTask, agentName, input.options.stalledAfterDays, input.options.nowUnixMs));
      entries.push({
        key: `${kind}:${wait.sourceId ?? `task:${taskId ?? 'unknown'}`}:${responderId}`,
        kind,
        mine: responderId === viewerPersonId,
        title: wait.what,
        projectName,
        projectKey,
        taskName: task.title,
        taskId,
        executionAgentName: agentName,
        question: wait.what,
        impactText: decisionImpact(input, wait),
        impactBehindCount: wait.behind.count,
        durationDays: wait.durationDays,
        sinceUnixMs: wait.startedAtUnixMs,
        recipient: personNamed(members, responderId),
        fromPerson: null,
        taskOwner: task.owner,
        basis,
        wait,
      });
    }
  }

  // Work assigned to a member and not yet accepted.
  if (input.handoffs.coverage !== 'unread') {
    for (const [index, handoff] of input.handoffs.items.entries()) {
      if (handoff?.pending == null || validId(handoff.toPersonId) == null) {
        attributionGap = true;
        continue;
      }
      if (handoff.pending !== true) continue;
      const recipientId = validId(handoff.toPersonId)!;
      const taskId = validId(handoff.taskId);
      const task = taskId == null ? null : tasksById.get(taskId) ?? null;
      const dependencyWaits = derived.waits.filter((wait) => wait.kind === 'dependency' && wait.task.id === taskId);
      const blocked = dependencyWaits.length;
      const basis: InboxBasisItem[] = task?.title != null ? [{ tone: 'info', fact: { fact: 'task', title: task.title } }] : [];
      basis.push(blocked === 0
        ? { tone: 'ok', fact: { fact: 'independentStart' } }
        : { tone: 'bad', fact: { fact: 'blockedByDependencies', count: blocked } });
      const fromId = validId(handoff.fromPersonId);
      entries.push({
        key: `assignment:${validId(handoff.id) ?? `task:${taskId ?? index}`}:${recipientId}`,
        kind: 'assignment',
        mine: recipientId === viewerPersonId,
        title: task?.title ?? null,
        projectName,
        projectKey,
        taskName: task?.title ?? null,
        taskId,
        executionAgentName: task?.agentName ?? null,
        question: null,
        impactText: null,
        impactBehindCount: null,
        durationDays: wholeDays(handoff.updatedAtUnixMs, input.options.nowUnixMs),
        sinceUnixMs: handoff.updatedAtUnixMs ?? null,
        recipient: personNamed(members, recipientId),
        fromPerson: fromId == null ? null : personNamed(members, fromId),
        taskOwner: task?.owner ?? null,
        basis,
        wait: null,
      });
    }
  }

  // Plan breakdowns awaiting approval and candidate lessons awaiting confirmation.
  if (input.approvals.coverage !== 'unread') {
    for (const [index, approval] of input.approvals.items.entries()) {
      if (approval == null || (approval.kind !== 'plan-approval' && approval.kind !== 'lesson-confirmation')) {
        attributionGap = true;
        continue;
      }
      const recipientId = validId(approval.recipientPersonId);
      if (recipientId == null) {
        attributionGap = true;
        continue;
      }
      const kind: InboxKind = approval.kind === 'lesson-confirmation' ? 'lesson' : 'plan-approval';
      const title = typeof approval.title === 'string' && approval.title.trim() !== '' ? approval.title : null;
      entries.push({
        key: `${kind}:${validId(approval.id) ?? `item:${index}`}:${recipientId}`,
        kind,
        mine: recipientId === viewerPersonId,
        title,
        projectName,
        projectKey: validId(approval.projectId) ?? projectKey,
        taskName: title,
        taskId: null,
        executionAgentName: null,
        question: null,
        impactText: null,
        impactBehindCount: null,
        durationDays: wholeDays(approval.sinceUnixMs, input.options.nowUnixMs),
        sinceUnixMs: approval.sinceUnixMs ?? null,
        recipient: personNamed(members, recipientId),
        fromPerson: null,
        taskOwner: null,
        basis: [],
        wait: null,
      });
    }
  }

  const planes = [input.decisions.coverage, input.handoffs.coverage, input.approvals.coverage];
  return {
    entries,
    pendingPlanesIncomplete: attributionGap || planes.some((coverage) => coverage !== 'complete'),
    pendingPlanesAllUnread: planes.every((coverage) => coverage === 'unread'),
  };
}

/** True when the task is the responder's own; null keeps the honest "decision" label. */
function taskOwner(task: DerivedTask, responderId: string): boolean | null {
  const ownerId = task.owner?.id;
  if (ownerId == null) return null;
  return ownerId === responderId;
}

function decisionImpact(input: OverviewInput, wait: DerivedWait): string | null {
  if (input.decisions.coverage === 'unread') return null;
  const record = input.decisions.items.find((item) => validId(item?.id) != null && validId(item.id) === wait.sourceId)
    ?? input.decisions.items.find((item) => validId(item?.taskId) != null && validId(item.taskId) === wait.task.id);
  const impact = record?.impact;
  return typeof impact === 'string' && impact.trim() !== '' ? impact : null;
}

function wholeDays(sinceUnixMs: number | null | undefined, nowUnixMs: number): number | null {
  if (typeof sinceUnixMs !== 'number' || !Number.isFinite(sinceUnixMs) || sinceUnixMs < 0 || sinceUnixMs > nowUnixMs) return null;
  return Math.floor((nowUnixMs - sinceUnixMs) / 86_400_000);
}

export function buildInboxModel({
  models,
  projects,
  failedReads = 0,
}: {
  models: readonly TeamOverviewModelResponse[];
  projects: readonly InboxProjectRef[];
  failedReads?: number;
}): InboxModel {
  const usable = inboxUsableModels(models);
  const projectTitle = new Map(projects.map((project) => [project.key, project.title]));
  const perProject = usable.map((model) => projectEntries(model, projectTitle.get(model.project) ?? null));

  const viewerPersonId = validId(usable.find((model) => validId(model.input.options.views?.me?.personId) != null)
    ?.input.options.views?.me?.personId);
  const viewerMembers = usable.flatMap((model) => (model.input.members.coverage === 'unread' ? [] : model.input.members.items));

  const byKey = new Map<string, InboxEntry>();
  for (const { entries } of perProject) {
    for (const entry of entries) {
      if (!byKey.has(entry.key)) byKey.set(entry.key, entry);
    }
  }
  const allEntries = [...byKey.values()].sort(compareEntries);
  const entries = viewerPersonId == null ? [] : allEntries.filter((entry) => entry.mine);

  const basisKnown = failedReads === 0
    && (perProject.length === 0 || perProject.some((project) => !project.pendingPlanesAllUnread));
  const count = viewerPersonId == null || !basisKnown ? null : entries.length;
  const partial = viewerPersonId != null && basisKnown
    && perProject.some((project) => project.pendingPlanesIncomplete);

  return {
    viewer: viewerPersonId == null ? null : personNamed(viewerMembers, viewerPersonId),
    entries,
    allEntries,
    count,
    partial,
    failedReads,
    viewerUnknown: viewerPersonId == null,
  };
}

/** The shared shell-and-page label: known counts render, unknown stays unread, partial counts carry "+". */
export function inboxPendingLabel(model: InboxModel): string | null {
  if (model.count == null) return null;
  if (model.count === 0) return '0';
  return `${model.count}${model.partial ? '+' : ''}`;
}

/** Resolves the linked entry, or falls back to the first queue row; bogus values never throw. */
export function resolveInboxSelection(model: InboxModel, requested: string | null): InboxEntry | null {
  if (requested != null) {
    const linked = model.allEntries.find((entry) => entry.key === requested);
    if (linked) return linked;
  }
  return model.entries[0] ?? null;
}

/** Every task link goes through the one task-path encoder. */
export function inboxTaskPath(workId: string): string {
  return taskPath(null, workId);
}

const FRESHNESS_SEVERITY = { measured: 0, stale: 1, not_measured: 2, failed: 3 } as const;

function planeState(value: unknown): keyof typeof FRESHNESS_SEVERITY {
  return (Object.keys(FRESHNESS_SEVERITY) as Array<keyof typeof FRESHNESS_SEVERITY>).includes(value as keyof typeof FRESHNESS_SEVERITY)
    ? value as keyof typeof FRESHNESS_SEVERITY
    : 'not_measured';
}

/** One freshness word for the whole composed read: the worst plane across projects. */
export function aggregateInboxFreshness(models: readonly TeamOverviewModelResponse[]): TeamFreshness | undefined {
  if (models.length === 0) return undefined;
  const planes = ['report_received', 'authoritative', 'page_read'] as const;
  const aggregate = {} as TeamFreshness;
  for (const plane of planes) {
    let worst: keyof typeof FRESHNESS_SEVERITY = 'measured';
    let at: number | null = null;
    for (const model of models) {
      const value = model.freshness?.[plane];
      const status = planeState(value?.status);
      if (FRESHNESS_SEVERITY[status] > FRESHNESS_SEVERITY[worst]) worst = status;
      const time = value?.at_unix_ms;
      if ((status === 'measured' || status === 'stale') && typeof time === 'number' && Number.isFinite(time)) {
        at = at == null ? time : Math.max(at, time);
      }
    }
    const plane_value: PlaneFreshness = { status: worst };
    if (at != null) plane_value.at_unix_ms = at;
    aggregate[plane] = plane_value;
  }
  return aggregate;
}
