import { deriveOverviewInsights } from './insights';
import {
  OVERVIEW_PARTS,
  WAIT_KINDS,
  type CountDetails,
  type CoverageIssue,
  type DerivedTask,
  type DerivedTaskStatus,
  type DerivedWait,
  type OverviewBaseCoverage,
  type OverviewCriterionEvidence,
  type OverviewDecision,
  type OverviewDependency,
  type OverviewDerivation,
  type OverviewInput,
  type OverviewPart,
  type OverviewReviewRound,
  type OverviewTask,
  type PersonRef,
  type ReadSlice,
  type WaitKind,
} from './types';

const DAY_MS = 86_400_000;

type Indexed<T> = { value: T; index: number };
type TriState = boolean | null;
type StatusDerivation = {
  status: DerivedTaskStatus;
  unknownReasons: string[];
  verification: DerivedTask['verification'];
};

function id(value: string | null | undefined): string | null {
  return typeof value === 'string' && value.trim().length > 0 ? value : null;
}

function text(value: string | null | undefined): string | null {
  return typeof value === 'string' && value.trim().length > 0 ? value : null;
}

function bool(value: boolean | null | undefined): TriState {
  return typeof value === 'boolean' ? value : null;
}

function time(value: number | null | undefined): number | null {
  return typeof value === 'number' && Number.isFinite(value) && value >= 0 ? value : null;
}

function itemsOf<T>(slice: ReadSlice<T>): readonly T[] {
  // A payload retained by a caller after a failed read is not a successful observation.
  return slice.coverage === 'unread' ? [] : slice.items;
}

function knownCount<T>(items: T[]): CountDetails<T> {
  return { count: items.length, items, unknownItems: [] };
}

function unknownCount<T>(items: T[], unknownItems: CoverageIssue[]): CountDetails<T> {
  return { count: null, items, unknownItems };
}

function issue(part: OverviewPart, reason: string, index?: number, taskId?: string | null): CoverageIssue {
  return { part, reason, ...(index === undefined ? {} : { index }), ...(taskId === undefined ? {} : { taskId }) };
}

