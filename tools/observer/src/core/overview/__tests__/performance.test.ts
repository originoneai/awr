import { describe, expect, it } from 'vitest';
import { deriveOverview } from '../index';
import type { OverviewDependency, OverviewTask } from '../index';
import { NOW, complete, decision, dependency, makeInput, reviewRound, startedTask } from './factories';

function benchmarkInput() {
  const tasks: OverviewTask[] = Array.from({ length: 4000 }, (_, index) => startedTask(`task-${index}`, {
    report: { completed: false, reportedAtUnixMs: NOW - 2000 },
  }));
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
  return makeInput({
    tasks: complete(tasks),
    dependencies: complete(dependencies),
    decisions: complete(Array.from({ length: 50 }, (_, index) => decision(`task-${index}`))),
    reviewRounds: complete(Array.from({ length: 50 }, (_, index) => reviewRound(`task-${50 + index}`, 'open'))),
    handoffs: complete(Array.from({ length: 100 }, (_, index) => ({
      id: `handoff-${index}`, taskId: `task-${index}`, kind: 'execution', status: 'accepted', pending: false,
      fromPersonId: 'person-a', toPersonId: 'person-c', updatedAtUnixMs: NOW - 1000,
    }))),
    activity: complete(tasks.map((task) => ({ id: `activity-${task.id}`, taskId: task.id, atUnixMs: NOW - 1000 }))),
  });
}

describe('status and waiting derivation performance', () => {
  it('derives 4000 tasks and 20000 dependencies with all wait kinds in under 500 ms', () => {
    const input = benchmarkInput();
    expect(input.dependencies.items).toHaveLength(20000);
    deriveOverview(input); // Warm the same code path; data construction and warmup are not measured.
    const started = process.hrtime.bigint();
    const result = deriveOverview(input);
    const elapsedMs = Number(process.hrtime.bigint() - started) / 1_000_000;
    console.info(`[overview benchmark] 4000 tasks, 20000 dependencies, ${result.waits.length} waits: ${elapsedMs.toFixed(1)} ms`);
    expect(result.tasks).toHaveLength(4000);
    expect(result.taskCounts.all.count).toBe(4000);
    expect(result.waitCounts.all.count).toBe(result.waits.length);
    expect(result.waitCounts.byKind.decision.count).toBe(50);
    expect(result.waitCounts.byKind.review.count).toBe(50);
    expect(result.waitCounts.byKind.external.count).toBe(50);
    expect(result.waitCounts.byKind.dependency.count).toBeGreaterThan(50);
    expect(result.coverage.complete).toBe(true);
    expect(elapsedMs).toBeLessThan(500);
  }, 10_000);
});
