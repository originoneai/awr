import type { UseQueryOptions } from '@tanstack/react-query';
import { apiPost } from './api';
import { readQueryDefaults } from './queries';
import type { ReviewRoundsData, TeamReadResponse } from './types';

export type OutcomeData = Record<string, unknown>;
export type OutcomeSelector = Record<string, unknown>;

/** Bounded, exact-object read through the Observer /api/team/read route. */
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
    meta: { failureScope: 'object' },
    queryFn: ({ signal }: { signal: AbortSignal }) =>
      apiPost<TeamReadResponse<OutcomeData>>('/api/team/read', { project, query }, signal),
    enabled: Boolean(project) && enabled && !revoked,
    ...readQueryDefaults,
  } satisfies UseQueryOptions<TeamReadResponse<OutcomeData>>;
}

export function reviewRoundsQueryOptions(project: string, workId: string, stream: string, cursor: string | null, revoked: boolean) {
  const query = { protocol_version: 1 as const, op: 'review.rounds', work_id: workId,
    workstream_id: stream, limit: 20, max_context_bytes: 65536, ...(cursor ? { cursor } : {}) };
  return {
    queryKey: ['team', 'review-rounds', project, workId, stream, cursor],
    meta: { failureScope: 'object' },
    queryFn: ({ signal }: { signal: AbortSignal }) =>
      apiPost<TeamReadResponse<ReviewRoundsData>>('/api/team/read', { project, query }, signal),
    enabled: Boolean(project && workId && stream) && !revoked,
    ...readQueryDefaults,
  } satisfies UseQueryOptions<TeamReadResponse<ReviewRoundsData>>;
}
