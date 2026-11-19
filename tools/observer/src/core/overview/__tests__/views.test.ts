import { describe, expect, it } from 'vitest';
import { deriveOverview, VIEW_IDS } from '../index';
import type { CountDetails, DerivedTask, OverviewTask, OverviewViewOptions } from '../index';
import {
  DAY,
  NOW,
  baseTask,
  complete,
  decision,
  makeInput,
  reviewRound,
  startedTask,
  supportedAcceptance,
} from './factories';

function scopedTask(id: string, projectId: string, groupId: string, goalId: string, overrides = {}): OverviewTask {
  return baseTask(id, {
    projectId, groupId, goalIds: [goalId], commitments: [], ...overrides,
  });
}

function verifiedTask(id: string, projectId: string, groupId: string, goalId: string, at: number): OverviewTask {
  return startedTask(id, {
    projectId, groupId, goalIds: [goalId], commitments: [], verifiedAtUnixMs: at,
    acceptance: supportedAcceptance(1), submittedResultAtUnixMs: at - DAY, integrated: true,
    report: { completed: true, reportedAtUnixMs: at }, agentActive: false, agentName: null,
  });
}

function viewsInput() {
  const tasks = [
    verifiedTask('verified-current', 'project-one', 'group-one', 'goal-one', NOW),
    verifiedTask('verified-previous', 'project-one', 'group-one', 'goal-one', NOW - 8 * DAY),
    verifiedTask('verified-other', 'project-two', 'group-two', 'goal-two', NOW - DAY),
    scopedTask('running-one', 'project-one', 'group-one', 'goal-one', {
      started: true, agentActive: true, agentName: 'Shared Agent',
    }),
    scopedTask('running-two', 'project-one', 'group-one', 'goal-one', {
      started: true, agentActive: true, agentName: 'Shared Agent',
    }),
    scopedTask('cross-review', 'project-one', 'group-one', 'goal-one', {
      started: true, agentActive: false, submittedResultAtUnixMs: NOW - DAY,
    }),
    scopedTask('outside', 'project-one', 'group-one', 'goal-one', {
      agentName: null, externalWait: { party: 'Outside Example', sinceUnixMs: NOW - 2 * DAY },
    }),
    scopedTask('other-decision', 'project-two', 'group-two', 'goal-two', { started: true, agentActive: true }),
    scopedTask('claimed', 'project-one', 'group-one', 'goal-one', {
      started: true, agentActive: false, agentName: null,
      report: { completed: true, reportedAtUnixMs: NOW - DAY }, submittedResultAtUnixMs: null,
    }),
  ];
  const views: OverviewViewOptions = {
    me: { personId: 'person-a', projectId: 'project-one' },
    lead: { personId: 'person-c', projectId: 'project-one' },
    department: { personId: 'person-c', projectIds: ['project-one', 'project-two', 'project-empty'] },
    top: { personId: 'person-c', goalIds: ['goal-one', 'goal-two', 'goal-empty'] },
    // Adjacent periods never share a boundary value: both ends are inclusive, so the caller
    // must keep the previous period one millisecond short of the current one.
    currentWeek: { startUnixMs: NOW - 7 * DAY, endUnixMs: NOW },
    previousWeek: { startUnixMs: NOW - 14 * DAY, endUnixMs: NOW - 7 * DAY - 1 },
    currentMonth: { startUnixMs: NOW - 30 * DAY, endUnixMs: NOW },
    previousMonth: { startUnixMs: NOW - 60 * DAY, endUnixMs: NOW - 30 * DAY - 1 },
  };
  return makeInput({
    tasks: complete(tasks),
    members: complete([
      { personId: 'person-a', displayName: 'Owner One', groupId: 'group-one' },
      { personId: 'person-b', displayName: 'Reviewer Two', groupId: 'group-two' },
      { personId: 'person-c', displayName: 'Owner Three', groupId: 'group-two' },
    ]),
    decisions: complete([
      decision('running-one', { allowedResponderIds: ['person-a'] }),
      decision('other-decision', { allowedResponderIds: ['person-c'] }),
    ]),
    reviewRounds: complete([
      reviewRound('verified-current', 'approved'),
      reviewRound('verified-previous', 'approved'),
      reviewRound('verified-other', 'approved'),
      reviewRound('cross-review', 'open', { reviewerPersonId: 'person-b' }),
    ]),
    options: { nowUnixMs: NOW, stalledAfterDays: 3, views },
  });
}

function expectKnown<T>(metric: CountDetails<T>, count: number) {
  expect(metric.count).toBe(count);
  expect(metric.items).toHaveLength(count);
}

