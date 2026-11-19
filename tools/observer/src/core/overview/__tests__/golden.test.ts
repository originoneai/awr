import { describe, expect, it } from 'vitest';
import PROGRESS from '../../../test-fixtures/work-state/progress-reports-run09.json';
import { deriveOverview } from '../index';
import type { OverviewReviewRound, OverviewTask } from '../index';
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

function syntheticGoldenInput() {
  const tasks = [
    startedTask('verified', {
      acceptance: supportedAcceptance(2), integrated: true, submittedResultAtUnixMs: NOW - DAY,
      report: { completed: true, reportedAtUnixMs: NOW - DAY }, agentActive: false,
    }),
    startedTask('claimed', {
      report: { completed: true, reportedAtUnixMs: NOW - DAY }, submittedResultAtUnixMs: null, agentActive: false,
    }),
    startedTask('returned', { submittedResultAtUnixMs: NOW - DAY, agentActive: false }),
    startedTask('review', { submittedResultAtUnixMs: NOW - DAY, agentActive: false }),
    startedTask('running'),
    startedTask('stalled', { report: { completed: false, reportedAtUnixMs: NOW - 4 * DAY } }),
    startedTask('waiting'),
    baseTask('ready'),
    baseTask('planned'),
    baseTask('external', {
      externalWait: { party: 'Outside Example', what: 'Confirm the synthetic handoff', sinceUnixMs: NOW - 5 * DAY },
    }),
  ];
  return makeInput({
    tasks: complete(tasks),
    dependencies: complete([
      dependency('waiting', 'review'),
      dependency('planned', 'waiting'),
    ]),
    decisions: complete([decision('returned')]),
    reviewRounds: complete([
      reviewRound('verified', 'approved'),
      reviewRound('returned', 'rejected'),
      reviewRound('review', 'open', { openedAtUnixMs: NOW - 4 * DAY }),
    ]),
  });
}

