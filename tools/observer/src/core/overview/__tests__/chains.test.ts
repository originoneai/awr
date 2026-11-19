import { describe, expect, it } from 'vitest';
import { deriveOverview } from '../index';
import type { OverviewTask } from '../index';
import { DAY, NOW, baseTask, complete, decision, dependency, makeInput, reviewRound, startedTask } from './factories';

function committedTask(id: string, commitments: OverviewTask['commitments'], overrides = {}) {
  return startedTask(id, { commitments, ...overrides });
}

describe('blocker chains', () => {
  it('connects a blocker through transitive tasks to affected commitments and shares footer details', () => {
    const result = deriveOverview(makeInput({
      tasks: complete([
        committedTask('provider', [{ id: 'project-commitment', title: 'Project promise', kind: 'project' }], {
          submittedResultAtUnixMs: NOW - DAY,
        }),
        committedTask('consumer', [{ id: 'project-commitment', title: 'Project promise', kind: 'project' }]),
        committedTask('tail', [{ id: 'goal-commitment', title: 'Goal promise', kind: 'goal' }]),
        baseTask('outside', {
          commitments: [], externalWait: { party: 'Outside Example', sinceUnixMs: NOW - 5 * DAY },
        }),
        baseTask('decider', { commitments: [] }),
      ]),
      dependencies: complete([dependency('consumer', 'provider'), dependency('tail', 'consumer')]),
      decisions: complete([decision('decider')]),
      reviewRounds: complete([reviewRound('provider', 'open')]),
    }));
    expect(result.chains).toHaveLength(3);
    const chain = result.chains[0];
    expect(chain.root).toMatchObject({ kind: 'review', tone: 'negative' });
    expect(chain.root.person?.id).toBe('person-b');
    expect(chain.impact.count).toBe(3);
    expect(chain.impact.items.map((task) => task.id)).toEqual(['provider', 'consumer', 'tail']);
    expect(chain.commitments.count).toBe(2);
    expect(chain.commitments.items.map((commitment) => commitment.id)).toEqual(['project-commitment', 'goal-commitment']);
    expect(chain.nodes.map((node) => node.kind)).toEqual(['root', 'task', 'task', 'task', 'commitment', 'commitment']);
    expect(chain.connectors.map((connector) => connector.kind)).toEqual([
      'blocks', 'depends', 'depends', 'affects', 'affects', 'affects',
    ]);
    expect(chain.footer.impact).toBe(chain.impact);
    expect(chain.footer.commitments).toBe(chain.commitments);
    const dependencyWait = result.waits.find((wait) => wait.kind === 'dependency')!;
    expect(dependencyWait.behind.items.map((task) => task.id)).toEqual(['consumer', 'tail']);
    for (const row of result.chains) {
      for (const value of [row.root.queue, row.impact, row.commitments]) {
        if (value.count != null) expect(value.count).toBe(value.items.length);
      }
    }
    if (result.chainCounts.all.count != null) expect(result.chainCounts.all.count).toBe(result.chainCounts.all.items.length);
  });

  it('groups review queues only by an explicit shared reviewer and unions shared downstream once', () => {
    const input = makeInput({
      tasks: complete([
        startedTask('first', { commitments: [], submittedResultAtUnixMs: NOW - DAY }),
        startedTask('second', { commitments: [], submittedResultAtUnixMs: NOW - DAY }),
        baseTask('shared', { commitments: [] }),
      ]),
      dependencies: complete([dependency('shared', 'first'), dependency('shared', 'second')]),
      reviewRounds: complete([reviewRound('first', 'open'), reviewRound('second', 'open')]),
    });
    const grouped = deriveOverview(input);
    expect(grouped.chains).toHaveLength(1);
    expect(grouped.chains[0].root.queue.count).toBe(2);
    expect(grouped.chains[0].impact.count).toBe(3);
    expect(grouped.chains[0].impact.items.map((task) => task.id)).toEqual(['first', 'second', 'shared']);

    const unknownReviewers = deriveOverview(makeInput({
      ...input,
      reviewRounds: complete([
        reviewRound('first', 'open', { reviewerPersonId: null }),
        reviewRound('second', 'open', { reviewerPersonId: null }),
      ]),
    }));
    expect(unknownReviewers.chains).toHaveLength(2);
    expect(unknownReviewers.chains.map((chain) => chain.root.person)).toEqual([null, null]);
  });

  it('handles a dependency cycle iteratively with deterministic nodes and connectors', () => {
    const input = makeInput({
      tasks: complete([
        baseTask('a', { commitments: [], externalWait: { party: 'Outside Example', sinceUnixMs: NOW - DAY } }),
        baseTask('b', { commitments: [] }),
      ]),
      dependencies: complete([dependency('b', 'a'), dependency('a', 'b')]),
    });
    const first = deriveOverview(input);
    const second = deriveOverview(input);
    expect(first.chains).toHaveLength(1);
    expect(first.chains[0].impact.count).toBe(2);
    expect(first.chains[0].nodes).toHaveLength(3);
    expect(first.chains[0].connectors.filter((connector) => connector.kind === 'depends')).toHaveLength(2);
    expect(second).toEqual(first);
  });

  it('keeps commitments unknown when an affected task did not provide them', () => {
    const result = deriveOverview(makeInput({
      tasks: complete([
        baseTask('root', {
          commitments: [{ id: 'known-promise', title: 'Known promise', kind: 'project' }],
          externalWait: { party: 'Outside Example', sinceUnixMs: NOW - DAY },
        }),
        baseTask('downstream', { commitments: undefined }),
      ]),
      dependencies: complete([dependency('downstream', 'root')]),
    }));
    expect(result.chains[0].impact.count).toBe(2);
    expect(result.chains[0].commitments.count).toBeNull();
    expect(result.chains[0].commitments.items.map((commitment) => commitment.id)).toEqual(['known-promise']);
    expect(result.coverage.sections.chains.complete).toBe(false);
  });

  it('orders equal-impact chains by the oldest wait first and then stable input order', () => {
    const result = deriveOverview(makeInput({
      tasks: complete([
        baseTask('first', { commitments: [] }),
        baseTask('second', { commitments: [] }),
        baseTask('third', { commitments: [] }),
      ]),
      decisions: complete([
        decision('first', { createdAtUnixMs: NOW - 2 * DAY }),
        decision('second', { createdAtUnixMs: NOW - 6 * DAY }),
        decision('third', { createdAtUnixMs: NOW - 6 * DAY }),
      ]),
    }));
    expect(result.chains.map((chain) => chain.impact.count)).toEqual([1, 1, 1]);
    expect(result.chains.map((chain) => chain.root.queue.items[0].task.id)).toEqual(['second', 'third', 'first']);
  });

  it('applies the oldest-wait tie-break at chain level even when the grouped waits arrive later', () => {
    // The decision wait sorts first in the wait list (it holds up four tasks versus three),
    // so the decision chain is built first; the grouped review chain is older in chain-level
    // days because its oldest member arrived later in the wait list. Equal impact must still
    // order the older chain first.
    const result = deriveOverview(makeInput({
      tasks: complete([
        baseTask('reviewed-newer', { commitments: [], started: true, agentActive: false, submittedResultAtUnixMs: NOW - 2 * DAY }),
        baseTask('reviewed-older', { commitments: [], started: true, agentActive: false, submittedResultAtUnixMs: NOW - 9 * DAY }),
        baseTask('decided', { commitments: [] }),
        baseTask('consumer-a', { commitments: [] }),
        baseTask('consumer-b', { commitments: [] }),
        baseTask('downstream-a', { commitments: [] }),
        baseTask('downstream-b', { commitments: [] }),
        baseTask('downstream-c', { commitments: [] }),
      ]),
      dependencies: complete([
        dependency('consumer-a', 'reviewed-newer', { satisfied: true }),
        dependency('consumer-b', 'reviewed-newer', { satisfied: true }),
        dependency('consumer-b', 'reviewed-older', { satisfied: true }),
        dependency('downstream-a', 'decided', { satisfied: true }),
        dependency('downstream-b', 'decided', { satisfied: true }),
        dependency('downstream-c', 'decided', { satisfied: true }),
      ]),
      decisions: complete([decision('decided', { createdAtUnixMs: NOW - 5 * DAY })]),
      reviewRounds: complete([
        reviewRound('reviewed-newer', 'open', { reviewerPersonId: 'person-b', openedAtUnixMs: NOW - 2 * DAY }),
        reviewRound('reviewed-older', 'open', { reviewerPersonId: 'person-b', openedAtUnixMs: NOW - 9 * DAY }),
      ]),
    }));
    expect(result.chains.map((chain) => chain.impact.count)).toEqual([4, 4]);
    expect(result.chains.map((chain) => [chain.root.kind, chain.root.oldestDays])).toEqual([
      ['review', 9],
      ['decision', 5],
    ]);
  });

  it('marks exactly one root node per chain with the red blocker tone', () => {
    const result = deriveOverview(makeInput({
      tasks: complete([
        baseTask('a', {
          commitments: [], externalWait: { party: 'Outside Example', sinceUnixMs: NOW - 2 * DAY },
        }),
        baseTask('b', { commitments: [] }),
      ]),
      decisions: complete([decision('b')]),
      dependencies: complete([dependency('b', 'a')]),
    }));
    expect(result.chains).toHaveLength(2);
    for (const chain of result.chains) {
      const rootNodes = chain.nodes.filter((node) => node.kind === 'root');
      expect(rootNodes).toHaveLength(1);
      expect(rootNodes[0].root).toBe(true);
      expect(rootNodes[0].tone).toBe('negative');
      for (const node of chain.nodes) {
        if (node !== rootNodes[0]) expect(node.root).toBe(false);
      }
    }
  });
});
