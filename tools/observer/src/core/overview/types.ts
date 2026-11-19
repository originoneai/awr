/**
 * Canonical, already-read facts for the panorama derivation. Adapters own service-field mapping;
 * this module never performs I/O and never infers facts from report prose.
 *
 * `undefined` means that a field was not provided. An explicit `null` means that the read
 * established absence (for example, no report or no external wait). Optional numeric timestamps
 * use the same rule: a number is a measurement, null is a measured absence, and undefined is not
 * provided. Where absence does not make sense (such as a wait's start), null means unknown.
 */

export const OVERVIEW_PARTS = [
  'tasks',
  'dependencies',
  'decisions',
  'reviewRounds',
  'handoffs',
  'members',
  'activity',
] as const;

export type OverviewPart = (typeof OVERVIEW_PARTS)[number];

export type ReadCoverageReason =
  | 'not-read'
  | 'read-failed'
  | 'truncated'
  | 'permission-restricted'
  | 'incomplete';

export type ReadSlice<T> =
  | { coverage: 'complete'; items: readonly T[] }
  | { coverage: 'partial'; items: readonly T[]; reason: ReadCoverageReason }
  | { coverage: 'unread'; items?: readonly T[]; reason: ReadCoverageReason };

export interface OverviewCriterionEvidence {
  /** The evidence passed on the candidate being considered; null/omitted is unknown. */
  passed?: boolean | null;
  /** The evidence belongs to the current candidate; null/omitted is unknown. */
  current?: boolean | null;
}

export interface OverviewCriterion {
  /** Only evidence explicitly bound to this criterion belongs here, never a task-wide test list. */
  evidence?: readonly OverviewCriterionEvidence[] | null;
}

export interface OverviewProgressReport {
  /** A deterministic adapter maps the reporter's completion declaration to this fact. */
  completed?: boolean | null;
  reportedAtUnixMs?: number | null;
}

export interface OverviewExternalWait {
  /** Free text recorded for the outside party; null when the read did not provide it. */
  party?: string | null;
  what?: string | null;
  sinceUnixMs?: number | null;
}

export interface OverviewCommitment {
  id?: string | null;
  title?: string | null;
  kind?: 'project' | 'goal' | null;
}

export interface OverviewTask {
  id?: string | null;
  title?: string | null;
  ownerPersonId?: string | null;
  agentName?: string | null;
  /** Explicit rollup scopes; an omitted scope remains unknown. */
  projectId?: string | null;
  groupId?: string | null;
  goalIds?: readonly string[] | null;
  /** Commitments affected when this task is held up; [] is known none, null/omitted is unknown. */
  commitments?: readonly OverviewCommitment[] | null;
  /** Verification time used by inclusive period counts; approval/report time is not a substitute. */
  verifiedAtUnixMs?: number | null;
  acceptance?: readonly OverviewCriterion[] | null;
  report?: OverviewProgressReport | null;
  /** Null means no result was submitted. A review round also establishes a submission. */
  submittedResultAtUnixMs?: number | null;
  /** The reviewed result is integrated; approval alone is not integration. */
  integrated?: boolean | null;
  started?: boolean | null;
  agentActive?: boolean | null;
  /** Null means no outside wait; an omitted field is not the same fact. */
  externalWait?: OverviewExternalWait | null;
}

export interface OverviewDependency {
  id?: string | null;
  consumerTaskId?: string | null;
  providerTaskId?: string | null;
  /** False is an unresolved requirement. Graph reachability uses known edges regardless of this. */
  satisfied?: boolean | null;
  what?: string | null;
  sinceUnixMs?: number | null;
}

export interface OverviewDecision {
  id?: string | null;
  taskId?: string | null;
  state?: string | null;
  /** Maps the decision's baseline binding. False is historical; null is not provided. */
  current?: boolean | null;
  question?: string | null;
  impact?: string | null;
  allowedResponderIds?: readonly string[] | null;
  createdAtUnixMs?: number | null;
}

export interface OverviewReviewRound {
  id?: string | null;
  taskId?: string | null;
  roundIndex?: number | null;
  state?: string | null;
  /** Current contract and candidate binding; false is a historical round. */
  current?: boolean | null;
  /** Explicit independence fact, not a guess from a display name. */
  independent?: boolean | null;
  authorPersonId?: string | null;
  reviewerPersonId?: string | null;
  authorActorId?: string | null;
  reviewerActorId?: string | null;
  openedAtUnixMs?: number | null;
}

export interface OverviewHandoff {
  id?: string | null;
  taskId?: string | null;
  kind?: string | null;
  status?: string | null;
  pending?: boolean | null;
  fromPersonId?: string | null;
  toPersonId?: string | null;
  updatedAtUnixMs?: number | null;
}

export interface OverviewMember {
  personId?: string | null;
  displayName?: string | null;
  groupId?: string | null;
}

