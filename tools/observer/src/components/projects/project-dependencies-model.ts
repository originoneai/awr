import {
  deriveOverview,
  OVERVIEW_PARTS,
  type ChainConnector,
  type ChainNode,
  type CountDetails,
  type DerivedChain,
  type DerivedTask,
  type DerivedWait,
  type OverviewDerivation,
  type OverviewPart,
  type OverviewReviewRound,
} from '../../core/overview';
import { readsAsName } from '../../core/observation/owner';
import type {
  DependencyListData,
  DependencyObservation,
  DependencyVersion,
  PlaneFreshness,
  TeamFreshness,
  TeamOverviewResponse,
} from '../../lib/types';
import type { ProjectListModel } from './project-list-model';

export type DependencyRowStatus =
  | 'changed'
  | 'none'
  | 'ok'
  | 'unadopted'
  | 'restricted'
  | 'unknown';

export type DependencyAdoptedCell =
  | { kind: 'version'; version: number }
  | { kind: 'notYet' }
  | { kind: 'restricted' }
  | { kind: 'adoptionUnavailable' };

export type DependencyAvailableCell =
  | { kind: 'version'; version: number }
  | { kind: 'inReviewVersion'; version: number }
  | { kind: 'inReview' }
  | { kind: 'stalledAnswer' }
  | { kind: 'stalled' }
  | { kind: 'waitingUpstream' }
  | { kind: 'cannotStart' }
  | { kind: 'waiting' }
  | { kind: 'running' }
  | { kind: 'returned' }
  | { kind: 'claimed' }
  | { kind: 'notYet' }
  | { kind: 'restricted' };

export type DependencyRequirementKind =
  | { kind: 'recorded'; text: string }
  | { kind: 'pinned' }
  | { kind: 'reviewed' };

export interface ProjectDependencyRow {
  key: string;
  consumerWorkId: string;
  consumerTitle: string;
  providerWorkId: string;
  providerTitle: string | null;
  requirement: DependencyRequirementKind;
  adopted: DependencyAdoptedCell;
  available: DependencyAvailableCell;
  status: DependencyRowStatus;
  highlighted: boolean;
}

export interface ProjectBlockerItem {
  key: string;
  kind: 'review' | 'decision' | 'external';
  lamp: 'blocked' | 'stalled';
  /** Explicit reviewer or responder person when identified by the read. */
  personId: string | null;
  personName: string | null;
  externalParty: string | null;
  /** Recorded question/reason (raw text; sanitized by presentation layer). */
  what: string | null;
  /** True when the responder owns the task (answer vs decision phrasing). */
  isOwnTaskAnswer: boolean;
  queuedCount: number;
  oldestDays: number | null;
  behind: CountDetails<DerivedTask>;
  /** Navigation destination: person for review queues with known reviewer, else task. */
  destination: { kind: 'person'; id: string } | { kind: 'task'; id: string } | null;
}

export interface ProjectChainDisplay {
  chain: DerivedChain;
  /** Representative horizontal path (root + up to 2 tasks + commitment) that fits cleanly without dropping impact facts. */
  displayNodes: ChainNode[];
  displayConnectors: ChainConnector[];
  /** Number of tasks in this chain's full impact set; null when incomplete. */
  impact: CountDetails<DerivedTask>;
  /** For project/week commitment end nodes, verified / total progress when known. */
  commitmentProgress: { done: number; total: number } | null;
}

export interface ProjectDependenciesModel {
  derived: OverviewDerivation | null;
  chains: ProjectChainDisplay[];
  chainsCount: number | null;
  rows: ProjectDependencyRow[];
  dependenciesTotal: number | null;
  dependenciesTruncated: boolean;
  blockers: ProjectBlockerItem[];
  blockersCount: number | null;
  blockersComplete: boolean;
  tasksById: ReadonlyMap<string, DerivedTask>;
  coverage: {
    modelPartialParts: OverviewPart[];
    modelUnreadParts: OverviewPart[];
    orderingIncomparable: boolean;
    hasUnknownFacts: boolean;
  };
}

function cleanText(value: string | null | undefined): string | null {
  return typeof value === 'string' && value.trim().length > 0 ? value.trim() : null;
}

