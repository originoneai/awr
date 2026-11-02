import { apiGet } from './api';
import { POLL_INTERVAL_MS, readQueryDefaults } from './queries';
import type { TeamActivityResponse } from './types';

export { capabilitiesQueryOptions } from './queries';

export type ActivityKind = 'development' | 'requests';

export interface ActivityFilters {
  workId?: string;
  actorId?: string;
  limit: number;
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
    // Only the first page refreshes on the normal cadence; loaded history is stable.
    refetchInterval: (query: { state: { data?: { pages: TeamActivityResponse[] } } }) =>
      (query.state.data?.pages.length ?? 0) <= 1 ? POLL_INTERVAL_MS : false,
  };
}
