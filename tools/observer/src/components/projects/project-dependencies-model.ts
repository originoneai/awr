import {
  deriveOverview,
  OVERVIEW_PARTS,
  type ChainConnector,
  type ChainNode,
  type CountDetails,
  type DerivedChain,
  type DerivedTask,
  type DerivedWait,
  type OverviewCommitment,
  type OverviewDerivation,
  type OverviewPart,
} from '../../core/overview';
import { readsAsName } from '../../core/observation/owner';
import { workLabel } from '../../core/observation/labels';
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
  | { kind: 'matchesAvailable' }
  | { kind: 'differsFromAvailable' }
  | { kind: 'recorded' }
  | { kind: 'notRecorded' }
  | { kind: 'restricted' }
  | { kind: 'adoptionUnavailable' };

export type DependencyAvailableCell =
  | { kind: 'recorded' }
  | { kind: 'inReview' }
  | { kind: 'stalledAnswer' }
  | { kind: 'stalled' }
  | { kind: 'waitingUpstream' }
  | { kind: 'cannotStart' }
  | { kind: 'waiting' }
  | { kind: 'running' }
  | { kind: 'returned' }
  | { kind: 'claimed' }
  | { kind: 'notRecorded' }
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

export interface RelatedOpenDecision {
  id: string | null;
  question: string;
  responderName: string | null;
}

export interface ProjectChainDisplay {
  chain: DerivedChain;
  /** Representative horizontal path (root -> up to 2 tasks -> primary commitment). */
  displayNodes: ChainNode[];
  displayConnectors: ChainConnector[];
  /** Number of tasks in this chain's full impact set; null when incomplete. */
  impact: CountDetails<DerivedTask>;
  /** For commitment end nodes, verified / total progress across tasks bound to that commitment when known. */
  commitmentProgress: { done: number; total: number } | null;
  /** True only when complete directory duty/role records confirm this reviewer is the sole reviewer on the project. */
  soleReviewer: boolean;
  /** Open current decision explicitly tied to a task in this chain's queue, when recorded. */
  relatedOpenDecision: RelatedOpenDecision | null;
}

export interface ProjectDirectoryMemberRole {
  personId: string | null;
  available: boolean;
  duty?: string | null;
  businessRoles?: readonly string[] | null;
}

