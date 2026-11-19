import type {
  CountDetails,
  CoverageIssue,
  DerivedTask,
  DerivedWait,
  OverviewInput,
  OverviewPart,
  OverviewStatusWaitingDerivation,
  PersonPendingItem,
  PersonRef,
  ReadSlice,
} from './types';

export function readItems<T>(slice: ReadSlice<T>): readonly T[] {
  return slice.coverage === 'unread' ? [] : slice.items;
}

export function validId(value: string | null | undefined): string | null {
  return typeof value === 'string' && value.trim().length > 0 ? value : null;
}

/** Missing identities are neither the same person nor somebody else. */
export function samePerson(left: string | null | undefined, right: string | null | undefined): boolean {
  return left != null && right != null && left === right;
}

export function makeIssue(part: OverviewPart, reason: string, taskId?: string | null): CoverageIssue {
  return { part, reason, ...(taskId === undefined ? {} : { taskId }) };
}

export function knownDetails<T>(items: T[]): CountDetails<T> {
  return { count: items.length, items, unknownItems: [] };
}

export function unknownDetails<T>(items: T[], unknownItems: CoverageIssue[]): CountDetails<T> {
  return { count: null, items, unknownItems };
}

export function details<T>(items: T[], issues: CoverageIssue[]): CountDetails<T> {
  return issues.length > 0 ? unknownDetails(items, uniqueIssues(issues)) : knownDetails(items);
}

export function uniqueIssues(values: readonly CoverageIssue[]): CoverageIssue[] {
  const seen = new Set<string>();
  const result: CoverageIssue[] = [];
  for (const value of values) {
    const key = `${value.part}\u0000${value.reason}\u0000${value.index ?? ''}\u0000${value.taskId ?? ''}`;
    if (!seen.has(key)) {
      seen.add(key);
      result.push(value);
    }
  }
  return result;
}

export function isPrototypeInProgress(task: DerivedTask): boolean {
  return ['review', 'stalled', 'returned', 'claimed', 'running'].includes(task.status)
    || task.status === 'waiting' && task.agentName != null;
}

export interface PendingIndex {
  byPerson: Map<string, PersonPendingItem[]>;
  countIssues: CoverageIssue[];
  coverageIssues: CoverageIssue[];
}

export function buildPendingIndex(input: OverviewInput, base: OverviewStatusWaitingDerivation): PendingIndex {
  const byPerson = new Map<string, PersonPendingItem[]>();
  const countIssues: CoverageIssue[] = [];
  const coverageIssues: CoverageIssue[] = [];
  const taskById = new Map(base.tasks.map((task) => [task.id, task]));
  const push = (personId: string, item: PersonPendingItem) => {
    const items = byPerson.get(personId) ?? [];
    items.push(item);
    byPerson.set(personId, items);
  };

  for (const wait of base.waits) {
    if (wait.kind === 'dependency') continue;
    for (const person of wait.waitingOn) {
      if (validId(person.id) != null) {
        push(person.id, { kind: 'wait', task: wait.task, wait, handoff: null, approval: null });
      }
    }
  }

  for (const [index, handoff] of readItems(input.handoffs).entries()) {
    if (handoff.pending == null) {
      const value = makeIssue('handoffs', 'handoff-pending-unknown', handoff.taskId);
      countIssues.push(value);
      coverageIssues.push(value);
      continue;
    }
    if (handoff.pending !== true) continue;
    const recipient = validId(handoff.toPersonId);
    if (recipient == null) {
      const value = makeIssue('handoffs', 'handoff-recipient-unknown', handoff.taskId);
      countIssues.push(value);
      coverageIssues.push(value);
      continue;
    }
    const taskId = validId(handoff.taskId);
    const task = taskId == null ? null : taskById.get(taskId) ?? null;
    if (task == null) coverageIssues.push(makeIssue('handoffs', `handoff-task-unknown-${index}`, taskId));
    push(recipient, { kind: 'handoff', task, wait: null, handoff, approval: null });
  }

  for (const approval of readItems(input.approvals)) {
    if (approval.kind !== 'plan-approval' && approval.kind !== 'lesson-confirmation') {
      const value = makeIssue('approvals', 'approval-kind-unknown');
      countIssues.push(value);
      coverageIssues.push(value);
      continue;
    }
    const recipient = validId(approval.recipientPersonId);
    if (recipient == null) {
      const value = makeIssue('approvals', 'approval-recipient-unknown');
      countIssues.push(value);
      coverageIssues.push(value);
      continue;
    }
    push(recipient, { kind: 'approval', task: null, wait: null, handoff: null, approval });
  }

  if (input.handoffs.coverage !== 'complete') {
    const value = makeIssue('handoffs', input.handoffs.reason);
    countIssues.push(value);
    coverageIssues.push(value);
  }
  if (input.approvals.coverage !== 'complete') {
    const value = makeIssue('approvals', input.approvals.reason);
    countIssues.push(value);
    coverageIssues.push(value);
  }
  for (const kind of ['decision', 'review', 'external'] as const) {
    countIssues.push(...base.waitCounts.byKind[kind].unknownItems);
  }
  return {
    byPerson,
    countIssues: uniqueIssues(countIssues),
    coverageIssues: uniqueIssues(coverageIssues),
  };
}

export function personFromId(personId: string, people: readonly PersonRef[]): PersonRef {
  return people.find((person) => person.id === personId) ?? { id: personId, displayName: null };
}

export function filterWaitsToTasks(waits: readonly DerivedWait[], taskIds: ReadonlySet<string>): DerivedWait[] {
  return waits.filter((wait) => wait.task.id != null && taskIds.has(wait.task.id));
}
