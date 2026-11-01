import { apiGet, apiPost } from './api';
import { readQueryDefaults } from './queries';
import type { TeamActivityResponse } from './types';

export type ActivityKind = 'development' | 'requests';

export interface ActivityFilters {
  workId?: string;
  actorId?: string;
  limit: number;
}

export function capabilitiesQueryOptions(project: string, revoked: boolean) {
  return {
    queryKey: ['team', 'capabilities', project],
    queryFn: () => apiPost<{ result: { data: { identity?: { actor_id?: string; can_read_project_audit?: boolean } } } }>(
      '/api/team/read',
      { project, query: { protocol_version: 1, op: 'capabilities' } },
    ),
    enabled: Boolean(project) && !revoked,
    ...readQueryDefaults,
  };
}

export function activityListQueryOptions(
  project: string,
  kind: ActivityKind,
  filters: ActivityFilters,
  revoked: boolean,
) {
  return {
    queryKey: ['team', 'activity-list', project, kind, filters],
    initialPageParam: undefined as string | undefined,
    queryFn: ({ pageParam }: { pageParam?: string }) => {
      const params = new URLSearchParams({ project, kind, limit: String(filters.limit) });
      if (filters.workId) params.set('work_id', filters.workId);
      if (filters.actorId) params.set('member_actor_id', filters.actorId);
      if (pageParam) params.set('cursor', pageParam);
      return apiGet<TeamActivityResponse>(`/api/team/activity?${params.toString()}`);
    },
    getNextPageParam: (lastPage: TeamActivityResponse) => {
      const cursor = lastPage.data.next_cursor;
      return typeof cursor === 'string' && cursor.length > 0 ? cursor : undefined;
    },
    enabled: Boolean(project) && !revoked,
    ...readQueryDefaults,
  };
}
