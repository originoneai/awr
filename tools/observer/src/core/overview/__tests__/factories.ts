import type {
  OverviewDecision,
  OverviewDependency,
  OverviewInput,
  OverviewReviewRound,
  OverviewTask,
  ReadSlice,
} from '../index';

export const DAY = 86_400_000;
export const NOW = 1_800_000_000_000;

export function complete<T>(items: readonly T[]): ReadSlice<T> {
  return { coverage: 'complete', items };
}

export function makeInput(overrides: Partial<OverviewInput> = {}): OverviewInput {
  return {
    tasks: complete([]),
    dependencies: complete([]),
    decisions: complete([]),
    reviewRounds: complete([]),
    handoffs: complete([]),
    members: complete([
      { personId: 'person-a', displayName: 'Owner One' },
      { personId: 'person-b', displayName: 'Reviewer Two' },
      { personId: 'person-c', displayName: 'Owner Three' },
    ]),
    activity: complete([]),
    options: { nowUnixMs: NOW, stalledAfterDays: 3 },
    ...overrides,
  };
}

export function baseTask(taskId: string, overrides: Partial<OverviewTask> = {}): OverviewTask {
  return {
    id: taskId,
    title: `Synthetic ${taskId}`,
    ownerPersonId: 'person-a',
    agentName: 'Example Agent',
    acceptance: [{ evidence: [{ passed: false, current: true }] }],
    report: { completed: false, reportedAtUnixMs: NOW - DAY / 24 },
    submittedResultAtUnixMs: null,
    integrated: false,
    started: false,
    agentActive: false,
    externalWait: null,
    ...overrides,
  };
}

export function startedTask(taskId: string, overrides: Partial<OverviewTask> = {}): OverviewTask {
  return baseTask(taskId, { started: true, agentActive: true, ...overrides });
}

export function supportedAcceptance(criteria = 2) {
  return Array.from({ length: criteria }, () => ({ evidence: [{ passed: true, current: true }] }));
}

export function reviewRound(
  taskId: string,
  state: OverviewReviewRound['state'],
  overrides: Partial<OverviewReviewRound> = {},
): OverviewReviewRound {
  return {
    id: `round-${taskId}-${state}`,
    taskId,
    roundIndex: 1,
    state,
    current: true,
    independent: true,
    authorPersonId: 'person-a',
    reviewerPersonId: 'person-b',
    authorActorId: 'actor-a',
    reviewerActorId: 'actor-b',
    openedAtUnixMs: NOW - 2 * DAY,
    ...overrides,
  };
}

export function decision(taskId: string, overrides: Partial<OverviewDecision> = {}): OverviewDecision {
  return {
    id: `decision-${taskId}`,
    taskId,
    state: 'open',
    current: true,
    question: 'Which synthetic option should be used?',
    impact: 'The next synthetic task can continue.',
    allowedResponderIds: ['person-a'],
    createdAtUnixMs: NOW - 2 * DAY,
    ...overrides,
  };
}

export function dependency(
  consumerTaskId: string,
  providerTaskId: string,
  overrides: Partial<OverviewDependency> = {},
): OverviewDependency {
  return {
    id: `edge-${consumerTaskId}-${providerTaskId}`,
    consumerTaskId,
    providerTaskId,
    satisfied: false,
    what: 'The synthetic provider result',
    sinceUnixMs: NOW - 2 * DAY,
    ...overrides,
  };
}