export interface OverviewActivity {
  id?: string | null;
  taskId?: string | null;
  action?: string | null;
  actorId?: string | null;
  atUnixMs?: number | null;
}

/** Inclusive epoch-millisecond bounds; the caller owns calendar and timezone conversion. */
export interface OverviewPeriod {
  startUnixMs?: number | null;
  endUnixMs?: number | null;
}

export interface OverviewPersonViewScope {
  personId?: string | null;
  projectId?: string | null;
}

export interface OverviewDepartmentViewScope {
  personId?: string | null;
  projectIds?: readonly string[] | null;
}

export interface OverviewViewOptions {
  me?: OverviewPersonViewScope | null;
  lead?: OverviewPersonViewScope | null;
  department?: OverviewDepartmentViewScope | null;
  top?: { personId?: string | null; goalIds?: readonly string[] | null } | null;
  currentWeek?: OverviewPeriod | null;
  previousWeek?: OverviewPeriod | null;
  currentMonth?: OverviewPeriod | null;
  previousMonth?: OverviewPeriod | null;
}

export interface OverviewDerivationOptions {
  /** The caller's common read time; the core never reads the wall clock. */
  nowUnixMs: number;
  /** Required on every call. There is deliberately no default stalled threshold. */
  stalledAfterDays: number;
  /** Missing viewer scopes or periods produce unknown view metrics, never invented windows. */
  views?: OverviewViewOptions | null;
}

export interface OverviewInput {
  tasks: ReadSlice<OverviewTask>;
  dependencies: ReadSlice<OverviewDependency>;
  decisions: ReadSlice<OverviewDecision>;
  reviewRounds: ReadSlice<OverviewReviewRound>;
  handoffs: ReadSlice<OverviewHandoff>;
  members: ReadSlice<OverviewMember>;
  activity: ReadSlice<OverviewActivity>;
  options: OverviewDerivationOptions;
}

export const TASK_STATUSES = [
  'verified',
  'claimed',
  'returned',
  'review',
  'running',
  'stalled',
  'waiting',
  'ready',
  'planned',
] as const;

export type TaskStatus = (typeof TASK_STATUSES)[number];
export type DerivedTaskStatus = TaskStatus | 'unknown';

export const WAIT_KINDS = ['decision', 'dependency', 'review', 'external'] as const;
export type WaitKind = (typeof WAIT_KINDS)[number];

export interface PersonRef {
  id: string;
  /** Null when the directory did not provide a name; the id is never used as a display name. */
  displayName: string | null;
}

export interface DerivedTask {
  /** Null for a listed row whose id was not provided. Such rows never enter graph counts. */
  id: string | null;
  title: string | null;
  owner: PersonRef | null;
  agentName: string | null;
  projectId: string | null;
  groupId: string | null;
  /** Null is unknown; an empty array explicitly says no goal is bound. */
  goalIds: string[] | null;
  /** Null is unknown; an empty array explicitly says no commitment is bound. */
  commitments: OverviewCommitment[] | null;
  verifiedAtUnixMs: number | null;
  status: DerivedTaskStatus;
  /** Stable semantic reason codes; UI wording belongs to the caller's vocabulary. */
  unknownReasons: string[];
  verification: {
    everyCriterionSupported: boolean | null;
    independentReviewApproved: boolean | null;
    integrated: boolean | null;
  };
}

export interface CoverageIssue {
  part: OverviewPart;
  reason: string;
  index?: number;
  taskId?: string | null;
}

/**
 * A known count and its details are constructed from one array. If relevant facts are incomplete,
 * count is null (unknown), items retains only confirmed details, and unknownItems explains the gap.
 * Consequently count === items.length whenever count is non-null; unknown items are never summed.
 */
export interface CountDetails<T> {
  count: number | null;
  items: T[];
  unknownItems: CoverageIssue[];
}

export interface DerivedWait {
  kind: WaitKind;
  /** The record that establishes this wait, when it has an id. No display text is generated. */
  sourceId: string | null;
  task: DerivedTask;
  /** People responsible for the next step; no member is guessed from a task title or report. */
  waitingOn: PersonRef[];
  /** True when the read did not identify a responsible person (including a role-only queue). */
  waitingOnUnknown: boolean;
  /** An outside party is free text, not a member identity. */
  externalParty: string | null;
  /** The recorded question, dependency reason, review request, or outside request. */
  what: string | null;
  providerTask: DerivedTask | null;
  startedAtUnixMs: number | null;
  /** Whole elapsed days; null is unknown and is never coerced to zero. */
  durationDays: number | null;
  /** Includes the waiting task and unique direct/transitive dependents, matching behindSet. */
  behind: CountDetails<DerivedTask>;
}

export type ChainRootKind = Exclude<WaitKind, 'dependency'>;

