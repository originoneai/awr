import { describe, expect, it } from 'vitest';
import { deriveOverview } from '../index';
import { DAY, NOW, baseTask, complete, decision, dependency, makeInput, reviewRound, startedTask } from './factories';

function peopleInput() {
  return makeInput({
    tasks: complete([
      startedTask('own', { ownerPersonId: 'person-a', agentName: 'Agent One' }),
      startedTask('provider', {
        ownerPersonId: 'person-b', agentName: 'Agent Two', submittedResultAtUnixMs: NOW - DAY,
      }),
      startedTask('consumer', { ownerPersonId: 'person-c', agentName: 'Agent Three' }),
      baseTask('outside', {
        ownerPersonId: 'person-a', agentName: null,
        externalWait: { party: 'Outside Example', sinceUnixMs: NOW - 2 * DAY },
      }),
      startedTask('unknown-provider', { ownerPersonId: null }),
      startedTask('unknown-consumer', { ownerPersonId: 'person-a', agentName: 'Agent Four' }),
    ]),
    dependencies: complete([
      dependency('consumer', 'provider'),
      dependency('unknown-consumer', 'unknown-provider'),
    ]),
    decisions: complete([decision('own', { allowedResponderIds: ['person-a'] })]),
    reviewRounds: complete([reviewRound('provider', 'open', { reviewerPersonId: 'person-c' })]),
    handoffs: complete([{
      id: 'pending-handoff', taskId: 'own', kind: 'responsibility', status: 'proposed', pending: true,
      fromPersonId: 'person-b', toPersonId: 'person-a', updatedAtUnixMs: NOW - DAY,
    }]),
  });
}

describe('per-person summaries', () => {
  it('derives pending, in-progress, waiting and owed-result details for each explicit person', () => {
    const result = deriveOverview(peopleInput());
    expect(result.people.map((summary) => summary.person.id)).toEqual(['person-a', 'person-b', 'person-c']);
    const [a, b, c] = result.people;
    expect(a.pending.count).toBe(2);
    expect(a.pending.items.map((item) => item.kind)).toEqual(['wait', 'handoff']);
    expect(a.inProgress.count).toBe(2);
    expect(a.inProgress.items.map((task) => task.id)).toEqual(['own', 'unknown-consumer']);
    expect(a.waitingOn.count).toBe(3);
    expect(a.waitingOn.items.map((wait) => wait.kind).sort()).toEqual(['decision', 'dependency', 'external']);
    expect(a.owedBy.count).toBe(0);

    expect(b.pending.count).toBe(0);
    expect(b.inProgress.items.map((task) => task.id)).toEqual(['provider']);
    expect(b.waitingOn.items.map((wait) => wait.kind)).toEqual(['review']);
    expect(b.owedBy.items.map((wait) => wait.task.id)).toEqual(['consumer']);

    expect(c.pending.count).toBe(1);
    expect(c.pending.items[0].wait?.kind).toBe('review');
    expect(c.inProgress.items.map((task) => task.id)).toEqual(['consumer']);
    expect(c.waitingOn.items.map((wait) => wait.kind)).toEqual(['dependency']);
    expect(result.peopleCounts.all.count).toBe(3);
    for (const summary of result.people) {
      for (const value of [summary.pending, summary.inProgress, summary.waitingOn, summary.owedBy]) {
        if (value.count != null) expect(value.count).toBe(value.items.length);
      }
    }
    if (result.peopleCounts.all.count != null) expect(result.peopleCounts.all.count).toBe(result.peopleCounts.all.items.length);
  });

  it('never treats a missing provider identity as another person or as the viewer', () => {
    const result = deriveOverview(peopleInput());
    for (const summary of result.people) {
      expect(summary.owedBy.items.some((wait) => wait.task.id === 'unknown-consumer')).toBe(false);
    }
    const withoutProvider = deriveOverview(makeInput({
      tasks: complete([startedTask('consumer', { ownerPersonId: 'person-a' }), startedTask('provider', { ownerPersonId: null })]),
      dependencies: complete([dependency('consumer', 'provider')]),
    }));
    const owner = withoutProvider.people.find((summary) => summary.person.id === 'person-a')!;
    expect(owner.waitingOn.count).toBe(1);
    expect(owner.owedBy.count).toBe(0);
  });

  it('marks pending unknown when a handoff read is incomplete instead of dropping its queue to zero', () => {
    const input = peopleInput();
    const result = deriveOverview(makeInput({
      ...input,
      handoffs: { coverage: 'unread', reason: 'read-failed' },
    }));
    const owner = result.people.find((summary) => summary.person.id === 'person-a')!;
    expect(owner.pending.count).toBeNull();
    expect(owner.pending.items).toHaveLength(1);
    expect(result.coverage.sections.people.complete).toBe(false);
  });
});
