import { describe, expect, it } from 'vitest';
import { deriveOverview } from '../index';
import type { CountDetails, OverviewTask } from '../index';
import {
  DAY,
  NOW,
  baseTask,
  complete,
  decision,
  dependency,
  makeInput,
  reviewRound,
  startedTask,
} from './factories';

function expectCountInvariant<T>(value: CountDetails<T>) {
  if (value.count == null) expect(value.unknownItems.length).toBeGreaterThan(0);
  else expect(value.count).toBe(value.items.length);
}

describe('waiting lists and transitive impact', () => {
  it('derives all four wait kinds and includes direct plus transitive dependents', () => {
    const tasks = [
      startedTask('root'),
      startedTask('consumer'),
      baseTask('downstream'),
      startedTask('reviewed', { submittedResultAtUnixMs: NOW - DAY }),
      baseTask('review-downstream'),
    ];
    const result = deriveOverview(makeInput({
      tasks: complete(tasks),
      dependencies: complete([
        dependency('consumer', 'root'),
        dependency('downstream', 'consumer'),
        dependency('review-downstream', 'reviewed'),
      ]),
      decisions: complete([decision('root')]),
      reviewRounds: complete([reviewRound('reviewed', 'open', { reviewerPersonId: 'person-b' })]),
    }));
    const withOutside = deriveOverview(makeInput({
      tasks: complete(tasks.map((task) => task.id === 'root' ? {
        ...task,
        externalWait: { party: 'Outside Example', what: 'Confirm a synthetic value', sinceUnixMs: NOW - 5 * DAY },
      } : task)),
      dependencies: complete([
        dependency('consumer', 'root'),
        dependency('downstream', 'consumer'),
        dependency('review-downstream', 'reviewed'),
      ]),
      decisions: complete([decision('root')]),
      reviewRounds: complete([reviewRound('reviewed', 'open')]),
    }));
    expect(result.waits.map((wait) => wait.kind).sort()).toEqual(['decision', 'dependency', 'review']);
    expect(withOutside.waits.map((wait) => wait.kind).sort()).toEqual(['decision', 'dependency', 'external', 'review']);
    const rootDecision = withOutside.waits.find((wait) => wait.kind === 'decision')!;
    const downstream = withOutside.waits.find((wait) => wait.kind === 'dependency')!;
    expect(rootDecision.behind.count).toBe(3);
    expect(rootDecision.behind.items.map((task) => task.id)).toEqual(['root', 'consumer', 'downstream']);
    expect(downstream.behind.count).toBe(2);
    expect(downstream.behind.items.map((task) => task.id)).toEqual(['consumer', 'downstream']);
    expect(withOutside.waitCounts.all.count).toBe(4);
  });

  it('deduplicates shared downstream tasks and terminates cycles', () => {
    const tasks = ['root', 'left', 'right', 'shared'].map((taskId) => startedTask(taskId));
    const diamond = deriveOverview(makeInput({
      tasks: complete(tasks),
      dependencies: complete([
        dependency('left', 'root'), dependency('right', 'root'),
        dependency('shared', 'left'), dependency('shared', 'right'),
      ]),
      decisions: complete([decision('root')]),
    }));
    expect(diamond.waits[0].behind.count).toBe(4);
    expect(new Set(diamond.waits[0].behind.items).size).toBe(4);

    const cycle = deriveOverview(makeInput({
      tasks: complete([startedTask('a'), startedTask('b')]),
      dependencies: complete([dependency('b', 'a'), dependency('a', 'b')]),
      decisions: complete([decision('a')]),
    }));
    expect(cycle.waits[0].behind.count).toBe(2);
  });

  it('does not turn unknown graph facts into zero or add unknown nodes to the sum', () => {
    const result = deriveOverview(makeInput({
      tasks: complete([startedTask('known')]),
      dependencies: complete([dependency('known', 'missing-provider')]),
      decisions: complete([decision('known')]),
    }));
    const wait = result.waits.find((row) => row.kind === 'decision')!;
    expect(wait.behind.count).toBeNull();
    expect(wait.behind.items.map((task) => task.id)).toEqual(['known']);
    expect(wait.behind.unknownItems.length).toBeGreaterThan(0);
    expect(wait.behind.items.some((task) => task.id === 'missing-provider')).toBe(false);
  });

  it('keeps known waits but marks their impact and totals unknown when dependency reads are incomplete', () => {
    const result = deriveOverview(makeInput({
      tasks: complete([baseTask('root', {
        externalWait: { party: 'Outside Example', sinceUnixMs: NOW - DAY },
      })]),
      dependencies: { coverage: 'unread', reason: 'read-failed' },
    }));
    expect(result.waitCounts.all.count).toBeNull();
    expect(result.waits).toHaveLength(1);
    expect(result.waits[0].behind.count).toBeNull();
    expect(result.waits[0].behind.items.map((task) => task.id)).toEqual(['root']);
    expect(result.coverage.parts.dependencies.state).toBe('unread');
  });

  it('keeps wait start and waiter identity unknown instead of inventing zero days or a person', () => {
    const result = deriveOverview(makeInput({
      tasks: complete([startedTask('reviewed', { submittedResultAtUnixMs: NOW - DAY })]),
      reviewRounds: complete([reviewRound('reviewed', 'open', {
        reviewerPersonId: null, reviewerActorId: null, openedAtUnixMs: null,
      })]),
    }));
    expect(result.waits[0]).toMatchObject({
      kind: 'review', durationDays: null, waitingOn: [], waitingOnUnknown: true,
    });
    const future = deriveOverview(makeInput({
      tasks: complete([baseTask('decider')]),
      decisions: complete([decision('decider', { createdAtUnixMs: NOW + DAY })]),
    }));
    expect(future.waits[0].durationDays).toBeNull();
  });

  it('sorts by known impact, then elapsed days, with unknown values last', () => {
    const tasks: OverviewTask[] = [
      baseTask('small', { externalWait: { party: 'Outside One', sinceUnixMs: NOW - 9 * DAY } }),
      baseTask('large', { externalWait: { party: 'Outside Two', sinceUnixMs: NOW - DAY } }),
      baseTask('tail'),
    ];
    const result = deriveOverview(makeInput({
      tasks: complete(tasks),
      dependencies: complete([dependency('tail', 'large', { satisfied: true })]),
    }));
    expect(result.waits.map((wait) => wait.task.id)).toEqual(['large', 'small']);
    expect(result.waits.map((wait) => wait.behind.count)).toEqual([2, 1]);
  });

  it('pins every known count to the exact same detail array, including per-wait impact', () => {
    const result = deriveOverview(makeInput({
      tasks: complete([startedTask('root'), startedTask('consumer'), baseTask('tail')]),
      dependencies: complete([dependency('consumer', 'root'), dependency('tail', 'consumer')]),
      decisions: complete([decision('root')]),
    }));
    expectCountInvariant(result.taskCounts.all);
    expectCountInvariant(result.waitCounts.all);
    Object.values(result.taskCounts.byStatus).forEach(expectCountInvariant);
    Object.values(result.waitCounts.byKind).forEach(expectCountInvariant);
    result.waits.forEach((wait) => expectCountInvariant(wait.behind));
    expect(result.taskCounts.byStatus.running.count).toBe(1);
    expect(result.taskCounts.byStatus.running.items).toHaveLength(1);
    expect(result.taskCounts.byStatus.waiting.count).toBe(1);
    expect(result.taskCounts.byStatus.waiting.items).toHaveLength(1);
  });

  it('does not mutate inputs and is deterministic across calls', () => {
    const input = makeInput({
      tasks: complete([startedTask('root'), startedTask('consumer')]),
      dependencies: complete([dependency('consumer', 'root')]),
      decisions: complete([decision('root')]),
    });
    const before = structuredClone(input);
    const first = deriveOverview(input);
    const second = deriveOverview(input);
    expect(input).toEqual(before);
    expect(second).toEqual(first);
  });
});
