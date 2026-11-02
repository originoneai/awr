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
