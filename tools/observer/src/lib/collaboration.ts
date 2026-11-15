import type { UseQueryOptions } from '@tanstack/react-query';
import { apiPost } from './api';
import { readQueryDefaults } from './queries';
import type { DependencyListData, HandoffListData, TeamReadResponse } from './types';

export function dependenciesQueryOptions(project: string, workId: string | null, cursor: string | null, revoked: boolean) {
  return {
    queryKey: ['team', 'dependencies.list', project, workId, cursor],
    meta: { failureScope: 'object' },
    queryFn: ({ signal }: { signal: AbortSignal }) => apiPost<TeamReadResponse<DependencyListData>>('/api/team/read', {
      project,
      query: { protocol_version: 1, op: 'dependencies.list', limit: 20, max_context_bytes: 65536,
        ...(workId ? { work_id: workId } : {}), ...(cursor ? { cursor } : {}) },
    }, signal),
    enabled: Boolean(project) && !revoked,
    ...readQueryDefaults,
  } satisfies UseQueryOptions<TeamReadResponse<DependencyListData>>;
}

export function handoffsQueryOptions(project: string, workId: string, cursor: string | null, revoked: boolean) {
  return {
    queryKey: ['team', 'handoff.list', project, workId, cursor],
    meta: { failureScope: 'object' },
    queryFn: ({ signal }: { signal: AbortSignal }) => apiPost<TeamReadResponse<HandoffListData>>('/api/team/read', {
      project,
      query: { protocol_version: 1, op: 'handoff.list', work_id: workId, handoff_state: 'all',
        limit: 10, max_context_bytes: 65536, ...(cursor ? { cursor } : {}) },
    }, signal),
    enabled: Boolean(project && workId) && !revoked,
    ...readQueryDefaults,
  } satisfies UseQueryOptions<TeamReadResponse<HandoffListData>>;
}
