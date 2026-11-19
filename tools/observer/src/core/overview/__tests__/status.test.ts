import { describe, expect, it } from 'vitest';
import { deriveOverview, TASK_STATUSES } from '../index';
import type { OverviewInput, OverviewTask } from '../index';
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
  supportedAcceptance,
} from './factories';

function one(task: OverviewTask, overrides: Partial<OverviewInput> = {}) {
  const result = deriveOverview(makeInput({
    tasks: complete([task]),
    ...overrides,
  }));
  return result.tasks[0];
}

function verifiedTask() {
  return startedTask('verified', {
    acceptance: supportedAcceptance(2),
    report: { completed: true, reportedAtUnixMs: NOW - DAY },
    submittedResultAtUnixMs: NOW - 2 * DAY,
    integrated: true,
    agentActive: false,
  });
}

function verifiedInput(rounds = [reviewRound('verified', 'approved')]) {
  return { reviewRounds: complete(rounds) };
}

describe('nine task states', () => {
  it('derives verified only from criterion evidence, independent approval and integration together', () => {
    const task = one(verifiedTask(), verifiedInput());
    expect(task.status).toBe('verified');
    expect(task.verification).toEqual({
      everyCriterionSupported: true,
      independentReviewApproved: true,
      integrated: true,
    });
  });

  it('derives reported complete without a submitted result as claimed', () => {
    const task = one(startedTask('claimed', {
      report: { completed: true, reportedAtUnixMs: NOW - DAY },
      submittedResultAtUnixMs: null,
      agentActive: false,
    }));
    expect(task.status).toBe('claimed');
    expect(one(startedTask('claimed', { report: { completed: true, reportedAtUnixMs: NOW - DAY } }), {
      reviewRounds: complete([reviewRound('claimed', 'open')]),
    }).status).toBe('review');
  });

  it('uses the latest current rejected round for returned', () => {
    const task = one(startedTask('returned', { submittedResultAtUnixMs: NOW - DAY }), {
      reviewRounds: complete([
        reviewRound('returned', 'approved', { id: 'historical', roundIndex: 1, current: false }),
        reviewRound('returned', 'rejected', { id: 'current', roundIndex: 2 }),
      ]),
    });
    expect(task.status).toBe('returned');
  });

  it('uses the latest current open round for review', () => {
    const task = one(startedTask('review', { submittedResultAtUnixMs: NOW - DAY }), {
      reviewRounds: complete([
        reviewRound('review', 'rejected', { id: 'historical', roundIndex: 1, current: false }),
        reviewRound('review', 'open', { id: 'current', roundIndex: 2 }),
      ]),
    });
    expect(task.status).toBe('review');
  });

  it('uses fresh report or activity time for running', () => {
    expect(one(startedTask('running')).status).toBe('running');
    const noReportTime = startedTask('running', { report: { completed: false, reportedAtUnixMs: null } });
    expect(one(noReportTime, {
      activity: complete([{ id: 'event', taskId: 'running', atUnixMs: NOW - 1000 }]),
    }).status).toBe('running');
  });

  it('applies the caller threshold at the exact boundary and has no hidden default', () => {
    const old = startedTask('stalled', {
      report: { completed: false, reportedAtUnixMs: NOW - 3 * DAY },
    });
    expect(one(old).status).toBe('stalled');
    expect(one(old, { options: { nowUnixMs: NOW, stalledAfterDays: 4 } }).status).toBe('running');
    expect(one(old, { options: { nowUnixMs: NOW, stalledAfterDays: 2 } }).status).toBe('stalled');
    expect(() => deriveOverview(makeInput({ options: { nowUnixMs: NOW } as OverviewInput['options'] })))
      .toThrow('stalled threshold');
  });

  it('distinguishes started dependency waits, outside waits and decision waits', () => {
    expect(one(startedTask('waiting'), { dependencies: complete([dependency('waiting', 'provider')]) }).status)
      .toBe('waiting'); // The wait is known; its graph impact remains unknown and is tested separately.
    const tasks = [startedTask('consumer'), baseTask('provider')];
    expect(deriveOverview(makeInput({
      tasks: complete(tasks), dependencies: complete([dependency('consumer', 'provider')]),
    })).tasks[0].status).toBe('waiting');
    expect(one(baseTask('outside', {
      externalWait: { party: 'Outside Example', what: 'Confirm a synthetic value', sinceUnixMs: NOW - DAY },
    })).status).toBe('waiting');
    expect(one(baseTask('decider'), { decisions: complete([decision('decider')]) }).status).toBe('waiting');
  });

  it('distinguishes ready from planned using written criteria and upstream state', () => {
    expect(one(baseTask('ready')).status).toBe('ready');
    expect(one(baseTask('planned', { acceptance: [] })).status).toBe('planned');
    expect(deriveOverview(makeInput({
      tasks: complete([baseTask('consumer'), baseTask('provider')]),
      dependencies: complete([dependency('consumer', 'provider')]),
    })).tasks[0].status).toBe('planned');
    expect(deriveOverview(makeInput({
      tasks: complete([baseTask('consumer'), baseTask('provider')]),
      dependencies: complete([dependency('consumer', 'provider', { satisfied: true })]),
    })).tasks[0].status).toBe('ready');
  });

  it('exports exactly the nine prototype statuses, with unknown kept separate', () => {
    expect(TASK_STATUSES).toEqual([
      'verified', 'claimed', 'returned', 'review', 'running', 'stalled', 'waiting', 'ready', 'planned',
    ]);
    expect(one(baseTask('unknown', { started: undefined })).status).toBe('unknown');
  });
});

describe('verified and unknown guards', () => {
  it.each([
    ['one criterion lacks evidence', verifiedTask(), [reviewRound('verified', 'approved')], (task: OverviewTask) => {
      task.acceptance = [{ evidence: [{ passed: true, current: true }] }, { evidence: [] }];
    }],
    ['evidence passed only on an old candidate', verifiedTask(), [reviewRound('verified', 'approved')], (task: OverviewTask) => {
      task.acceptance = [{ evidence: [{ passed: true, current: false }] }];
    }],
    ['review is not independent', verifiedTask(), [reviewRound('verified', 'approved', { independent: false })], undefined],
    ['reviewer is the author', verifiedTask(), [reviewRound('verified', 'approved', { reviewerPersonId: 'person-a' })], undefined],
    ['result is not integrated', verifiedTask(), [reviewRound('verified', 'approved')], (task: OverviewTask) => {
      task.integrated = false;
    }],
    ['integration is unknown', verifiedTask(), [reviewRound('verified', 'approved')], (task: OverviewTask) => {
      task.integrated = undefined;
    }],
  ])('does not call a task verified when %s', (_name, original, rounds, mutate) => {
    const task = structuredClone(original);
    mutate?.(task);
    const result = one(task, { reviewRounds: complete(rounds) });
    expect(result.status).not.toBe('verified');
    expect(result.status).toBe('unknown');
  });

  it('keeps activity time unknown rather than converting it to zero age', () => {
    expect(one(startedTask('unknown-time', { report: { completed: false, reportedAtUnixMs: null } })).status)
      .toBe('unknown');
  });

  it('does not call an old task stalled while newer activity may be unread', () => {
    const old = startedTask('partial-activity', {
      report: { completed: false, reportedAtUnixMs: NOW - 10 * DAY },
    });
    expect(one(old).status).toBe('stalled');
    expect(one(old, { activity: { coverage: 'unread', reason: 'read-failed' } }).status).toBe('unknown');
  });
});