function cleanName(value: string | null | undefined): string | null {
  const trimmed = cleanText(value);
  return trimmed && readsAsName(trimmed) ? trimmed : null;
}

function versionKey(version: DependencyVersion | null | undefined): string | null {
  if (!version) return null;
  const receipt = cleanText(version.receipt_id);
  const hash = cleanText(version.contract_hash);
  if (!receipt && !hash) return null;
  return receipt ? `r:${receipt}|h:${hash ?? ''}` : `h:${hash}`;
}

function sameVersion(left: DependencyVersion | null | undefined, right: DependencyVersion | null | undefined): boolean {
  const lk = versionKey(left);
  const rk = versionKey(right);
  return lk !== null && lk === rk;
}

/**
 * Assign deterministic 1-based delivery numbers ("第 N 版" / "vN") per upstream provider.
 * Receipts and hashes are used ONLY as opaque ordering keys and are never exposed in UI text.
 */
export function buildDeliveryVersionMap(
  dependencies: readonly DependencyObservation[],
  reviewRounds: readonly OverviewReviewRound[],
): {
  versionOf: (providerWorkId: string, version: DependencyVersion | null | undefined) => number | null;
  latestRoundNumberOf: (providerWorkId: string) => number | null;
} {
  const roundsByProvider = new Map<string, OverviewReviewRound[]>();
  for (const round of reviewRounds) {
    const taskId = cleanText(round.taskId);
    if (!taskId) continue;
    const list = roundsByProvider.get(taskId) ?? [];
    list.push(round);
    roundsByProvider.set(taskId, list);
  }
  for (const list of roundsByProvider.values()) {
    list.sort((a, b) => (a.roundIndex ?? Number.MAX_SAFE_INTEGER) - (b.roundIndex ?? Number.MAX_SAFE_INTEGER)
      || (a.openedAtUnixMs ?? Number.MAX_SAFE_INTEGER) - (b.openedAtUnixMs ?? Number.MAX_SAFE_INTEGER)
      || (a.id ?? '').localeCompare(b.id ?? ''));
  }

  // Per provider, collect distinct delivery keys in earliest-to-latest order:
  // consumed -> adopted -> available -> required (matching temporal progression).
  const orderedKeysByProvider = new Map<string, string[]>();
  const recordKey = (providerId: string, version: DependencyVersion | null | undefined) => {
    const key = versionKey(version);
    if (!key) return;
    const list = orderedKeysByProvider.get(providerId) ?? [];
    if (!list.includes(key)) list.push(key);
    orderedKeysByProvider.set(providerId, list);
  };

  // First pass: older versions (consumed, adopted) across all edges of this provider.
  for (const item of dependencies) {
    recordKey(item.provider_work_id, item.consumed_version);
    recordKey(item.provider_work_id, item.adopted_version);
  }
  // Second pass: currently available / required versions (newer or equal).
  for (const item of dependencies) {
    recordKey(item.provider_work_id, item.available_version);
    if (item.required_version?.receipt_id) {
      recordKey(item.provider_work_id, item.required_version);
    }
  }

  const numberByProvider = new Map<string, Map<string, number>>();
  const latestRoundByProvider = new Map<string, number>();

  for (const [providerId, rounds] of roundsByProvider) {
    const maxRoundIndex = rounds.reduce((max, round) => (
      typeof round.roundIndex === 'number' && Number.isSafeInteger(round.roundIndex) && round.roundIndex > max
        ? round.roundIndex
        : max
    ), 0);
    const total = Math.max(maxRoundIndex, rounds.length);
    if (total > 0) latestRoundByProvider.set(providerId, total);
  }

  for (const [providerId, keys] of orderedKeysByProvider) {
    const map = new Map<string, number>();
    const rounds = roundsByProvider.get(providerId) ?? [];
    // If a provider has multiple review rounds (e.g. round 1 rejected, round 2 approved) and
    // a single observed delivery receipt, align that delivery to the approved round's sequence.
    const approvedRounds = rounds.filter((r) => r.state === 'approved');
    const latestApproved = approvedRounds.at(-1);
    const baseOffset = keys.length === 1 && latestApproved && typeof latestApproved.roundIndex === 'number' && latestApproved.roundIndex >= 1
      ? latestApproved.roundIndex - 1
      : 0;
    keys.forEach((key, index) => {
      map.set(key, index + 1 + baseOffset);
    });
    numberByProvider.set(providerId, map);
  }

  return {
    versionOf(providerWorkId, version) {
      const key = versionKey(version);
      if (!key) return null;
      return numberByProvider.get(providerWorkId)?.get(key) ?? null;
    },
    latestRoundNumberOf(providerWorkId) {
      return latestRoundByProvider.get(providerWorkId) ?? null;
    },
  };
}