export interface ProjectDependenciesModel {
  derived: OverviewDerivation | null;
  chains: ProjectChainDisplay[];
  chainsCount: number | null;
  chainsComplete: boolean;
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

/**
 * Deterministic blocker kind priority when two blockers hold up the same number of tasks:
 * internal team blockers (review queue, then internal decision) precede external waits.
 */
export const BLOCKER_KIND_PRIORITY: Readonly<Record<ProjectBlockerItem['kind'], number>> = {
  review: 0,
  decision: 1,
  external: 2,
};

const OPAQUE_KEY_PATTERN = /^[A-Z]{1,8}-\d+$/i;
const REVIEWER_DUTY_LABELS = new Set(['复核', 'reviewer']);

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
 * Returns true when a commitment carries a recorded human-readable title rather than
 * an omitted title or an opaque record key such as "P-001" or "GOAL-001".
 */
export function hasRecordedCommitmentTitle(
  commitment: OverviewCommitment | null | undefined,
  projectKey: string,
): boolean {
  const title = cleanText(commitment?.title);
  if (!title) return false;
  if (commitment?.id && title === commitment.id) return false;
  if (title === projectKey) return false;
  if (OPAQUE_KEY_PATTERN.test(title)) return false;
  return workLabel(title, { language: 'en-US', recordedTitle: false }).kind === 'named';
}

/**
 * Determine whether project directory records establish a single sole reviewer.
 * Only returns a personId when:
 * - at least one member is present,
 * - every member is available and carries an explicit recorded duty or businessRoles list, and
 * - exactly one member holds reviewer duty/role.
 * Never guesses from past review rounds or incomplete directory data.
 */
export function findSoleReviewerPersonId(
  directoryMembers: readonly ProjectDirectoryMemberRole[] | null | undefined,
): string | null {
  if (!directoryMembers || directoryMembers.length === 0) return null;
  const reviewerIds = new Set<string>();
  for (const member of directoryMembers) {
    if (!member.available || !member.personId) return null;
    const roles = Array.isArray(member.businessRoles) && member.businessRoles.length > 0
      ? member.businessRoles.map((role) => role.trim().toLowerCase()).filter(Boolean)
      : null;
    const duty = cleanText(member.duty);
    if (!roles && !duty) return null;
    const isReviewer = (roles !== null && roles.includes('reviewer'))
      || (duty !== null && REVIEWER_DUTY_LABELS.has(duty.toLowerCase()));
    if (isReviewer) reviewerIds.add(member.personId);
  }
  if (reviewerIds.size !== 1) return null;
  return [...reviewerIds][0] ?? null;
}

/**
 * Deterministic project dependency chain selection rule (matches prototype pDeps()):
 * - Always include review-queue chains (cross-person review bottlenecks).
 * - Always include external-wait chains (blockers outside the project boundary).
 * - Include decision chains only when they hold up downstream dependent tasks (impact > 1).
 *   Standalone single-task decisions remain visible in the right-column "卡点" blocker list.
 */
export function isProjectBlockedChain(chain: DerivedChain): boolean {
  if (chain.root.kind === 'review' || chain.root.kind === 'external') return true;
  const impactSize = chain.impact.count ?? chain.impact.items.length;
  return impactSize > 1;
}

/**
 * Select a clean linear path from a DerivedChain for horizontal diagram display
 * (root -> up to 2 tasks along the deepest downstream dependency chain -> primary commitment).
 * The chain's full impact list stays intact on ProjectChainDisplay.impact.
 */
export function selectChainDisplayNodes(
  chain: DerivedChain,
  tasksById: ReadonlyMap<string, DerivedTask>,
  projectKey = '',
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
    if (path.length >= 4) return;
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

  // Choose one primary commitment node from explicit kind and recorded title — never parse commitment.id.
  const commitmentNodes = chain.nodes.filter((node) => node.kind === 'commitment' && node.commitment);
  const namedProjectCommitment = commitmentNodes.find((node) =>
    node.commitment?.kind === 'project' && hasRecordedCommitmentTitle(node.commitment, projectKey));
  const goalCommitment = commitmentNodes.find((node) => node.commitment?.kind === 'goal');
  const projectCommitment = commitmentNodes.find((node) => node.commitment?.kind === 'project');
  const endNode = namedProjectCommitment ?? goalCommitment ?? projectCommitment ?? commitmentNodes[0] ?? null;

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
 * Excludes dependency waits (downstream consequences of an upstream root),
 * groups review waits only when an explicit reviewer ID is recorded, and sorts by:
 *   1. behind.count descending (known counts before unknown)
 *   2. blocker kind priority: review (0) -> decision (1) -> external (2) (internal blockers before external)
 *   3. oldestDays descending (known before unknown)
 *   4. original read order for stability
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
      || BLOCKER_KIND_PRIORITY[a.item.kind] - BLOCKER_KIND_PRIORITY[b.item.kind]
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

/**
 * Derive the "现在可用" cell without inventing version numbers or mixing review round
 * indices into the version column. When `available_version` is absent (`not_recorded`),
 * show the upstream task's factual blocking state if known, otherwise `notRecorded`.
 */
function deriveAvailableCell(
  item: DependencyObservation,
  providerTask: DerivedTask | undefined,
  providerWaits: readonly DerivedWait[],
): DependencyAvailableCell {
  if (item.available_version && versionKey(item.available_version) !== null) {
    return { kind: 'recorded' };
  }
  if (item.missing?.available_version === 'permission_restricted') {
    return { kind: 'restricted' };
  }
  if (!providerTask) return { kind: 'notRecorded' };

  switch (providerTask.status) {
    case 'review':
      return { kind: 'inReview' };
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
      return { kind: 'notRecorded' };
  }
}

/**
 * Derive the "已采用" cell honestly without guessing ordinal version numbers or
 * conflating `not_recorded` with "还没有".
 */
function deriveAdoptedCell(item: DependencyObservation): DependencyAdoptedCell {
  if (item.adopted_version && versionKey(item.adopted_version) !== null) {
    if (item.available_version && versionKey(item.available_version) !== null) {
      return sameVersion(item.adopted_version, item.available_version)
        ? { kind: 'matchesAvailable' }
        : { kind: 'differsFromAvailable' };
    }
    return { kind: 'recorded' };
  }
  if (item.missing?.adopted_version === 'permission_restricted') {
    return { kind: 'restricted' };
  }
  if (item.missing?.adopted_version === 'adoption_unavailable') {
    return { kind: 'adoptionUnavailable' };
  }
  return { kind: 'notRecorded' };
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

  return items.map((item) => {
    const pairKey = `${item.work_id}\u0000${item.provider_work_id}`;
    const recordedReq = depRequirementByPair.get(pairKey);
    const requirement: DependencyRequirementKind = recordedReq
      ? { kind: 'recorded', text: recordedReq }
      : item.version_policy === 'fixed_delivery'
        ? { kind: 'pinned' }
        : { kind: 'reviewed' };

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
      adopted: deriveAdoptedCell(item),
      available: deriveAvailableCell(item, providerTask, providerWaits),
      status,
      highlighted: status === 'changed',
    };
  });
}

const PLANE_KEYS = ['report_received', 'authoritative', 'page_read'] as const;

function oldestTimestamp(planes: ReadonlyArray<PlaneFreshness | undefined>): number | undefined {
  if (planes.length === 0) return undefined;
  const times: number[] = [];
  for (const entry of planes) {
    if (typeof entry?.at_unix_ms !== 'number' || !Number.isFinite(entry.at_unix_ms)) {
      return undefined;
    }
    times.push(entry.at_unix_ms);
  }
  return Math.min(...times);
}

/**
 * Conservatively merge freshness across multiple read sources:
 * - If any source failed on a plane, the merged plane is `failed` with NO timestamp.
 * - Otherwise `stale > measured > not_measured`.
 * - Timestamps always take the OLDEST (`Math.min`) recorded time among the decisive planes
 *   so the footer never appears fresher than its stalest underlying source.
 */
export function mergeDependenciesFreshness(
  ...sources: Array<TeamFreshness | undefined>
): TeamFreshness | undefined {
  const present = sources.filter((item): item is TeamFreshness => Boolean(item));
  if (present.length === 0) return undefined;
  const plane = (key: (typeof PLANE_KEYS)[number]): PlaneFreshness => {
    const planes = present.map((source) => source[key]);
    if (planes.some((entry) => entry?.status === 'failed')) {
      return { status: 'failed' };
    }
    const stalePlanes = planes.filter((entry) => entry?.status === 'stale');
    if (stalePlanes.length > 0) {
      const oldest = oldestTimestamp(stalePlanes);
      return oldest !== undefined ? { status: 'stale', at_unix_ms: oldest } : { status: 'stale' };
    }
    const measuredPlanes = planes.filter((entry) => entry?.status === 'measured');
    if (measuredPlanes.length > 0) {
      const oldest = oldestTimestamp(measuredPlanes);
      return oldest !== undefined ? { status: 'measured', at_unix_ms: oldest } : { status: 'measured' };
    }
    return { status: 'not_measured' };
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
  directoryMembers,
}: {
  projectKey: string;
  overview?: TeamOverviewResponse;
  model?: ProjectListModel;
  dependencies?: DependencyListData;
  directoryMembers?: readonly ProjectDirectoryMemberRole[] | null;
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

  const memberNameById = new Map<string, string>();
  if (usableModel && usableModel.input.members.coverage !== 'unread') {
    for (const member of usableModel.input.members.items) {
      const id = cleanText(member.personId);
      const name = cleanName(member.displayName);
      if (id && name) memberNameById.set(id, name);
    }
  }

  // Compute verified progress for any commitment when task coverage is complete (never sniff commitment.id).
  const commitmentTasks = new Map<string, { done: number; total: number }>();
  if (derived && !orderingIncomparable && usableModel?.input.tasks.coverage === 'complete') {
    for (const task of derived.tasks) {
      if (!task.commitments) continue;
      for (const commitment of task.commitments) {
        const cid = cleanText(commitment.id);
        if (!cid) continue;
        const entry = commitmentTasks.get(cid) ?? { done: 0, total: 0 };
        entry.total += 1;
        if (task.status === 'verified') entry.done += 1;
        commitmentTasks.set(cid, entry);
      }
    }
  }

  const soleReviewerId = findSoleReviewerPersonId(directoryMembers);

  const openDecisions = usableModel && usableModel.input.decisions.coverage !== 'unread'
    ? usableModel.input.decisions.items.filter((decision) =>
      decision.state === 'open' && decision.current !== false && cleanText(decision.question) !== null)
    : [];

  const selectedDerivedChains = (derived?.chains ?? []).filter(isProjectBlockedChain);

  const chains: ProjectChainDisplay[] = selectedDerivedChains.map((chain) => {
    const { displayNodes, displayConnectors } = selectChainDisplayNodes(chain, tasksById, projectKey);
    const endNode = displayNodes.at(-1);
    const commitmentProgress = endNode?.kind === 'commitment' && endNode.commitment?.id
      ? (commitmentTasks.get(endNode.commitment.id) ?? null)
      : null;
    const impact: CountDetails<DerivedTask> = orderingIncomparable
      ? { count: null, items: chain.impact.items, unknownItems: [...chain.impact.unknownItems, { part: 'tasks', reason: 'ordering-incomparable' }] }
      : chain.impact;

    const soleReviewer = chain.root.kind === 'review'
      && soleReviewerId !== null
      && chain.root.person?.id === soleReviewerId;

    let relatedOpenDecision: RelatedOpenDecision | null = null;
    if (chain.root.kind === 'review' && openDecisions.length > 0) {
      const queueTaskIds = new Set(
        chain.root.queue.items
          .map((wait) => wait.task.id)
          .filter((id): id is string => id !== null),
      );
      const matched = openDecisions.find((decision) =>
        decision.taskId != null && queueTaskIds.has(decision.taskId));
      if (matched) {
        const firstResponderId = matched.allowedResponderIds?.find((id) => cleanText(id) !== null);
        relatedOpenDecision = {
          id: cleanText(matched.id),
          question: cleanText(matched.question)!,
          responderName: firstResponderId ? (memberNameById.get(firstResponderId) ?? null) : null,
        };
      }
    }

    return {
      chain,
      displayNodes,
      displayConnectors,
      impact,
      commitmentProgress,
      soleReviewer,
      relatedOpenDecision,
    };
  });

  const decisionImpactComplete = (derived?.chains ?? [])
    .filter((chain) => chain.root.kind === 'decision')
    .every((chain) => chain.impact.count !== null);
  const chainsComplete = Boolean(
    derived
    && !orderingIncomparable
    && derived.chainCounts.all.count !== null
    && decisionImpactComplete,
  );
  const chainsCount = chainsComplete ? chains.length : null;

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
    chainsComplete,
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
