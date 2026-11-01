import type { UseQueryOptions } from '@tanstack/react-query';
import { apiPost } from './api';
import { readQueryDefaults } from './queries';
import type { TeamReadResponse } from './types';

export type OutcomeData = Record<string, unknown>;
export type OutcomeSelector = Record<string, unknown>;

/** Bounded, exact-object read through the existing Inspector /api/team/read route. */
export function outcomeQueryOptions(
  project: string,
  op: string,
  selector: OutcomeSelector,
  revoked: boolean,
  enabled = true,
) {
  const query = { protocol_version: 1 as const, op, ...selector };
  return {
    queryKey: ['team', 'outcome', project, op, selector],
    queryFn: ({ signal }: { signal: AbortSignal }) =>
      apiPost<TeamReadResponse<OutcomeData>>('/api/team/read', { project, query }, signal),
    enabled: Boolean(project) && enabled && !revoked,
    ...readQueryDefaults,
  } satisfies UseQueryOptions<TeamReadResponse<OutcomeData>>;
}