/**
 * Select a clean linear path from a DerivedChain for horizontal diagram display
 * (root -> up to 2 tasks along the deepest downstream dependency chain -> primary commitment).
 * The chain's full impact list stays intact on ProjectChainDisplay.impact.
 */
export function selectChainDisplayNodes(
  chain: DerivedChain,
  tasksById: ReadonlyMap<string, DerivedTask>,
): { displayNodes: ChainNode[]; displayConnectors: ChainConnector[] } {
  const rootNode = chain.nodes.find((node) => node.root);
  if (!rootNode) return { displayNodes: chain.nodes, displayConnectors: chain.connectors };

  const taskNodesById = new Map<string, ChainNode>();
  for (const node of chain.nodes) {
    if (node.kind === 'task' && node.task?.id) {
      taskNodesById.set(node.task.id, node);
    }
  }

  // Build directed adjacency within this chain's task nodes from connectors.
  const outgoing = new Map<string, string[]>();
  for (const conn of chain.connectors) {
    if (conn.kind !== 'depends') continue;
    const fromNode = chain.nodes.find((n) => n.id === conn.fromNodeId);
    const toNode = chain.nodes.find((n) => n.id === conn.toNodeId);
    if (!fromNode?.task?.id || !toNode?.task?.id) continue;
    const list = outgoing.get(fromNode.task.id) ?? [];
    if (!list.includes(toNode.task.id)) list.push(toNode.task.id);
    outgoing.set(fromNode.task.id, list);
  }

  // Candidate starting tasks: those directly queued at the root, preferring highest impact then longest wait.
  const startCandidates = [...chain.root.queue.items]
    .sort((a, b) => (b.behind.count ?? -1) - (a.behind.count ?? -1)
      || (b.durationDays ?? -1) - (a.durationDays ?? -1))
    .map((wait) => wait.task.id)
    .filter((id): id is string => id !== null && taskNodesById.has(id));

  // Walk the deepest cycle-safe simple path from each candidate start task.
  let bestPath: string[] = [];
  const walk = (current: string, path: string[], visited: Set<string>) => {
    if (path.length > bestPath.length) bestPath = [...path];
    if (path.length >= 4) return; // cap depth for cycle-safety & rendering budget
    for (const next of outgoing.get(current) ?? []) {
      if (visited.has(next) || !taskNodesById.has(next)) continue;
      visited.add(next);
      path.push(next);
      walk(next, path, visited);
      path.pop();
      visited.delete(next);
    }
  };
  for (const startId of startCandidates) {
    walk(startId, [startId], new Set([startId]));
  }
  if (bestPath.length === 0) {
    const firstTaskNode = chain.nodes.find((node) => node.kind === 'task' && node.task?.id);
    if (firstTaskNode?.task?.id) bestPath = [firstTaskNode.task.id];
  }

  // Choose one primary commitment node (prefer week commitment -> goal -> project).
  const commitmentNodes = chain.nodes.filter((node) => node.kind === 'commitment' && node.commitment);
  const weekCommitment = commitmentNodes.find((node) => node.commitment?.id?.includes('-week-'));
  const goalCommitment = commitmentNodes.find((node) => node.commitment?.kind === 'goal');
  const projectCommitment = commitmentNodes.find((node) => node.commitment?.kind === 'project');
  const endNode = weekCommitment ?? goalCommitment ?? projectCommitment ?? null;

  // Keep at most 2 task nodes when an end commitment exists (4 cards total, matching prototype),
  // or up to 3 task nodes when there is no end commitment.
  const maxTasks = endNode ? 2 : 3;
  const chosenTaskIds = bestPath.slice(0, maxTasks);

  const displayNodes: ChainNode[] = [rootNode];
  const displayConnectors: ChainConnector[] = [];
  let prevId = rootNode.id;

  chosenTaskIds.forEach((taskId, index) => {
    const node = taskNodesById.get(taskId);
    if (!node) return;
    displayNodes.push(node);
    displayConnectors.push({
      fromNodeId: prevId,
      toNodeId: node.id,
      kind: index === 0 ? 'blocks' : 'depends',
    });
    prevId = node.id;
  });

  if (endNode) {
    displayNodes.push(endNode);
    displayConnectors.push({
      fromNodeId: prevId,
      toNodeId: endNode.id,
      kind: 'affects',
    });
  }

  // Ensure tasksById is acknowledged for callers that inspect node metadata.
  void tasksById;
  return { displayNodes, displayConnectors };
}

