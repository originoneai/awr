import type { OperationKind } from '../../i18n/human-language';
import type { TeamActivityItem } from '../../lib/types';

export type SemanticTone = 'neutral' | 'accent' | 'positive' | 'warning' | 'negative';

export interface RawObservationTextResult {
  text: string | null;
  technical: { raw: string | null };
}

export interface StatusWordResult {
  text: string;
  known: boolean;
  missing: boolean;
  tone: SemanticTone;
  technical: { raw: string | null };
}

export interface DescribeUnknownResult {
  isUnknown: boolean;
  text: string | null;
  disclosure: string | null;
  technical: { result: string | null; means: string | null };
}

export interface AbsoluteTimeResult {
  text: string;
  technical: { atUnixMs: number | null };
}

export type AgeTextKind = 'relative' | 'today' | 'absolute' | 'future' | 'missing';

export interface AgeTextResult {
  text: string;
  atUnixMs: number | null;
  referenceUnixMs: number;
  kind: AgeTextKind;
  technical: { atUnixMs: number | null; referenceUnixMs: number; timeZone: string };
}

export type OperationEvent = Omit<TeamActivityItem, 'id' | 'action' | 'created_at_unix_ms'> & {
  id?: string;
  action?: string;
  created_at_unix_ms?: number;
};

export interface OperationWordResult {
  kind: OperationKind;
  text: string;
  technical: { rawAction: string | null };
}

export interface OperationStep {
  kind: OperationKind;
  text: string;
  count: number;
  separatorBefore: boolean;
  firstAtUnixMs: number | null;
  lastAtUnixMs: number | null;
  technical: {
    rawActions: Array<string | null>;
    eventIds: Array<string | undefined>;
  };
}

export interface OperationDisclosure {
  code: 'duplicate-id' | 'missing-id' | 'missing-time' | 'time-gap' | 'unmapped-action';
  text: string;
  technical: { ids?: string[]; rawAction?: string | null; gapMs?: number };
}

export interface FoldOperationsResult {
  events: OperationEvent[];
  uniqueEvents: OperationEvent[];
  steps: OperationStep[];
  duplicateIds: string[];
  disclosures: OperationDisclosure[];
  technical: { gapThresholdMs: number; order: 'oldest-first' | 'newest-first' };
}

export interface ActorAliasEntry {
  actorId: string;
  label: string;
  isSelf: boolean;
}

export interface ActorAliasResult {
  /** Aliases in label order; the viewer's own id (when present) comes first as "You". */
  entries: ActorAliasEntry[];
  /** Plain label for one actor id; a missing id says so instead of inventing a name. */
  labelOf: (actorId: string | null | undefined) => string;
}

export type WorkLabelKind = 'none' | 'named' | 'unnamed';

export type CheckLabelKind = 'named' | 'known' | 'numbered';

export interface ArtifactLabelResult {
  kind: CheckLabelKind;
  text: string;
  technical: { raw: string | null };
}

/** What was produced, in the plain words of the artifact vocabulary; the raw label stays for the details. */
export interface ProgressReportArtifact {
  text: string;
  raw: string | null;
}

export interface ActionWordsResult {
  shown: string[];
  other: number;
  technical: { raw: string[] };
}

export type DocumentLabelKind = 'named' | 'numbered';

export interface DocumentLabelResult {
  kind: DocumentLabelKind;
  text: string;
  technical: { raw: string | null };
}

export interface CheckLabelResult {
  kind: CheckLabelKind;
  text: string;
  technical: { raw: string | null };
}

export interface WorkLabelResult {
  kind: WorkLabelKind;
  text: string;
  technical: { raw: string | null };
}

export type DigestVerdict = 'verified' | 'mismatch' | 'unverified';

/** The part of Web Crypto the digest check needs; the core never reaches for `crypto` itself. */
export interface DigestSubtle {
  digest(algorithm: string, data: Uint8Array<ArrayBuffer>): Promise<ArrayBuffer>;
}

/** A list cut to what a person can take in; `more` says how many entries are not shown. */
export interface CappedList<T> {
  shown: T[];
  more: number;
}

export interface ProgressReportTest {
  name: string;
  outcome: string;
  tone: SemanticTone;
}