export interface DerivedChainRoot {
  kind: ChainRootKind;
  /** Explicit reviewer identity for a grouped queue; unknown reviewers stay in separate chains. */
  person: PersonRef | null;
  externalParty: string | null;
  queue: CountDetails<DerivedWait>;
  /** Null when any member of the queue has an unknown elapsed time. */
  oldestDays: number | null;
  /** The root is the red blocker marker in the prototype. */
  tone: 'negative';
}

export interface ChainNode {
  id: string;
  kind: 'root' | 'task' | 'commitment';
  root: boolean;
  tone: 'negative' | null;
  task: DerivedTask | null;
  commitment: OverviewCommitment | null;
}

export interface ChainConnector {
  fromNodeId: string;
  toNodeId: string;
  kind: 'blocks' | 'depends' | 'affects';
}

export interface DerivedChain {
  id: string;
  root: DerivedChainRoot;
  nodes: ChainNode[];
  connectors: ChainConnector[];
  impact: CountDetails<DerivedTask>;
  commitments: CountDetails<OverviewCommitment>;
  /** Footer rendering uses the same count/detail objects as the chain, never separate totals. */
  footer: {
    impact: CountDetails<DerivedTask>;
    commitments: CountDetails<OverviewCommitment>;
  };
}

export interface PersonPendingItem {
  kind: 'wait' | 'handoff';
  task: DerivedTask | null;
  wait: DerivedWait | null;
  handoff: OverviewHandoff | null;
}

export interface PersonSummary {
  person: PersonRef;
  /** Non-dependency waits addressed to this person plus pending handoffs addressed to them. */
  pending: CountDetails<PersonPendingItem>;
  /** Prototype inProgress: review/stalled/returned/claimed/running, or waiting with an Agent. */
  inProgress: CountDetails<DerivedTask>;
  /** All four wait kinds recorded on tasks explicitly owned by this person. */
  waitingOn: CountDetails<DerivedWait>;
  /** Dependency waits whose provider result is explicitly owned by this person. */
  owedBy: CountDetails<DerivedWait>;
}

export const VIEW_IDS = ['me', 'lead', 'department', 'top'] as const;
export type ViewId = (typeof VIEW_IDS)[number];

export type ViewMetricKey =
  | 'verified-period'
  | 'agents-running'
  | 'waiting-by-me'
  | 'pending-for-me'
  | 'in-progress'
  | 'waits'
  | 'projects'
  | 'cross-group-waits'
  | 'goals'
  | 'verified-total'
  | 'blockers'
  | 'decisions-for-me';

export interface ViewAgentSummary {
  agentName: string;
  tasks: DerivedTask[];
}

export interface ViewUnit {
  id: string;
  kind: 'project' | 'goal';
  tasks: DerivedTask[];
}

export interface ViewMetric<T> extends CountDetails<T> {
  key: ViewMetricKey;
  /** The same vcount rule over the caller's previous inclusive period. */
  previousPeriod?: CountDetails<T>;
  /** Top-level verified completion carries the prototype's claimed comparison. */
  claimed?: CountDetails<DerivedTask>;
}

export interface OverviewViewSummary {
  /** Exactly four metrics, in the prototype's left-to-right order. */
  metrics: [ViewMetric<unknown>, ViewMetric<unknown>, ViewMetric<unknown>, ViewMetric<unknown>];
}

export interface OverviewPartCoverage {
  state: 'complete' | 'partial' | 'unread';
  reason: ReadCoverageReason | null;
  issues: CoverageIssue[];
}

export interface OverviewSectionCoverage {
  complete: boolean;
  issues: CoverageIssue[];
}

export interface OverviewBaseCoverage {
  /** Coverage of the seven input planes; derived sections report their own additional gaps. */
  complete: boolean;
  parts: Record<OverviewPart, OverviewPartCoverage>;
  sections: {
    status: OverviewSectionCoverage;
    waits: OverviewSectionCoverage;
  };
}

export interface OverviewCoverage extends OverviewBaseCoverage {
  sections: OverviewBaseCoverage['sections'] & {
    chains: OverviewSectionCoverage;
    people: OverviewSectionCoverage;
    views: OverviewSectionCoverage;
  };
}

export interface OverviewStatusWaitingDerivation {
  tasks: DerivedTask[];
  waits: DerivedWait[];
  taskCounts: {
    all: CountDetails<DerivedTask>;
    byStatus: Record<DerivedTaskStatus, CountDetails<DerivedTask>>;
  };
  waitCounts: {
    all: CountDetails<DerivedWait>;
    byKind: Record<WaitKind, CountDetails<DerivedWait>>;
  };
  coverage: OverviewBaseCoverage;
}

export interface OverviewDerivation extends OverviewStatusWaitingDerivation {
  chains: DerivedChain[];
  chainCounts: { all: CountDetails<DerivedChain> };
  people: PersonSummary[];
  peopleCounts: { all: CountDetails<PersonSummary> };
  views: Record<ViewId, OverviewViewSummary>;
  coverage: OverviewCoverage;
}
