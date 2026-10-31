declare module 'observation-contract' {
  export type PlaneState = 'measured' | 'stale' | 'failed' | 'not_measured';

  export interface AuthoritativePlaneLike {
    status?: string;
    project_revision?: string;
    source_snapshot_id?: string;
    coordinator_epoch?: string;
    [key: string]: unknown;
  }

  export type CompareVerdict = 'replace' | 'keep' | 'incomparable';

  export interface CurrencyResult {
    status: 'measured' | 'stale';
    stale_after_ms?: number;
    age_ms?: number;
  }

  export const PLANE_STATES: readonly PlaneState[];
  export const OBSERVATION_STALE_AFTER_MS: number;
  export const READ_TIMEOUT_MS: number;
  export const OVERVIEW_DEADLINE_MS: number;
  export function compareAuthoritative(
    held: AuthoritativePlaneLike | null | undefined,
    incoming: AuthoritativePlaneLike | null | undefined,
  ): CompareVerdict;
  export function resolveCurrency(input?: {
    serviceStale?: boolean;
    atUnixMs?: number;
    readAtMs?: number;
    staleAfterMs?: number;
  }): CurrencyResult;
}