describe('status and waiting goldens', () => {
  it('keeps the complete synthetic organization golden tied to one task list', () => {
    const result = deriveOverview(syntheticGoldenInput());
    expect(result.tasks.map((task) => [task.id, task.status])).toEqual([
      ['verified', 'verified'],
      ['claimed', 'claimed'],
      ['returned', 'returned'],
      ['review', 'review'],
      ['running', 'running'],
      ['stalled', 'stalled'],
      ['waiting', 'waiting'],
      ['ready', 'ready'],
      ['planned', 'planned'],
      ['external', 'waiting'],
    ]);
    expect(result.waits.map((wait) => ({
      kind: wait.kind,
      task: wait.task.id,
      source: wait.sourceId,
      days: wait.durationDays,
      behind: wait.behind.count,
      details: wait.behind.items.map((task) => task.id),
      waitingOn: wait.waitingOn.map((member) => member.displayName),
      outside: wait.externalParty,
    }))).toEqual([
      {
        kind: 'review', task: 'review', source: 'round-review-open', days: 4, behind: 3,
        details: ['review', 'waiting', 'planned'], waitingOn: ['Reviewer Two'], outside: null,
      },
      {
        kind: 'dependency', task: 'waiting', source: 'edge-waiting-review', days: 2, behind: 2,
        details: ['waiting', 'planned'], waitingOn: ['Owner One'], outside: null,
      },
      {
        kind: 'external', task: 'external', source: null, days: 5, behind: 1,
        details: ['external'], waitingOn: [], outside: 'Outside Example',
      },
      {
        kind: 'decision', task: 'returned', source: 'decision-returned', days: 2, behind: 1,
        details: ['returned'], waitingOn: ['Owner One'], outside: null,
      },
    ]);
    expect({
      all: result.taskCounts.all.count,
      waits: result.waitCounts.all.count,
      statuses: Object.fromEntries(Object.entries(result.taskCounts.byStatus).map(([status, value]) => [status, value.count])),
      kinds: Object.fromEntries(Object.entries(result.waitCounts.byKind).map(([kind, value]) => [kind, value.count])),
      complete: result.coverage.complete,
    }).toEqual({
      all: 10,
      waits: 4,
      statuses: {
        verified: 1, claimed: 1, returned: 1, review: 1, running: 1,
        stalled: 1, waiting: 2, ready: 1, planned: 1, unknown: 0,
      },
      kinds: { decision: 1, dependency: 1, review: 1, external: 1 },
      complete: true,
    });
    expect(result.chains.map((chain) => [chain.root.kind, chain.impact.count])).toEqual([
      ['review', 3], ['external', 1], ['decision', 1],
    ]);
    expect(result.peopleCounts.all.count).toBe(3);
    expect(Object.fromEntries(result.people.map((summary) => [summary.person.id, summary.pending.count]))).toEqual({
      'person-a': 1, 'person-b': 1, 'person-c': 0,
    });
  });

  it('derives the same review golden from verbatim public run09 reply extracts, not a demo-only shape', () => {
    // These retained JSON values are copied verbatim from the real rehearsal replies. The adapter
    // supplies only canonical facts those reads establish; criterion-to-evidence mapping and the
    // reviewer/opening time remain unknown because the trimmed reads do not establish them.
    const report = PROGRESS['api-in-review-owned-by-other'];
    const taskId = 'issue-filter-api';
    const reviewArtifact = report.artifacts.find((artifact) => artifact.label === 'review round')!;
    const task: OverviewTask = {
      ...baseTask(taskId),
      title: taskId,
      acceptance: undefined,
      report: { completed: report.phase === 'delivered', reportedAtUnixMs: report.reported_at_unix_ms },
      submittedResultAtUnixMs: null,
      integrated: false,
      started: true,
      agentActive: false,
      externalWait: null,
    };
    const round: OverviewReviewRound = {
      id: reviewArtifact.reference,
      taskId,
      roundIndex: 1,
      state: 'open',
      current: report.contract_matches_current,
      independent: null,
      authorPersonId: null,
      reviewerPersonId: null,
      authorActorId: null,
      reviewerActorId: null,
      openedAtUnixMs: null,
    };
    const result = deriveOverview(makeInput({
      tasks: complete([task]),
      reviewRounds: complete([round]),
      options: { nowUnixMs: report.reported_at_unix_ms + 2 * DAY, stalledAfterDays: 3 },
    }));
    expect(result.tasks.map((row) => ({
      id: row.id, status: row.status, criteria: row.verification.everyCriterionSupported,
    }))).toEqual([{ id: taskId, status: 'review', criteria: null }]);
    expect(result.waits.map((wait) => ({
      kind: wait.kind,
      source: wait.sourceId,
      task: wait.task.id,
      days: wait.durationDays,
      behind: wait.behind.count,
      details: wait.behind.items.map((item) => item.id),
      waiterUnknown: wait.waitingOnUnknown,
    }))).toEqual([{
      kind: 'review', source: '01M6AVQ870N7MBRJ5MPCN79T55', task: taskId, days: null,
      behind: 1, details: [taskId], waiterUnknown: true,
    }]);
    expect(result.taskCounts.all.count).toBe(1);
    expect(result.waitCounts.byKind.review.count).toBe(1);
    expect(result.chains.map((chain) => ({
      kind: chain.root.kind,
      impact: chain.impact.count,
      details: chain.impact.items.map((item) => item.id),
      commitments: chain.commitments.count,
    }))).toEqual([{ kind: 'review', impact: 1, details: [taskId], commitments: null }]);
    for (const view of Object.values(result.views)) {
      for (const item of view.metrics) expect(item.count).toBeNull();
    }
    expect(result.coverage.sections.status.complete).toBe(true);
    expect(result.coverage.sections.waits.complete).toBe(false);
    expect(result.coverage.sections.chains.complete).toBe(false);
    expect(result.coverage.sections.views.complete).toBe(false);
  });
});
