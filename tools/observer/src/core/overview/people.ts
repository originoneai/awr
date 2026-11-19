import type {
  CoverageIssue,
  OverviewInput,
  OverviewStatusWaitingDerivation,
  PersonRef,
  PersonSummary,
} from './types';
import {
  details,
  isPrototypeInProgress,
  makeIssue,
  readItems,
  samePerson,
  uniqueIssues,
  validId,
  type PendingIndex,
} from './shared';

export function derivePeople(input: OverviewInput, base: OverviewStatusWaitingDerivation, pending: PendingIndex) {
  const people: PersonRef[] = [];
  const personIds = new Set<string>();
  const add = (person: PersonRef | null) => {
    if (person == null || validId(person.id) == null || personIds.has(person.id)) return;
    personIds.add(person.id);
    people.push(person);
  };
  for (const member of readItems(input.members)) {
    const personId = validId(member.personId);
    if (personId != null) add({ id: personId, displayName: member.displayName ?? null });
  }
  base.tasks.forEach((task) => add(task.owner));
  base.waits.forEach((wait) => wait.waitingOn.forEach(add));

  const coverageIssues: CoverageIssue[] = [...base.coverage.parts.members.issues, ...pending.coverageIssues];
  const summaries = people.map((person): PersonSummary => {
    const pendingItems = pending.byPerson.get(person.id) ?? [];
    const inProgressItems = base.tasks.filter((task) => samePerson(task.owner?.id, person.id) && isPrototypeInProgress(task));
    const waitingItems = base.waits.filter((wait) => samePerson(wait.task.owner?.id, person.id));
    const owedItems = base.waits.filter((wait) => wait.kind === 'dependency'
      && samePerson(wait.providerTask?.owner?.id, person.id));
    const ownUnknownStatus = base.tasks.some((task) => samePerson(task.owner?.id, person.id) && task.status === 'unknown');
    const inProgressIssues = [...base.taskCounts.all.unknownItems];
    if (ownUnknownStatus) inProgressIssues.push(makeIssue('tasks', 'person-in-progress-unknown', person.id));
    return {
      person,
      pending: details(pendingItems, pending.countIssues),
      inProgress: details(inProgressItems, inProgressIssues),
      waitingOn: details(waitingItems, base.waitCounts.all.unknownItems),
      owedBy: details(owedItems, base.waitCounts.byKind.dependency.unknownItems),
    };
  });
  for (const summary of summaries) {
    coverageIssues.push(...summary.pending.unknownItems, ...summary.inProgress.unknownItems,
      ...summary.waitingOn.unknownItems, ...summary.owedBy.unknownItems);
  }
  const directoryIssues = input.members.coverage === 'complete'
    ? base.coverage.parts.members.issues.filter((value) => value.reason.includes('member-id') || value.reason.includes('duplicate'))
    : [makeIssue('members', input.members.reason)];
  return {
    people: summaries,
    peopleCounts: { all: details(summaries, directoryIssues) },
    coverageIssues: uniqueIssues(coverageIssues),
  };
}