export interface ProgressReportResult {
  /** 'missing' when there is no checkpoint object at all. */
  kind: 'missing' | 'reported';
  /** The reporter's summary sentence, verbatim; null when none was given. */
  summary: string | null;
  /** What the reporter says is done, verbatim and in order. */
  completed: CappedList<string>;
  /** `reported` is true when the reporter gave the list at all, so an empty list can say "none reported". */
  blockers: CappedList<string> & { reported: boolean };
  tests: CappedList<ProgressReportTest>;
  /** The plain labels of what the reporter produced; references go to the technical layer. */
  artifacts: CappedList<ProgressReportArtifact>;
  /** Who reported, when, and that nobody checked it, in short plain parts. */
  declaration: string[];
  /** A warning when the task requirements changed after the report was written; null otherwise. */
  contractNote: string | null;
  /** Every raw field of the report, label and value, for the collapsed technical details. */
  technical: { items: Array<{ label: string; value: string | null }> };
}

export interface ClaimStateResult {
  /** 'unknown' when the read did not say (the field is missing) or said something that is not a claim state. */
  kind: 'claimed' | 'unclaimed' | 'unknown';
  text: string;
  technical: { raw: string | null };
}

export type WorkGroupWork = { workstream_id: string };
export type WorkstreamRef = { id: string; external_key?: unknown; title?: unknown };

export interface WorkGroup<W extends WorkGroupWork = WorkGroupWork> {
  id: string;
  title: string;
  works: W[];
}

export interface GroupWorksResult<W extends WorkGroupWork = WorkGroupWork> {
  /** Every input work once: grouped works in workstream order, then works of unknown workstreams. */
  works: W[];
  groups: WorkGroup<W>[];
  /** Title of each known workstream by id; an unknown id has no entry (callers fall back to the id). */
  titles: Record<string, string>;
}

export interface WorkStateSnapshot {
  /** The service's own hint addressed to the calling agent, not the reporter's statement. */
  next_step?: unknown;
  guidance?: { action?: { op?: unknown; note?: unknown } | null } | null;
  runtime?: { state?: unknown; selected_completion_id?: unknown } | null;
  observation?: {
    /** When the service measured this snapshot: the as-of of every fact in it. */
    observed_at_unix_ms?: unknown;
    reporting?: { client_reported_at_unix_ms?: unknown; client_stale?: unknown } | null;
    runtime?: { state?: unknown; selected_completion_id?: unknown } | null;
    /** The reporter's own checkpoint; `next_action` is what the reporter says comes next. */
    progress?: { next_action?: unknown; phase?: unknown } | null;
  } | null;
}

/** State of a read the derivation depends on: it never invents what it did not read. */
export type ReadState = 'loading' | 'ready' | 'failed';

export interface WorkStateInput {
  language: string;
  /** State of the work's own detail read. */
  readState: ReadState;
  /** The next step the overview row carries, used when the reporter's checkpoint has none or the read failed. */
  workNextStep?: unknown;
  /** The status the overview row carries, used when neither the reporter's phase nor the runtime state is read. */
  workStatus?: unknown;
  snapshot?: WorkStateSnapshot | null;
}

export interface WorkStateResult {
  progress: {
    /** Where the word comes from: the reporter's phase, the runtime state, the overview row, or nothing known. */
    kind: 'loading' | 'phase' | 'runtime' | 'row' | 'unknown';
    text: string;
    tone: SemanticTone;
    /** When the service measured the facts behind the word; null when the read did not happen or gave no time. */
    asOfUnixMs: number | null;
    technical: { phase: string | null; runtimeState: string | null; rowStatus: string | null; completionId: string | null };
  };
  next: {
    kind: 'loading' | 'reported' | 'missing';
    /** What the work's reporter said comes next, with long identifiers abbreviated, or a plain statement that none was reported. */
    text: string;
    /** The sentence exactly as the reporter wrote it (trimmed); null unless `text` is one. */
    fullText: string | null;
    /** The service's own hint and guidance are addressed to the agent and stay out of the default view. */
    technical: { nextAction: string | null; nextStep: string | null; guidanceOp: string | null; guidanceNote: string | null };
  };
  lastReport: {
    kind: 'loading' | 'measured' | 'not_measured';
    /** The whole statement in one string: the time and, when the service declared it stale, the note after it. */
    text: string;
    /** The reported time alone; null unless a time was reported. */
    timeText: string | null;
    /** The stale note alone ("may be outdated"); null unless the service declared the report stale and the work is still open. */
    staleText: string | null;
    atUnixMs: number | null;
    /** True or false as the service declared it; null when it said nothing. */
    stale: boolean | null;
    /** The work is finished (runtime completed, or the reporter's phase is delivered): nothing more is expected from its reporter, so it never carries a stale note. */
    finished: boolean;
    technical: { atUnixMs: number | null; runtimeState: string | null; phase: string | null };
  };
}