function compareNullableDescending(left: number | null, right: number | null): number {
  if (left === null && right === null) return 0;
  if (left === null) return 1;
  if (right === null) return -1;
  return right - left;
}

/**
 * Derive the prototype's right-column "卡点" blocker list from derived waits.
 * Excludes dependency waits (those are downstream consequences of an upstream root),
 * groups review waits by explicit reviewer ID, and sorts descending by behind count.
 */
export function deriveProjectBlockers(
  derived: OverviewDerivation | null,
  orderingIncomparable: boolean,
): { items: ProjectBlockerItem[]; count: number | null; complete: boolean } {
  if (!derived) {
    return { items: [], count: null, complete: false };
  }

  const out: ProjectBlockerItem[] = [];
  const reviewGroups = new Map<string, DerivedWait[]>();
  const anonymousReviewWaits: DerivedWait[] = [];

  for (const wait of derived.waits) {
    if (wait.kind === 'dependency') continue;
    if (wait.kind === 'review') {
      const explicitReviewer = !wait.waitingOnUnknown && wait.waitingOn.length === 1 ? wait.waitingOn[0] : null;
      if (explicitReviewer) {
        const list = reviewGroups.get(explicitReviewer.id) ?? [];
        list.push(wait);
        reviewGroups.set(explicitReviewer.id, list);
      } else {
        anonymousReviewWaits.push(wait);
      }
      continue;
    }

    const responder = !wait.waitingOnUnknown && wait.waitingOn.length >= 1 ? wait.waitingOn[0] : null;
    const isOwnTaskAnswer = wait.kind === 'decision'
      && responder !== null
      && wait.task.owner?.id !== null
      && wait.task.owner?.id === responder.id;
    const behind: CountDetails<DerivedTask> = orderingIncomparable
      ? { count: null, items: wait.behind.items, unknownItems: [...wait.behind.unknownItems, { part: 'tasks', reason: 'ordering-incomparable' }] }
      : wait.behind;

    out.push({
      key: `${wait.kind}:${wait.task.id ?? 'unknown'}:${wait.sourceId ?? out.length}`,
      kind: wait.kind,
      lamp: 'stalled',
      personId: responder?.id ?? null,
      personName: cleanName(responder?.displayName),
      externalParty: cleanText(wait.externalParty),
      what: cleanText(wait.what),
      isOwnTaskAnswer,
      queuedCount: 1,
      oldestDays: wait.durationDays,
      behind,
      destination: wait.task.id ? { kind: 'task', id: wait.task.id } : null,
    });
  }

  const buildReviewBlocker = (key: string, waits: DerivedWait[], personId: string | null, personName: string | null): ProjectBlockerItem => {
    const seen = new Set<string>();
    const impactTasks: DerivedTask[] = [];
    let anyUnknownBehind = orderingIncomparable;
    const unknownItems = [];
    for (const wait of waits) {
      if (wait.behind.count === null) anyUnknownBehind = true;
      unknownItems.push(...wait.behind.unknownItems);
      for (const task of wait.behind.items) {
        const tid = task.id ?? `${impactTasks.length}`;
        if (seen.has(tid)) continue;
        seen.add(tid);
        impactTasks.push(task);
      }
    }
    const oldestDays = waits.some((w) => w.durationDays === null)
      ? null
      : Math.max(...waits.map((w) => w.durationDays!));
    const firstTaskId = waits[0]?.task.id ?? null;
    return {
      key,
      kind: 'review',
      lamp: 'blocked',
      personId,
      personName,
      externalParty: null,
      what: null,
      isOwnTaskAnswer: false,
      queuedCount: waits.length,
      oldestDays,
      behind: {
        count: anyUnknownBehind ? null : impactTasks.length,
        items: impactTasks,
        unknownItems,
      },
      destination: personId
        ? { kind: 'person', id: personId }
        : firstTaskId
          ? { kind: 'task', id: firstTaskId }
          : null,
    };
  };

  for (const [personId, waits] of reviewGroups) {
    const name = cleanName(waits[0]?.waitingOn[0]?.displayName);
    out.push(buildReviewBlocker(`review:${personId}`, waits, personId, name));
  }
  anonymousReviewWaits.forEach((wait, index) => {
    out.push(buildReviewBlocker(`review:unknown:${wait.task.id ?? index}`, [wait], null, null));
  });

  const sorted = out
    .map((item, index) => ({ item, index }))
    .sort((a, b) => compareNullableDescending(a.item.behind.count, b.item.behind.count)
      || compareNullableDescending(a.item.oldestDays, b.item.oldestDays)
      || a.index - b.index)
    .map((entry) => entry.item);

  const waitPartsComplete = !orderingIncomparable
    && derived.waitCounts.byKind.review.count !== null
    && derived.waitCounts.byKind.decision.count !== null
    && derived.waitCounts.byKind.external.count !== null;

  return {
    items: sorted,
    count: waitPartsComplete ? sorted.length : null,
    complete: waitPartsComplete,
  };
}

