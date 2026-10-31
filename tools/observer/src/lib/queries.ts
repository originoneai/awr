import {
  keepPreviousData,
  QueryCache,
  QueryClient,
  type UseQueryOptions,
} from '@tanstack/react-query';
import { apiGet, isAuthError } from './api';
import { READ_TIMEOUT_MS } from './contract';
import type { TeamOverviewResponse, TeamProjectsResponse } from './types';

/**
 * Read defaults for every observation query. The poll interval is the bridge's
 * declared read bound (READ_TIMEOUT_MS in observation-contract.js), not an
 * invented number. No custom polling manager: TanStack Query owns it.
 */
export const readQueryDefaults = {
  refetchInterval: READ_TIMEOUT_MS,
  refetchOnWindowFocus: true,
  placeholderData: keepPreviousData,
  retry: 1,
} as const;

/** Query keys are namespaced per resource/tab. */
export const queryKeys = {
  projects: ['team', 'projects'] as const,
  overview: (project: string | null) => ['team', 'overview', project] as const,
  inbox: (project: string | null) => ['team', 'inbox', project] as const,
};

export function projectsQueryOptions() {
  return {
    queryKey: queryKeys.projects,
    queryFn: () => apiGet<TeamProjectsResponse>('/api/team/projects'),
    ...readQueryDefaults,
  } satisfies UseQueryOptions<TeamProjectsResponse>;
}

export function overviewQueryOptions(project: string | null) {
  return {
    queryKey: queryKeys.overview(project),
    queryFn: () =>
      apiGet<TeamOverviewResponse>(`/api/team/overview?project=${encodeURIComponent(project ?? '')}`),
    enabled: Boolean(project),
    ...readQueryDefaults,
  } satisfies UseQueryOptions<TeamOverviewResponse>;
}

/**
 * Identity or permission revocation clears restricted content at once: any
 * query that fails with an auth error code empties the whole cache instead of
 * leaving stale restricted data on screen.
 */
export function createAppQueryClient(overrides?: { retry?: number | boolean }) {
  const queryClient = new QueryClient({
    queryCache: new QueryCache({
      onError: (error) => {
        if (isAuthError(error)) {
          queryClient.clear();
        }
      },
    }),
    defaultOptions: {
      queries: { ...readQueryDefaults, ...(overrides?.retry !== undefined ? { retry: overrides.retry } : {}) },
    },
  });
  return queryClient;
}
