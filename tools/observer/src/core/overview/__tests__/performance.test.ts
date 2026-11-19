import { describe, expect, it } from 'vitest';
import { deriveOverview, VIEW_IDS } from '../index';
import type { OverviewDependency, OverviewTask, OverviewViewOptions } from '../index';
import {
  DAY, NOW, approval, complete, decision, dependency, makeInput, reviewRound, startedTask,
} from './factories';

function benchmarkInput() {
  const tasks: OverviewTask[] = Array.from({ length: 4000 }, (_, index) => {
    const projectId = index < 2000 ? 'project-a' : 'project-b';
    return startedTask(`task-${index}`, {
      projectId,
      groupId: index % 2 === 0 ? 'group-a' : 'group-b',
      goalIds: [index < 2000 ? 'goal-a' : 'goal-b'],
      commitments: [{ id: `promise-${projectId}`, title: `Promise ${projectId}`, kind: 'project' }],
      verifiedAtUnixMs: null,
      report: { completed: false, reportedAtUnixMs: NOW - 2000 },
    });
  });
  const dependencies: OverviewDependency[] = [];
  let consumer = 1000;
  while (dependencies.length < 20000) {
    for (let slot = 0; slot < 6 && dependencies.length < 20000; slot += 1) {
      const provider = consumer >= 1000 ? (consumer + slot) % 1000 : slot;
      dependencies.push(dependency(`task-${consumer}`, `task-${provider}`, {
        satisfied: dependencies.length % 197 !== 0,
        sinceUnixMs: NOW - (dependencies.length % 9) * 1000,
      }));
    }
    consumer = consumer === 3999 ? 6 : consumer + 1;
  }
  for (let index = 0; index < 50; index += 1) {
    tasks[200 + index].externalWait = {
      party: `Outside Example ${index}`, what: 'Synthetic benchmark wait', sinceUnixMs: NOW - index * 1000,
    };
  }
  const views: OverviewViewOptions = {
    me: { personId: 'person-a', projectId: 'project-a' },
    lead: { personId: 'person-a', projectId: 'project-a' },
    department: { personId: 'person-c', projectIds: ['project-a', 'project-b'] },
    top: { personId: 'person-c', goalIds: ['goal-a', 'goal-b'] },
    currentWeek: { startUnixMs: NOW - 7 * DAY, endUnixMs: NOW },
    previousWeek: { startUnixMs: NOW - 14 * DAY, endUnixMs: NOW - 7 * DAY },
    currentMonth: { startUnixMs: NOW - 30 * DAY, endUnixMs: NOW },
    previousMonth: { startUnixMs: NOW - 60 * DAY, endUnixMs: NOW - 30 * DAY },
  };
  return makeInput({
    tasks: complete(tasks),
    dependencies: complete(dependencies),
    decisions: complete(Array.from({ length: 50 }, (_, index) => decision(`task-${index}`))),
    reviewRounds: complete(Array.from({ length: 50 }, (_, index) => reviewRound(`task-${50 + index}`, 'open'))),
    handoffs: complete(Array.from({ length: 100 }, (_, index) => ({
      id: `handoff-${index}`, taskId: `task-${index}`, kind: 'execution', status: 'accepted', pending: false,
      fromPersonId: 'person-a', toPersonId: 'person-c', updatedAtUnixMs: NOW - 1000,
    }))),
    approvals: complete(Array.from({ length: 500 }, (_, index) => approval(
      index % 2 === 0 ? 'plan-approval' : 'lesson-confirmation',
      index % 2 === 0 ? 'person-a' : 'person-c',
      {
        id: `approval-${index}`,
        projectId: index % 4 === 3 ? 'project-b' : 'project-a',
        title: `Synthetic approval ${index}`,
        sinceUnixMs: NOW - index * 1000,
      },
    ))),
    members: complete([
      { personId: 'person-a', displayName: 'Owner One', groupId: 'group-a' },
      { personId: 'person-b', displayName: 'Reviewer Two', groupId: 'group-b' },
      { personId: 'person-c', displayName: 'Owner Three', groupId: 'group-b' },
    ]),
    activity: complete(tasks.map((task) => ({ id: `activity-${task.id}`, taskId: task.id, atUnixMs: NOW - 1000 }))),
    options: { nowUnixMs: NOW, stalledAfterDays: 3, views },
  });
}

describe('full overview derivation performance', () => {
  it('derives every slice for 4000 tasks and 20000 dependencies in under 500 ms', () => {
    const input = benchmarkInput();
    expect(input.dependencies.items).toHaveLength(20000);
    deriveOverview(input); // Warm the same code path; data construction and warmup are not measured.
    const started = process.hrtime.bigint();
    const result = deriveOverview(input);
    const elapsedMs = Number(process.hrtime.bigint() - started) / 1_000_000;
    console.info(`[overview benchmark] 4000 tasks, 20000 dependencies, ${result.waits.length} waits, ${result.chains.length} chains: ${elapsedMs.toFixed(1)} ms`);
    expect(result.tasks).toHaveLength(4000);
    expect(result.taskCounts.all.count).toBe(4000);
    expect(result.waitCounts.all.count).toBe(result.waits.length);
    expect(result.waitCounts.byKind.decision.count).toBe(50);
    expect(result.waitCounts.byKind.review.count).toBe(50);
    expect(result.waitCounts.byKind.external.count).toBe(50);
    expect(result.waitCounts.byKind.dependency.count).toBeGreaterThan(50);
    expect(result.chains.length).toBeGreaterThan(100);
    expect(result.peopleCounts.all.count).toBe(3);
    // The approvals plane rides the same derivation: 250 plan approvals for person-a in
    // project-a beside their 50 decision waits, and 250 lesson confirmations for person-c.
    const owner = result.people.find((summary) => summary.person.id === 'person-a')!;
    expect(owner.pending.count).toBe(300);
    expect(result.views.me.metrics[3].count).toBe(300);
    expect(result.views.department.metrics[3].count).toBe(250);
    for (const viewId of VIEW_IDS) expect(result.views[viewId].metrics).toHaveLength(4);
    expect(result.coverage.complete).toBe(true);
    expect(result.coverage.sections.chains.complete).toBe(true);
    expect(result.coverage.sections.people.complete).toBe(true);
    expect(result.coverage.sections.views.complete).toBe(true);
    expect(elapsedMs).toBeLessThan(500);
  }, 10_000);
});