function deriveAvailableCell(
  item: DependencyObservation,
  availableVersionNum: number | null,
  providerTask: DerivedTask | undefined,
  providerWaits: readonly DerivedWait[],
  latestRoundNum: number | null,
): DependencyAvailableCell {
  if (item.available_version && availableVersionNum !== null) {
    return { kind: 'version', version: availableVersionNum };
  }
  if (item.missing?.available_version === 'permission_restricted') {
    return { kind: 'restricted' };
  }
  if (!providerTask) return { kind: 'notYet' };

  switch (providerTask.status) {
    case 'review':
      return latestRoundNum !== null
        ? { kind: 'inReviewVersion', version: latestRoundNum }
        : { kind: 'inReview' };
    case 'stalled':
      return providerWaits.some((wait) => wait.kind === 'decision')
        ? { kind: 'stalledAnswer' }
        : { kind: 'stalled' };
    case 'waiting':
      if (providerWaits.some((wait) => wait.kind === 'external')) return { kind: 'cannotStart' };
      if (providerWaits.some((wait) => wait.kind === 'dependency')) return { kind: 'waitingUpstream' };
      return { kind: 'waiting' };
    case 'running':
      return { kind: 'running' };
    case 'returned':
      return { kind: 'returned' };
    case 'claimed':
      return { kind: 'claimed' };
    default:
      return { kind: 'notYet' };
  }
}

function deriveAdoptedCell(
  item: DependencyObservation,
  adoptedVersionNum: number | null,
): DependencyAdoptedCell {
  if (item.adopted_version && adoptedVersionNum !== null) {
    return { kind: 'version', version: adoptedVersionNum };
  }
  if (item.missing?.adopted_version === 'permission_restricted') {
    return { kind: 'restricted' };
  }
  if (item.missing?.adopted_version === 'adoption_unavailable') {
    return { kind: 'adoptionUnavailable' };
  }
  return { kind: 'notYet' };
}

function deriveRowStatus(item: DependencyObservation): DependencyRowStatus {
  if (item.reason === 'permission_restricted' || item.missing?.required_version === 'permission_restricted') {
    return 'restricted';
  }
  if (item.validity === 'upstream_changed') return 'changed';
  if (item.validity === 'unavailable') return 'none';
  if (item.validity === 'valid') {
    if (item.adopted_version === null) {
      if (item.available_version !== null) return 'unadopted';
      return 'none';
    }
    if (item.available_version !== null && !sameVersion(item.adopted_version, item.available_version) && item.version_policy !== 'fixed_delivery') {
      return 'changed';
    }
    return 'ok';
  }
  return 'unknown';
}

