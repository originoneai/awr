import { describe, expect, it } from 'vitest';
import { deriveOverview } from '../index';
import type { OverviewDependency, OverviewInput, OverviewTask } from '../index';
import { NOW, baseTask, complete, dependency, makeInput } from './factories';

/** A local LCG makes the generated graph reproducible without sharing production code. */
function randomSource(seed = 0x5eed1234) {
  let state = seed >>> 0;
  return () => {
    state = (Math.imul(state, 1664525) + 1013904223) >>> 0;
    return state / 0x1_0000_0000;
  };
}

function generatedGraph(blocks: number, blockSize: number, waitCount: number) {
  const random = randomSource();
  const size = blocks * blockSize;
  const tasks: OverviewTask[] = Array.from({ length: size }, (_, index) => baseTask(`task-${index}`, {
    commitments: [],
    externalWait: index < waitCount
      ? { party: `Outside ${index}`, what: 'Generated wait', sinceUnixMs: NOW - (index % 11) }
      : null,
  }));
  const dependencies: OverviewDependency[] = [];
  const edges = new Set<string>();
  const add = (consumer: number, provider: number) => {
    const key = `${consumer}:${provider}`;
    if (edges.has(key)) return;
    edges.add(key);
    dependencies.push(dependency(`task-${consumer}`, `task-${provider}`, { satisfied: true }));
  };
  for (let block = 0; block < blocks; block += 1) {
    const start = block * blockSize;
    // Every block has a two-node cycle and a diamond with a shared downstream node.
    add(start + 1, start);
    add(start, start + 1);
    add(start + 3, start + 2);
    add(start + 4, start + 2);
    add(start + 5, start + 3);
    add(start + 5, start + 4);
    for (let local = 0; local < blockSize; local += 1) {
      for (let edge = 0; edge < 4; edge += 1) {
        const provider = start + Math.floor(random() * blockSize);
        add(start + local, provider);
      }
    }
  }
  return { tasks, dependencies };
}

/** Naive point-by-point reachability. It intentionally shares no helper with the implementation. */
function naiveReachability(taskIds: readonly string[], dependencies: readonly OverviewDependency[], root: string) {
  const nextByProvider = new Map<string, string[]>();
  for (const edge of dependencies) {
    const provider = edge.providerTaskId!;
    const consumer = edge.consumerTaskId!;
    const next = nextByProvider.get(provider) ?? [];
    next.push(consumer);
    nextByProvider.set(provider, next);
  }
  const seen = new Set<string>([root]);
  const queue = [root];
  for (let cursor = 0; cursor < queue.length; cursor += 1) {
    for (const next of nextByProvider.get(queue[cursor]) ?? []) {
      if (seen.has(next)) continue;
      seen.add(next);
      queue.push(next);
    }
  }
  return taskIds.filter((taskId) => seen.has(taskId));
}

function expectOracleMatch(input: OverviewInput, expectComplete: boolean) {
  if (input.tasks.coverage === 'unread' || input.dependencies.coverage === 'unread') throw new Error('Test setup error');
  const tasks = input.tasks.items;
  const dependencies = input.dependencies.items;
  const taskIds = tasks.map((task) => task.id!);
  const result = deriveOverview(input);
  const expectedByRoot = new Map<string, string[]>();
  for (const wait of result.waits) {
    const root = wait.task.id!;
    const expected = expectedByRoot.get(root) ?? naiveReachability(taskIds, dependencies, root);
    expectedByRoot.set(root, expected);
    expect(wait.behind.items.map((task) => task.id), `wait ${root}`).toEqual(expected);
    if (expectComplete) expect(wait.behind.count, `wait ${root}`).toBe(expected.length);
    else expect(wait.behind.count, `wait ${root}`).toBeNull();
  }
  expect(result.chains).toHaveLength(result.waits.length);
  for (const chain of result.chains) {
    const roots = chain.root.queue.items.map((wait) => wait.task.id!);
    const expectedSet = new Set<string>();
    for (const root of roots) {
      const expected = expectedByRoot.get(root) ?? naiveReachability(taskIds, dependencies, root);
      expectedByRoot.set(root, expected);
      expected.forEach((taskId) => expectedSet.add(taskId));
    }
    expect(chain.impact.items.map((task) => task.id), `chain ${chain.id}`).toEqual(taskIds.filter((id) => expectedSet.has(id)));
    if (expectComplete) expect(chain.impact.count, `chain ${chain.id}`).toBe(expectedSet.size);
    else expect(chain.impact.count, `chain ${chain.id}`).toBeNull();
  }
  return result;
}

describe('independent graph oracle', () => {
  it('matches naive reachability for 1200 waits/chains on generated cycles and diamonds', () => {
    const graph = generatedGraph(75, 20, 1200);
    const result = expectOracleMatch(makeInput({
      tasks: complete(graph.tasks), dependencies: complete(graph.dependencies),
    }), true);
    expect(result.waits).toHaveLength(1200);
    expect(result.chains).toHaveLength(1200);
    expect(graph.dependencies.length).toBeGreaterThan(4000);
  }, 15_000);

  it('keeps the same confirmed details with null totals when the generated graph read is truncated', () => {
    const graph = generatedGraph(5, 20, 100);
    const result = expectOracleMatch(makeInput({
      tasks: complete(graph.tasks),
      dependencies: { coverage: 'partial', items: graph.dependencies, reason: 'truncated' },
    }), false);
    expect(result.waits).toHaveLength(100);
    expect(result.coverage.sections.chains.complete).toBe(false);
  }, 10_000);
});