describe('four prototype views', () => {
  it('derives exactly four metrics per view in prototype order with inclusive vcount periods', () => {
    const result = deriveOverview(viewsInput());
    expect(VIEW_IDS).toEqual(['me', 'lead', 'department', 'top']);
    for (const view of Object.values(result.views)) expect(view.metrics).toHaveLength(4);
    expect(Object.fromEntries(VIEW_IDS.map((view) => [view, result.views[view].metrics.map((metric) => metric.key)]))).toEqual({
      me: ['verified-period', 'agents-running', 'waiting-by-me', 'pending-for-me'],
      lead: ['verified-period', 'in-progress', 'waits', 'pending-for-me'],
      department: ['projects', 'verified-period', 'cross-group-waits', 'pending-for-me'],
      top: ['goals', 'verified-total', 'blockers', 'decisions-for-me'],
    });

    const me = result.views.me.metrics;
    expectKnown(me[0], 1);
    expectKnown(me[0].previousPeriod!, 1);
    expectKnown(me[1], 1);
    expect((me[1].items[0] as { tasks: DerivedTask[] }).tasks).toHaveLength(2);
    expectKnown(me[2], 3);
    expectKnown(me[3], 1);

    const lead = result.views.lead.metrics;
    expectKnown(lead[0], 1);
    expectKnown(lead[0].previousPeriod!, 1);
    expectKnown(lead[1], 4);
    expectKnown(lead[2], 3);
    expectKnown(lead[3], 0);

    const department = result.views.department.metrics;
    expectKnown(department[0], 3);
    expectKnown(department[1], 3);
    expectKnown(department[1].previousPeriod!, 0);
    expectKnown(department[2], 2);
    expectKnown(department[3], 1);

    const top = result.views.top.metrics;
    expectKnown(top[0], 3);
    expectKnown(top[1], 3);
    expectKnown(top[1].claimed!, 1);
    expectKnown(top[2], 4);
    expectKnown(top[3], 1);
    expect(result.coverage.sections.views.complete).toBe(true);
  });

  it('keeps missing verification times, groups, goals and view options unknown', () => {
    const input = viewsInput();
    const missingTime = structuredClone(input);
    if (missingTime.tasks.coverage !== 'unread') missingTime.tasks.items[0].verifiedAtUnixMs = null;
    const timeResult = deriveOverview(missingTime);
    expect(timeResult.views.me.metrics[0].count).toBeNull();
    expect(timeResult.views.top.metrics[1].count).toBe(3);

    const missingGroup = structuredClone(input);
    if (missingGroup.members.coverage !== 'unread') missingGroup.members.items[1].groupId = null;
    expect(deriveOverview(missingGroup).views.department.metrics[2].count).toBeNull();

    const missingGoals = structuredClone(input);
    if (missingGoals.tasks.coverage !== 'unread') missingGoals.tasks.items[0].goalIds = null;
    expect(deriveOverview(missingGoals).views.top.metrics[0].count).toBeNull();

    const noOptions = deriveOverview(makeInput({ tasks: complete([baseTask('task')]) }));
    for (const view of Object.values(noOptions.views)) {
      for (const item of view.metrics) expect(item.count).toBeNull();
    }
    expect(noOptions.coverage.sections.views.complete).toBe(false);
  });

  it('counts a review waiting on somebody else\'s task as pending for the viewer in the me view', () => {
    const views: OverviewViewOptions = {
      me: { personId: 'person-b', projectId: 'project-one' },
      lead: { personId: 'person-b', projectId: 'project-one' },
      currentWeek: { startUnixMs: NOW - 7 * DAY, endUnixMs: NOW },
      previousWeek: { startUnixMs: NOW - 14 * DAY, endUnixMs: NOW - 7 * DAY - 1 },
      currentMonth: { startUnixMs: NOW - 30 * DAY, endUnixMs: NOW },
      previousMonth: { startUnixMs: NOW - 60 * DAY, endUnixMs: NOW - 30 * DAY - 1 },
    };
    const result = deriveOverview(makeInput({
      tasks: complete([
        scopedTask('own-task', 'project-one', 'group-one', 'goal-one', { ownerPersonId: 'person-b' }),
        scopedTask('reviewed-task', 'project-one', 'group-one', 'goal-one', {
          ownerPersonId: 'person-c', started: true, agentActive: false, submittedResultAtUnixMs: NOW - DAY,
        }),
        scopedTask('other-project-task', 'project-two', 'group-two', 'goal-two', {
          ownerPersonId: 'person-c', started: true, agentActive: false, submittedResultAtUnixMs: NOW - DAY,
        }),
      ]),
      reviewRounds: complete([
        reviewRound('reviewed-task', 'open', { reviewerPersonId: 'person-b', authorPersonId: 'person-c' }),
        reviewRound('other-project-task', 'open', { reviewerPersonId: 'person-b', authorPersonId: 'person-c' }),
      ]),
      options: { nowUnixMs: NOW, stalledAfterDays: 3, views },
    }));
    // The person-level summary is project-agnostic; both review queues wait for person-b.
    expect(result.people.find((summary) => summary.person.id === 'person-b')?.pending.count).toBe(2);
    // The lead view already scopes the viewer's project, not the viewer's own tasks.
    expect(result.views.lead.metrics[3].count).toBe(1);
    // The me view must agree: the pending queue is not limited to tasks the viewer owns.
    expect(result.views.me.metrics[3].count).toBe(1);
    expect(result.views.me.metrics[3].items.map((item) => (item as { task: { id: string } }).task.id))
      .toEqual(['reviewed-task']);
  });

  it('counts tasks verified exactly at the inclusive period bounds and not one millisecond outside', () => {
    const weekStart = NOW - 7 * DAY;
    const weekEnd = NOW - DAY;
    const views: OverviewViewOptions = {
      me: { personId: 'person-a', projectId: 'project-one' },
      lead: { personId: 'person-c', projectId: 'project-one' },
      currentWeek: { startUnixMs: weekStart, endUnixMs: weekEnd },
      previousWeek: { startUnixMs: NOW - 14 * DAY, endUnixMs: weekStart - 1 },
      currentMonth: { startUnixMs: NOW - 30 * DAY, endUnixMs: NOW },
      previousMonth: { startUnixMs: NOW - 60 * DAY, endUnixMs: NOW - 30 * DAY - 1 },
    };
    const result = deriveOverview(makeInput({
      tasks: complete([
        verifiedTask('at-start', 'project-one', 'group-one', 'goal-one', weekStart),
        verifiedTask('at-end', 'project-one', 'group-one', 'goal-one', weekEnd),
        verifiedTask('just-before', 'project-one', 'group-one', 'goal-one', weekStart - 1),
        verifiedTask('just-after', 'project-one', 'group-one', 'goal-one', weekEnd + 1),
      ]),
      reviewRounds: complete(['at-start', 'at-end', 'just-before', 'just-after'].map((id) => reviewRound(id, 'approved'))),
      options: { nowUnixMs: NOW, stalledAfterDays: 3, views },
    }));
    const current = result.views.me.metrics[0];
    expect(current.count).toBe(2);
    expect(current.items.map((task) => (task as DerivedTask).id).sort()).toEqual(['at-end', 'at-start']);
    // One millisecond before the inclusive start belongs to the previous period, not this one.
    expect(current.previousPeriod!.count).toBe(1);
    expect(current.previousPeriod!.items.map((task) => (task as DerivedTask).id)).toEqual(['just-before']);
    // One millisecond after the inclusive end belongs to neither period.
    expect(result.views.me.metrics[0].items.concat(current.previousPeriod!.items)
      .some((task) => (task as DerivedTask).id === 'just-after')).toBe(false);
  });

  it('keeps the department pending queue inside the department project scope', () => {
    const views: OverviewViewOptions = {
      me: { personId: 'person-c', projectId: 'project-one' },
      lead: { personId: 'person-c', projectId: 'project-one' },
      department: { personId: 'person-c', projectIds: ['project-one'] },
      currentWeek: { startUnixMs: NOW - 7 * DAY, endUnixMs: NOW },
      previousWeek: { startUnixMs: NOW - 14 * DAY, endUnixMs: NOW - 7 * DAY - 1 },
      currentMonth: { startUnixMs: NOW - 30 * DAY, endUnixMs: NOW },
      previousMonth: { startUnixMs: NOW - 60 * DAY, endUnixMs: NOW - 30 * DAY - 1 },
    };
    const result = deriveOverview(makeInput({
      tasks: complete([
        scopedTask('inside', 'project-one', 'group-one', 'goal-one', {
          ownerPersonId: 'person-a', started: true, agentActive: false, submittedResultAtUnixMs: NOW - DAY,
        }),
        scopedTask('outside', 'project-two', 'group-two', 'goal-two', {
          ownerPersonId: 'person-a', started: true, agentActive: false, submittedResultAtUnixMs: NOW - DAY,
        }),
      ]),
      reviewRounds: complete([
        reviewRound('inside', 'open', { reviewerPersonId: 'person-c', authorPersonId: 'person-a' }),
        reviewRound('outside', 'open', { reviewerPersonId: 'person-c', authorPersonId: 'person-a' }),
      ]),
      options: { nowUnixMs: NOW, stalledAfterDays: 3, views },
    }));
    // Both queues wait for person-c at the person level; the department counts only its projects.
    expect(result.people.find((summary) => summary.person.id === 'person-c')?.pending.count).toBe(2);
    const pending = result.views.department.metrics[3];
    expect(pending.count).toBe(1);
    expect(pending.items.map((item) => (item as { task: { id: string } }).task.id)).toEqual(['inside']);
  });

  it('pins every view number and period comparison to its own detail array', () => {
    const result = deriveOverview(viewsInput());
    for (const view of Object.values(result.views)) {
      for (const item of view.metrics) {
        if (item.count != null) expect(item.count).toBe(item.items.length);
        if (item.previousPeriod?.count != null) expect(item.previousPeriod.count).toBe(item.previousPeriod.items.length);
        if (item.claimed?.count != null) expect(item.claimed.count).toBe(item.claimed.items.length);
      }
    }
  });
});