export function deriveDependencyRows(
  dependencies: DependencyListData | undefined,
  model: ProjectListModel | undefined,
  derived: OverviewDerivation | null,
): ProjectDependencyRow[] {
  const items = dependencies?.items ?? [];
  if (items.length === 0) return [];

  const reviewRounds = model?.input?.reviewRounds?.coverage === 'unread'
    ? []
    : (model?.input?.reviewRounds?.items ?? []);
  const modelDeps = model?.input?.dependencies?.coverage === 'unread'
    ? []
    : (model?.input?.dependencies?.items ?? []);

  const depRequirementByPair = new Map<string, string>();
  for (const dep of modelDeps) {
    const consumer = cleanText(dep.consumerTaskId);
    const provider = cleanText(dep.providerTaskId);
    const what = cleanText(dep.what);
    if (consumer && provider && what) {
      depRequirementByPair.set(`${consumer}\u0000${provider}`, what);
    }
  }

  const tasksById = new Map<string, DerivedTask>();
  for (const task of derived?.tasks ?? []) {
    if (task.id) tasksById.set(task.id, task);
  }
  const waitsByTaskId = new Map<string, DerivedWait[]>();
  for (const wait of derived?.waits ?? []) {
    if (!wait.task.id) continue;
    const list = waitsByTaskId.get(wait.task.id) ?? [];
    list.push(wait);
    waitsByTaskId.set(wait.task.id, list);
  }

  const versions = buildDeliveryVersionMap(items, reviewRounds);

  return items.map((item) => {
    const pairKey = `${item.work_id}\u0000${item.provider_work_id}`;
    const recordedReq = depRequirementByPair.get(pairKey);
    const requirement: DependencyRequirementKind = recordedReq
      ? { kind: 'recorded', text: recordedReq }
      : item.version_policy === 'fixed_delivery'
        ? { kind: 'pinned' }
        : { kind: 'reviewed' };

    const adoptedNum = versions.versionOf(item.provider_work_id, item.adopted_version);
    const availableNum = versions.versionOf(item.provider_work_id, item.available_version);
    const latestRoundNum = versions.latestRoundNumberOf(item.provider_work_id);
    const providerTask = tasksById.get(item.provider_work_id);
    const providerWaits = waitsByTaskId.get(item.provider_work_id) ?? [];
    const status = deriveRowStatus(item);

    return {
      key: `${item.work_id}::${item.provider_work_id}::${item.dependency_position}`,
      consumerWorkId: item.work_id,
      consumerTitle: item.work_title,
      providerWorkId: item.provider_work_id,
      providerTitle: item.provider_title,
      requirement,
      adopted: deriveAdoptedCell(item, adoptedNum),
      available: deriveAvailableCell(item, availableNum, providerTask, providerWaits, latestRoundNum),
      status,
      highlighted: status === 'changed',
    };
  });
}

const PLANE_KEYS = ['report_received', 'authoritative', 'page_read'] as const;

export function mergeDependenciesFreshness(
  ...sources: Array<TeamFreshness | undefined>
): TeamFreshness | undefined {
  const present = sources.filter((item): item is TeamFreshness => Boolean(item));
  if (present.length === 0) return undefined;
  if (present.length === 1) return present[0];
  const plane = (key: (typeof PLANE_KEYS)[number]): PlaneFreshness => {
    const planes = present.map((source) => source[key]);
    const status = planes.some((entry) => entry?.status === 'failed') ? 'failed'
      : planes.some((entry) => entry?.status === 'stale') ? 'stale'
        : planes.some((entry) => entry?.status === 'measured') ? 'measured'
          : 'not_measured';
    const times = planes
      .map((entry) => entry?.at_unix_ms)
      .filter((value): value is number => typeof value === 'number' && Number.isFinite(value));
    return { status, ...(times.length > 0 ? { at_unix_ms: Math.max(...times) } : {}) };
  };
  return {
    report_received: plane('report_received'),
    authoritative: plane('authoritative'),
    page_read: plane('page_read'),
  };
}