function uniqueIssues(values: readonly CoverageIssue[]): CoverageIssue[] {
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

function elapsedDays(startedAtUnixMs: number | null, nowUnixMs: number): number | null {
  if (startedAtUnixMs == null || startedAtUnixMs > nowUnixMs) return null;
  return Math.floor((nowUnixMs - startedAtUnixMs) / DAY_MS);
}

function criterionSupport(task: OverviewTask): TriState {
  if (!Array.isArray(task.acceptance)) return null;
  if (task.acceptance.length === 0) return false;
  for (const criterion of task.acceptance) {
    if (!Array.isArray(criterion.evidence)) return null;
    const evidence = criterion.evidence as readonly OverviewCriterionEvidence[];
    if (evidence.some((entry) => entry.passed === true && entry.current === true)) continue;
    if (evidence.some((entry) => bool(entry.passed) == null || bool(entry.current) == null)) return null;
    return false;
  }
  return true;
}

function differentIdentities(round: OverviewReviewRound): TriState {
  const authorPerson = id(round.authorPersonId);
  const reviewerPerson = id(round.reviewerPersonId);
  if (authorPerson != null && reviewerPerson != null) return authorPerson !== reviewerPerson;
  const authorActor = id(round.authorActorId);
  const reviewerActor = id(round.reviewerActorId);
  if (authorActor != null && reviewerActor != null) return authorActor !== reviewerActor;
  return null;
}

function compareNullableNumbersDescending(a: number | null, b: number | null): number {
  if (a == null && b == null) return 0;
  if (a == null) return 1;
  if (b == null) return -1;
  return b - a;
}

/** Derive the status/waiting slice of the panorama without reading a clock, network, or React state. */
export function deriveOverview(input: OverviewInput): OverviewDerivation {
  const { nowUnixMs, stalledAfterDays } = input.options;
  if (!Number.isFinite(nowUnixMs) || nowUnixMs < 0) {
    throw new RangeError('A finite, non-negative read time is required');
  }
  if (!Number.isFinite(stalledAfterDays) || stalledAfterDays < 0) {
    throw new RangeError('A finite, non-negative stalled threshold is required');
  }
  const stalledAfterMs = stalledAfterDays * DAY_MS;
  if (!Number.isFinite(stalledAfterMs)) throw new RangeError('The stalled threshold is too large');

  const slices = {
    tasks: input.tasks,
    dependencies: input.dependencies,
    decisions: input.decisions,
    reviewRounds: input.reviewRounds,
    handoffs: input.handoffs,
    approvals: input.approvals,
    members: input.members,
    activity: input.activity,
  };
  const partIssues: Record<OverviewPart, CoverageIssue[]> = {
    tasks: [], dependencies: [], decisions: [], reviewRounds: [], handoffs: [], approvals: [], members: [], activity: [],
  };
  for (const part of OVERVIEW_PARTS) {
    const slice = slices[part];
    if (slice.coverage !== 'complete') partIssues[part].push(issue(part, slice.reason));
  }

  const memberById = new Map<string, PersonRef>();
  for (const [index, member] of itemsOf(input.members).entries()) {
    const personId = id(member.personId);
    if (personId == null) {
      partIssues.members.push(issue('members', 'member-id-unknown', index));
      continue;
    }
    if (memberById.has(personId)) {
      partIssues.members.push(issue('members', 'duplicate-member-id', index));
      continue;
    }
    memberById.set(personId, { id: personId, displayName: text(member.displayName) });
  }
  const person = (personId: string | null | undefined): PersonRef | null => {
    const valid = id(personId);
    if (valid == null) return null;
    return memberById.get(valid) ?? { id: valid, displayName: null };
  };

  const taskInputs = itemsOf(input.tasks);
  const taskIds = taskInputs.map((task) => id(task.id));
  const taskIdSet = new Set(taskIds.filter((value): value is string => value != null));
  const taskStructuralIssues: CoverageIssue[] = [...partIssues.tasks];
  const taskIndexById = new Map<string, number>();
  for (const [index, taskId] of taskIds.entries()) {
    if (taskId == null) {
      const value = issue('tasks', 'task-id-unknown', index);
      taskStructuralIssues.push(value);
      partIssues.tasks.push(value);
    } else if (taskIndexById.has(taskId)) {
      const value = issue('tasks', 'duplicate-task-id', index, taskId);
      taskStructuralIssues.push(value);
      partIssues.tasks.push(value);
    } else {
      taskIndexById.set(taskId, index);
    }
  }

  const openDecisionsByTask = new Map<string, Indexed<OverviewDecision>[]>();
  let decisionsHaveUnknownCurrent = false;
  for (const [index, decision] of itemsOf(input.decisions).entries()) {
    const taskId = id(decision.taskId);
    if (taskId == null || !taskIdSet.has(taskId)) {
      partIssues.decisions.push(issue('decisions', 'decision-task-unknown', index, taskId));
      continue;
    }
    if (!['open', 'replied', 'cancelled'].includes(decision.state ?? '')) {
      partIssues.decisions.push(issue('decisions', 'decision-state-unknown', index, taskId));
      continue;
    }
    if (decision.state === 'open' && decision.current == null) {
      decisionsHaveUnknownCurrent = true;
      partIssues.decisions.push(issue('decisions', 'decision-current-unknown', index, taskId));
      continue;
    }
    if (decision.state !== 'open' || decision.current === false) continue;
    const rows = openDecisionsByTask.get(taskId) ?? [];
    rows.push({ value: decision, index });
    openDecisionsByTask.set(taskId, rows);
  }

  const roundsByTask = new Map<string, Indexed<OverviewReviewRound>[]>();
  for (const [index, round] of itemsOf(input.reviewRounds).entries()) {
    const taskId = id(round.taskId);
    if (taskId == null || !taskIdSet.has(taskId)) {
      partIssues.reviewRounds.push(issue('reviewRounds', 'review-task-unknown', index, taskId));
      continue;
    }
    if (!['open', 'approved', 'rejected', 'invalidated'].includes(round.state ?? '')) {
      partIssues.reviewRounds.push(issue('reviewRounds', 'review-state-unknown', index, taskId));
      continue;
    }
    if (round.current == null) {
      partIssues.reviewRounds.push(issue('reviewRounds', 'review-current-unknown', index, taskId));
    }
    if (round.current === false) continue;
    const rows = roundsByTask.get(taskId) ?? [];
    rows.push({ value: round, index });
    roundsByTask.set(taskId, rows);
  }
  const latestRoundByTask = new Map<string, Indexed<OverviewReviewRound>>();
  for (const [taskId, rows] of roundsByTask) {
    if (rows.length > 1 && rows.some((row) => !Number.isSafeInteger(row.value.roundIndex))) {
      partIssues.reviewRounds.push(issue('reviewRounds', 'latest-review-unknown', undefined, taskId));
      continue;
    }
    let latest = rows[0];
    for (const row of rows.slice(1)) {
      const previousIndex = row.value.roundIndex ?? -1;
      const latestIndex = latest.value.roundIndex ?? -1;
      if (previousIndex > latestIndex || previousIndex === latestIndex && row.index > latest.index) latest = row;
    }
    if (latest) latestRoundByTask.set(taskId, latest);
  }

  const dependencyGraphIssues: CoverageIssue[] = [...partIssues.dependencies];
  const dependenciesByConsumer = new Map<string, Indexed<OverviewDependency>[]>();
  const dependencyKeys = new Set<string>();
  for (const [index, dependency] of itemsOf(input.dependencies).entries()) {
    const consumerTaskId = id(dependency.consumerTaskId);
    const providerTaskId = id(dependency.providerTaskId);
    const key = `${consumerTaskId ?? ''}\u0000${providerTaskId ?? ''}`;
    if (dependencyKeys.has(key)) {
      const value = issue('dependencies', 'duplicate-dependency', index, consumerTaskId);
      dependencyGraphIssues.push(value);
      partIssues.dependencies.push(value);
      continue;
    }
    dependencyKeys.add(key);
    if (consumerTaskId == null || providerTaskId == null
      || !taskIdSet.has(consumerTaskId) || !taskIdSet.has(providerTaskId)) {
      const value = issue('dependencies', 'dependency-endpoint-unknown', index, consumerTaskId);
      dependencyGraphIssues.push(value);
      partIssues.dependencies.push(value);
    }
    if (bool(dependency.satisfied) == null) {
      partIssues.dependencies.push(issue('dependencies', 'dependency-satisfaction-unknown', index, consumerTaskId));
    }
    if (consumerTaskId == null || !taskIdSet.has(consumerTaskId)) continue;
    const rows = dependenciesByConsumer.get(consumerTaskId) ?? [];
    rows.push({ value: dependency, index });
    dependenciesByConsumer.set(consumerTaskId, rows);
  }

  const lastActivityByTask = new Map<string, number>();
  for (const [index, activity] of itemsOf(input.activity).entries()) {
    const taskId = id(activity.taskId);
    const at = time(activity.atUnixMs);
    if (taskId == null || !taskIdSet.has(taskId) || at == null || at > nowUnixMs) {
      partIssues.activity.push(issue('activity', 'activity-fact-unknown', index, taskId));
      continue;
    }
    lastActivityByTask.set(taskId, Math.max(lastActivityByTask.get(taskId) ?? -1, at));
  }

  const dependenciesComplete = input.dependencies.coverage === 'complete' && partIssues.dependencies.length === 0;
  const decisionsComplete = input.decisions.coverage === 'complete' && partIssues.decisions.length === 0;
  const reviewsComplete = input.reviewRounds.coverage === 'complete' && partIssues.reviewRounds.length === 0;
  const activityComplete = input.activity.coverage === 'complete' && partIssues.activity.length === 0;

  function dependencyState(taskId: string | null): TriState {
    if (taskId == null) return null;
    const rows = dependenciesByConsumer.get(taskId) ?? [];
    if (rows.some((row) => row.value.satisfied === false)) return true;
    if (!dependenciesComplete || rows.some((row) => bool(row.value.satisfied) == null)) return null;
    return false;
  }

  function decisionState(taskId: string | null): TriState {
    if (taskId == null) return null;
    if ((openDecisionsByTask.get(taskId) ?? []).length > 0) return true;
    return decisionsComplete && !decisionsHaveUnknownCurrent ? false : null;
  }

  function reviewApproval(round: Indexed<OverviewReviewRound> | undefined): TriState {
    if (round == null) return reviewsComplete ? false : null;
    const value = round.value;
    if (value.current !== true || value.state !== 'approved') return value.current == null ? null : false;
    const independent = bool(value.independent);
    const different = differentIdentities(value);
    return independent === true && different === true ? true : independent === false || different === false ? false : null;
  }

  function submissionState(task: OverviewTask, round: Indexed<OverviewReviewRound> | undefined): TriState {
    if (time(task.submittedResultAtUnixMs) != null || round != null) return true;
    if (task.submittedResultAtUnixMs === null) return reviewsComplete ? false : null;
    return null;
  }

  function deriveStatus(task: OverviewTask, taskId: string | null): StatusDerivation {
    const round = latestRoundByTask.get(taskId ?? '');
    const everyCriterionSupported = criterionSupport(task);
    const independentReviewApproved = reviewApproval(round);
    const integrated = bool(task.integrated);
    const verification = { everyCriterionSupported, independentReviewApproved, integrated };
    const result = (status: DerivedTaskStatus, unknownReasons: string[] = []): StatusDerivation => ({
      status, unknownReasons: status === 'unknown' && unknownReasons.length === 0 ? ['required-facts-unknown'] : unknownReasons, verification,
    });

    if (everyCriterionSupported === true && independentReviewApproved === true && integrated === true) {
      return result('verified');
    }
    if (round?.value.current === true && round.value.state === 'rejected') return result('returned');
    if (round?.value.current === true && round.value.state === 'open') return result('review');

    const submitted = submissionState(task, round);
    if (task.report?.completed === true) {
      if (submitted === false) return result('claimed');
      return result('unknown', [submitted == null ? 'submission-unknown' : 'submitted-result-unclassified']);
    }
    if (round?.value.current == null && round != null) return result('unknown', ['review-current-unknown']);
    if (integrated === true || round?.value.current === true && round.value.state === 'approved') {
      return result('unknown', ['verification-incomplete']);
    }

    const external = task.externalWait === null ? false : typeof task.externalWait === 'object' ? true : null;
    if (external === true) return result('waiting');
    const upstream = dependencyState(taskId);
    const decision = decisionState(taskId);
    const started = bool(task.started);
    if (external == null) return result('unknown', ['external-wait-unknown']);
    if (started == null) return result('unknown', ['started-unknown']);

    if (started === false) {
      if (decision === true) return result('waiting');
      if (upstream === true) return result('planned');
      if (decision == null) return result('unknown', ['decisions-incomplete']);
      if (upstream == null) return result('unknown', ['dependencies-incomplete']);
      if (!Array.isArray(task.acceptance)) return result('unknown', ['acceptance-unknown']);
      return task.acceptance.length === 0 ? result('planned') : result('ready');
    }

    if (upstream === true) return result('waiting');
    if (upstream == null) return result('unknown', ['dependencies-incomplete']);
    if (decision == null) return result('unknown', ['decisions-incomplete']);
    const active = bool(task.agentActive);
    const reportAt = time(task.report?.reportedAtUnixMs);
    const activityAt = taskId == null ? null : lastActivityByTask.get(taskId) ?? null;
    const latestActivityAt = reportAt == null ? activityAt : activityAt == null ? reportAt : Math.max(reportAt, activityAt);
    const ageMs = latestActivityAt == null || latestActivityAt > nowUnixMs ? null : nowUnixMs - latestActivityAt;

    if (active === true || decision === true) {
      if (ageMs == null) return result('unknown', ['activity-time-unknown']);
      if (ageMs >= stalledAfterMs) {
        return activityComplete ? result('stalled') : result('unknown', ['activity-incomplete']);
      }
      return result(active === true ? 'running' : 'waiting');
    }
    if (active === false && ageMs != null && ageMs >= stalledAfterMs && activityComplete) return result('stalled');
    return result('unknown', [active == null ? 'agent-activity-unknown' : 'inactive-without-recorded-wait']);
  }

  const tasks: DerivedTask[] = taskInputs.map((task, index) => {
    const taskId = taskIds[index];
    const derived = deriveStatus(task, taskId);
    return {
      id: taskId,
      title: text(task.title),
      owner: person(task.ownerPersonId),
      agentName: text(task.agentName),
      projectId: id(task.projectId),
      groupId: id(task.groupId),
      goalIds: Array.isArray(task.goalIds) ? [...task.goalIds] : null,
      commitments: Array.isArray(task.commitments) ? task.commitments.map((commitment) => ({ ...commitment })) : null,
      verifiedAtUnixMs: time(task.verifiedAtUnixMs),
      status: derived.status,
      unknownReasons: derived.unknownReasons,
      verification: derived.verification,
    };
  });
  const taskById = new Map<string, DerivedTask>();
  for (const [index, task] of tasks.entries()) {
    if (task.id != null && taskIndexById.get(task.id) === index) taskById.set(task.id, task);
  }

  const unknownStatusIssues: CoverageIssue[] = [];
  for (const [index, task] of tasks.entries()) {
    for (const reason of task.unknownReasons) {
      const value = issue('tasks', `task-status-${reason}`, index, task.id);
      unknownStatusIssues.push(value);
      partIssues.tasks.push(value);
    }
  }

  const dependents: number[][] = tasks.map(() => []);
  for (const rows of dependenciesByConsumer.values()) {
    for (const row of rows) {
      const consumer = id(row.value.consumerTaskId);
      const provider = id(row.value.providerTaskId);
      const consumerIndex = consumer == null ? undefined : taskIndexById.get(consumer);
      const providerIndex = provider == null ? undefined : taskIndexById.get(provider);
      if (consumerIndex != null && providerIndex != null) dependents[providerIndex].push(consumerIndex);
    }
  }
  const graphComplete = input.dependencies.coverage === 'complete'
    && dependencyGraphIssues.length === 0 && taskStructuralIssues.length === 0;
  const graphUnknownItems = uniqueIssues([...dependencyGraphIssues, ...taskStructuralIssues]);
  const behindCache = new Map<string, CountDetails<DerivedTask>>();
  const visited = new Int32Array(tasks.length);
  let visitToken = 0;

  function behindOf(task: DerivedTask): CountDetails<DerivedTask> {
    if (task.id == null) return unknownCount([], graphUnknownItems.length > 0 ? graphUnknownItems : [issue('tasks', 'task-id-unknown')]);
    const cached = behindCache.get(task.id);
    if (cached) return cached;
    const root = taskIndexById.get(task.id);
    if (root == null) return unknownCount([], [issue('tasks', 'task-id-unknown', undefined, task.id)]);
    visitToken += 1;
    const stack = [root];
    visited[root] = visitToken;
    while (stack.length > 0) {
      const current = stack.pop()!;
      for (const next of dependents[current]) {
        if (visited[next] === visitToken) continue;
        visited[next] = visitToken;
        stack.push(next);
      }
    }
    const items: DerivedTask[] = [];
    for (let index = 0; index < tasks.length; index += 1) {
      if (visited[index] === visitToken) items.push(tasks[index]);
    }
    const result = graphComplete ? knownCount(items) : unknownCount(items, graphUnknownItems);
    behindCache.set(task.id, result);
    return result;
  }

  const waits: DerivedWait[] = [];
  const waiterIssues: CoverageIssue[] = [];
  const externalUnknownIssues: CoverageIssue[] = [];
  const dependencyWaitIssues: CoverageIssue[] = [];
  for (const [index, task] of taskInputs.entries()) {
    if (task.externalWait === undefined) {
      externalUnknownIssues.push(issue('tasks', 'external-wait-unknown', index, taskIds[index]));
    }
    if (task.externalWait != null && taskIds[index] == null) {
      externalUnknownIssues.push(issue('tasks', 'external-wait-task-unknown', index));
    }
  }

  function addWait(
    task: DerivedTask,
    kind: WaitKind,
    sourceId: string | null,
    waitingOn: PersonRef[],
    waitingOnUnknown: boolean,
    what: string | null,
    providerTask: DerivedTask | null,
    startedAtUnixMs: number | null,
    externalParty: string | null,
  ) {
    if (waitingOnUnknown) waiterIssues.push(issue(kind === 'review' ? 'reviewRounds' : kind === 'decision' ? 'decisions' : 'tasks', 'waiter-unknown', undefined, task.id));
    waits.push({
      kind, sourceId, task, waitingOn, waitingOnUnknown, what, providerTask,
      externalParty, startedAtUnixMs, durationDays: elapsedDays(startedAtUnixMs, nowUnixMs), behind: behindOf(task),
    });
  }

  for (const [index, task] of tasks.entries()) {
    const taskId = task.id;
    const source = taskInputs[index];
    if (taskId == null) continue;
    for (const decision of openDecisionsByTask.get(taskId) ?? []) {
      const responders = decision.value.allowedResponderIds;
      const waitingOn = Array.isArray(responders) ? responders.map(person).filter((value): value is PersonRef => value != null) : [];
      addWait(task, 'decision', id(decision.value.id), waitingOn,
        !Array.isArray(responders) || waitingOn.length === 0 || waitingOn.length !== responders.length,
        text(decision.value.question), null, time(decision.value.createdAtUnixMs), null);
    }
    for (const dependency of dependenciesByConsumer.get(taskId) ?? []) {
      if (dependency.value.satisfied !== false) continue;
      if (source.started !== true) {
        if (source.started == null) dependencyWaitIssues.push(issue('dependencies', 'dependency-wait-start-unknown', dependency.index, taskId));
        continue;
      }
      const provider = person(taskById.get(id(dependency.value.providerTaskId) ?? '')?.owner?.id);
      const providerTask = taskById.get(id(dependency.value.providerTaskId) ?? '') ?? null;
      addWait(task, 'dependency', id(dependency.value.id), provider ? [provider] : [], provider == null,
        text(dependency.value.what), providerTask, time(dependency.value.sinceUnixMs), null);
    }
    const round = latestRoundByTask.get(taskId);
    if (round?.value.current === true && round.value.state === 'open') {
      const reviewer = person(round.value.reviewerPersonId);
      addWait(task, 'review', id(round.value.id), reviewer ? [reviewer] : [], reviewer == null,
        null, null, time(round.value.openedAtUnixMs), null);
    }
    if (source.externalWait != null) {
      const party = text(source.externalWait.party);
      addWait(task, 'external', null, [], party == null, text(source.externalWait.what), null,
        time(source.externalWait.sinceUnixMs), party);
    }
  }

  waits.sort((left, right) => compareNullableNumbersDescending(left.behind.count, right.behind.count)
    || compareNullableNumbersDescending(left.durationDays, right.durationDays));

  const taskListIncomplete = taskStructuralIssues.length > 0;
  const byStatusItems: Record<DerivedTaskStatus, DerivedTask[]> = {
    verified: [], claimed: [], returned: [], review: [], running: [], stalled: [],
    waiting: [], ready: [], planned: [], unknown: [],
  };
  for (const task of tasks) byStatusItems[task.status].push(task);
  const byStatusEntries = Object.entries(byStatusItems).map(([status, items]) => {
    const typedStatus = status as DerivedTaskStatus;
    if (taskListIncomplete) return [status, unknownCount(items, taskStructuralIssues)];
    if (typedStatus !== 'unknown' && byStatusItems.unknown.length > 0) return [status, unknownCount(items, unknownStatusIssues)];
    return [status, knownCount(items)];
  });
  const byStatus = Object.fromEntries(byStatusEntries) as Record<DerivedTaskStatus, CountDetails<DerivedTask>>;

  const tasksReadIssues = taskStructuralIssues;
  const kindIssues: Record<WaitKind, CoverageIssue[]> = {
    decision: uniqueIssues([...tasksReadIssues, ...partIssues.decisions]),
    dependency: uniqueIssues([...tasksReadIssues, ...partIssues.dependencies, ...dependencyWaitIssues]),
    review: uniqueIssues([...tasksReadIssues, ...partIssues.reviewRounds]),
    external: uniqueIssues([...tasksReadIssues, ...externalUnknownIssues]),
  };
  const byKind = Object.fromEntries(WAIT_KINDS.map((kind) => {
    const items = waits.filter((wait) => wait.kind === kind);
    return [kind, kindIssues[kind].length > 0 ? unknownCount(items, kindIssues[kind]) : knownCount(items)];
  })) as Record<WaitKind, CountDetails<DerivedWait>>;
  const allWaitIssues = uniqueIssues(WAIT_KINDS.flatMap((kind) => kindIssues[kind]));

  for (const value of [...externalUnknownIssues, ...waiterIssues]) partIssues[value.part].push(value);
  const parts = Object.fromEntries(OVERVIEW_PARTS.map((part) => [part, {
    state: slices[part].coverage,
    reason: slices[part].coverage === 'complete' ? null : slices[part].reason,
    issues: uniqueIssues(partIssues[part]),
  }])) as OverviewBaseCoverage['parts'];
  const statusIssues = uniqueIssues([
    ...parts.tasks.issues, ...parts.dependencies.issues, ...parts.decisions.issues,
    ...parts.reviewRounds.issues, ...parts.activity.issues,
  ]).filter((value) => value.reason !== 'waiter-unknown');
  const waitCoverageIssues = uniqueIssues([
    ...parts.tasks.issues, ...parts.dependencies.issues, ...parts.decisions.issues,
    ...parts.reviewRounds.issues, ...parts.members.issues,
  ]);
  const coverage: OverviewBaseCoverage = {
    complete: OVERVIEW_PARTS.every((part) => parts[part].state === 'complete' && parts[part].issues.length === 0),
    parts,
    sections: {
      status: { complete: statusIssues.length === 0, issues: statusIssues },
      waits: { complete: waitCoverageIssues.length === 0, issues: waitCoverageIssues },
    },
  };

  const base = {
    tasks,
    waits,
    taskCounts: {
      all: taskListIncomplete ? unknownCount(tasks, taskStructuralIssues) : knownCount(tasks),
      byStatus,
    },
    waitCounts: {
      all: allWaitIssues.length > 0 ? unknownCount(waits, allWaitIssues) : knownCount(waits),
      byKind,
    },
    coverage,
  };
  return { ...base, ...deriveOverviewInsights(input, base) };
}
