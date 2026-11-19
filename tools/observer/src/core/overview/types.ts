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

export interface OverviewTask {
  id?: string | null;
  title?: string | null;
  ownerPersonId?: string | null;
  agentName?: string | null;
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
}

export interface OverviewActivity {
  id?: string | null;
  taskId?: string | null;
  action?: string | null;
  actorId?: string | null;
  atUnixMs?: number | null;
}

export interface OverviewDerivationOptions {
  /** The caller's common read time; the core never reads the wall clock. */
  nowUnixMs: number;
  /** Required on every call. There is deliberately no default stalled threshold. */
  stalledAfterDays: number;
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

export interface OverviewPartCoverage {
  state: 'complete' | 'partial' | 'unread';
  reason: ReadCoverageReason | null;
  issues: CoverageIssue[];
}

export interface OverviewCoverage {
  /** True only when every input plane, including planes reserved for the next slice, is complete. */
  complete: boolean;
  parts: Record<OverviewPart, OverviewPartCoverage>;
  sections: {
    status: { complete: boolean; issues: CoverageIssue[] };
    waits: { complete: boolean; issues: CoverageIssue[] };
  };
}

export interface OverviewDerivation {
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
  coverage: OverviewCoverage;
  // The second PR adds chains, people and view summaries additively; these inputs and task/wait
  // identities are stable so it does not need to replace the status/waiting derivation.
}