export function deriveProjectDependencies({
  projectKey,
  overview,
  model,
  dependencies,
}: {
  projectKey: string;
  overview?: TeamOverviewResponse;
  model?: ProjectListModel;
  dependencies?: DependencyListData;
}): ProjectDependenciesModel {
  const usableModel = model?.project === projectKey && model.input && model.input.options ? model : undefined;
  const orderingIncomparable = usableModel?.ordering === 'incomparable';

  let derived: OverviewDerivation | null = null;
  if (usableModel) {
    try {
      derived = deriveOverview(usableModel.input);
    } catch {
      derived = null;
    }
  }

  const tasksById = new Map<string, DerivedTask>();
  for (const task of derived?.tasks ?? []) {
    if (task.id) tasksById.set(task.id, task);
  }

  // Compute week-commitment verified progress when tasks are complete.
  const weekCommitmentTasks = new Map<string, { done: number; total: number }>();
  if (derived && !orderingIncomparable && usableModel?.input.tasks.coverage === 'complete') {
    for (const task of derived.tasks) {
      if (!task.commitments) continue;
      for (const commitment of task.commitments) {
        if (!commitment.id || !commitment.id.includes('-week-')) continue;
        const entry = weekCommitmentTasks.get(commitment.id) ?? { done: 0, total: 0 };
        entry.total += 1;
        if (task.status === 'verified') entry.done += 1;
        weekCommitmentTasks.set(commitment.id, entry);
      }
    }
  }

  const chains: ProjectChainDisplay[] = (derived?.chains ?? []).map((chain) => {
    const { displayNodes, displayConnectors } = selectChainDisplayNodes(chain, tasksById);
    const endNode = displayNodes.at(-1);
    const commitmentProgress = endNode?.kind === 'commitment' && endNode.commitment?.id
      ? (weekCommitmentTasks.get(endNode.commitment.id) ?? null)
      : null;
    const impact: CountDetails<DerivedTask> = orderingIncomparable
      ? { count: null, items: chain.impact.items, unknownItems: [...chain.impact.unknownItems, { part: 'tasks', reason: 'ordering-incomparable' }] }
      : chain.impact;
    return {
      chain,
      displayNodes,
      displayConnectors,
      impact,
      commitmentProgress,
    };
  });

  const chainsCount = derived && !orderingIncomparable && derived.chainCounts.all.count !== null
    ? derived.chainCounts.all.count
    : null;

  const rows = deriveDependencyRows(dependencies, usableModel, derived);
  const dependenciesTotal = dependencies ? dependencies.total : null;
  const dependenciesTruncated = Boolean(dependencies?.next_cursor);

  const blockersResult = deriveProjectBlockers(derived, orderingIncomparable);

  const trackedParts = OVERVIEW_PARTS.filter((part) => part !== 'approvals');
  const modelPartialParts = usableModel
    ? trackedParts.filter((part) => usableModel.coverage?.[part]?.status === 'partial' || usableModel.input[part]?.coverage === 'partial')
    : [];
  const modelUnreadParts = usableModel
    ? trackedParts.filter((part) => usableModel.coverage?.[part]?.status === 'unread' || usableModel.input[part]?.coverage === 'unread')
    : [];

  const hasUnknownFacts = orderingIncomparable
    || rows.some((row) => row.status === 'unknown')
    || blockersResult.items.some((blocker) => blocker.behind.count === null)
    || (derived !== null && chainsCount === null)
    || usableModel?.freshness.authoritative.status === 'not_measured'
    || Boolean(overview && overview.coverage.failed > 0);

  return {
    derived,
    chains,
    chainsCount,
    rows,
    dependenciesTotal,
    dependenciesTruncated,
    blockers: blockersResult.items,
    blockersCount: blockersResult.count,
    blockersComplete: blockersResult.complete,
    tasksById,
    coverage: {
      modelPartialParts,
      modelUnreadParts,
      orderingIncomparable,
      hasUnknownFacts,
    },
  };
}
