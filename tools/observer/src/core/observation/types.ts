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

export interface WorkLabelResult {
  kind: WorkLabelKind;
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
