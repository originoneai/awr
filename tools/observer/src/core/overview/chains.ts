import type {
  ChainConnector,
  ChainNode,
  CoverageIssue,
  DerivedChain,
  DerivedTask,
  DerivedWait,
  OverviewCommitment,
  OverviewInput,
  OverviewStatusWaitingDerivation,
  PersonRef,
} from './types';
import { details, knownDetails, makeIssue, readItems, uniqueIssues, unknownDetails, validId } from './shared';

interface RootGroup {
  key: string;
  kind: 'decision' | 'review' | 'external';
  waits: DerivedWait[];
  person: PersonRef | null;
}

function compareNullableDescending(left: number | null, right: number | null): number {
  if (left == null && right == null) return 0;
  if (left == null) return 1;
  if (right == null) return -1;
  return right - left;
}

/** Chain reachability is iterative and independent from the first slice's impact implementation. */
export function deriveChains(input: OverviewInput, base: OverviewStatusWaitingDerivation) {
  const taskIndexById = new Map<string, number>();
  base.tasks.forEach((task, index) => {
    if (task.id != null && !taskIndexById.has(task.id)) taskIndexById.set(task.id, index);
  });
  const adjacency: number[][] = base.tasks.map(() => []);
  const graphEdges: Array<{ provider: string; consumer: string }> = [];
  const edgeKeys = new Set<string>();
  for (const edge of readItems(input.dependencies)) {
    const provider = validId(edge.providerTaskId);
    const consumer = validId(edge.consumerTaskId);
    if (provider == null || consumer == null || !taskIndexById.has(provider) || !taskIndexById.has(consumer)) continue;
    const key = `${provider}\u0000${consumer}`;
    if (edgeKeys.has(key)) continue;
    edgeKeys.add(key);
    adjacency[taskIndexById.get(provider)!].push(taskIndexById.get(consumer)!);
    graphEdges.push({ provider, consumer });
  }

  const reachCache = new Map<string, DerivedTask[]>();
  function reach(taskId: string): DerivedTask[] {
    const cached = reachCache.get(taskId);
    if (cached) return cached;
    const root = taskIndexById.get(taskId);
    if (root == null) return [];
    const seen = new Set<number>([root]);
    const stack = [root];
    while (stack.length > 0) {
      const current = stack.pop()!;
      for (const next of adjacency[current]) {
        if (seen.has(next)) continue;
        seen.add(next);
        stack.push(next);
      }
    }
    const items = base.tasks.filter((_task, index) => seen.has(index));
    reachCache.set(taskId, items);
    return items;
  }

  const groups: RootGroup[] = [];
  const groupByKey = new Map<string, RootGroup>();
  base.waits.forEach((wait, index) => {
    if (wait.kind === 'dependency') return;
    const explicitPerson = wait.waitingOn.length === 1 ? wait.waitingOn[0] : null;
    const key = wait.kind === 'review' && explicitPerson != null
      ? `review:${explicitPerson.id}`
      : `${wait.kind}:${wait.task.id ?? 'unknown'}:${wait.sourceId ?? index}`;
    const existing = groupByKey.get(key);
    if (existing) {
      existing.waits.push(wait);
      return;
    }
    const group: RootGroup = { key, kind: wait.kind, waits: [wait], person: explicitPerson };
    groupByKey.set(key, group);
    groups.push(group);
  });

  const coverageIssues: CoverageIssue[] = [];
  const chains = groups.map((group): DerivedChain => {
    const affectedIds = new Set<string>();
    for (const wait of group.waits) {
      if (wait.task.id == null) continue;
      for (const task of reach(wait.task.id)) if (task.id != null) affectedIds.add(task.id);
    }
    const impactItems = base.tasks.filter((task) => task.id != null && affectedIds.has(task.id));
    const impactIssues = uniqueIssues(group.waits.flatMap((wait) => wait.behind.unknownItems));
    const impact = details(impactItems, impactIssues);
    const rootNodeId = `${group.key}:root`;
    const nodes: ChainNode[] = [{
      id: rootNodeId, kind: 'root', root: true, tone: 'negative', task: null, commitment: null,
    }];
    const connectors: ChainConnector[] = [];
    const connectorKeys = new Set<string>();
    const connect = (fromNodeId: string, toNodeId: string, kind: ChainConnector['kind']) => {
      const key = `${fromNodeId}\u0000${toNodeId}\u0000${kind}`;
      if (connectorKeys.has(key)) return;
      connectorKeys.add(key);
      connectors.push({ fromNodeId, toNodeId, kind });
    };
    for (const wait of group.waits) {
      if (wait.task.id != null) connect(rootNodeId, `${group.key}:task:${wait.task.id}`, 'blocks');
    }
    for (const task of impactItems) {
      nodes.push({
        id: `${group.key}:task:${task.id}`, kind: 'task', root: false, tone: null, task, commitment: null,
      });
    }
    for (const edge of graphEdges) {
      if (affectedIds.has(edge.provider) && affectedIds.has(edge.consumer)) {
        connect(`${group.key}:task:${edge.provider}`, `${group.key}:task:${edge.consumer}`, 'depends');
      }
    }

    const commitmentItems: OverviewCommitment[] = [];
    const commitmentIds = new Set<string>();
    const commitmentIssues: CoverageIssue[] = [];
    for (const task of impactItems) {
      if (task.commitments == null) {
        commitmentIssues.push(makeIssue('tasks', 'chain-commitments-unknown', task.id));
        continue;
      }
      for (const commitment of task.commitments) {
        const commitmentId = validId(commitment.id);
        if (commitmentId == null) {
          commitmentIssues.push(makeIssue('tasks', 'chain-commitment-id-unknown', task.id));
          continue;
        }
        if (!commitmentIds.has(commitmentId)) {
          commitmentIds.add(commitmentId);
          commitmentItems.push(commitment);
          nodes.push({
            id: `${group.key}:commitment:${commitmentId}`, kind: 'commitment', root: false, tone: null,
            task: null, commitment,
          });
        }
        connect(`${group.key}:task:${task.id}`, `${group.key}:commitment:${commitmentId}`, 'affects');
      }
    }
    const commitments = details(commitmentItems, [...base.taskCounts.all.unknownItems, ...commitmentIssues]);
    const queueSource = base.waitCounts.byKind[group.kind];
    const queue = queueSource.count == null
      ? unknownDetails(group.waits, queueSource.unknownItems)
      : knownDetails(group.waits);
    const oldestDays = group.waits.some((wait) => wait.durationDays == null)
      ? null
      : Math.max(...group.waits.map((wait) => wait.durationDays!));
    const externalParties = new Set(group.waits.map((wait) => wait.externalParty));
    const externalParty = externalParties.size === 1 ? group.waits[0].externalParty : null;
    coverageIssues.push(...impactIssues, ...commitmentIssues, ...queue.unknownItems);
    return {
      id: group.key,
      root: { kind: group.kind, person: group.person, externalParty, queue, oldestDays, tone: 'negative' },
      nodes,
      connectors,
      impact,
      commitments,
      footer: { impact, commitments },
    };
  });

  chains.sort((left, right) => compareNullableDescending(left.impact.count, right.impact.count)
    || compareNullableDescending(left.root.oldestDays, right.root.oldestDays));
  const chainIssues = uniqueIssues(['decision', 'review', 'external'].flatMap((kind) =>
    base.waitCounts.byKind[kind as 'decision' | 'review' | 'external'].unknownItems));
  return {
    chains,
    chainCounts: { all: details(chains, chainIssues) },
    coverageIssues: uniqueIssues([...coverageIssues, ...chainIssues]),
  };
}
