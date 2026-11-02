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
