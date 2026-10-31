import { QueryCache, QueryClient, type UseQueryOptions } from '@tanstack/react-query';
import { apiGet, isAuthError } from './api';
import type { TeamOverviewResponse, TeamProjectsResponse } from './types';

// AWR-前后端能力审查完整清单.md:212 and reviewer-web.md:22 require a 5 s
// visible-page cadence; READ_TIMEOUT_MS is only the per-read deadline.
export const POLL_INTERVAL_MS = 5000;

function retryRead(failureCount: number, error: unknown) {
  return !isAuthError(error) && failureCount < 1;
}

export const readQueryDefaults = {
  refetchInterval: POLL_INTERVAL_MS,
  refetchOnWindowFocus: true,
  retry: retryRead,
} as const;

/** Query keys are namespaced per resource/tab. */
export const queryKeys = {
  projects: ['team', 'projects'] as const,
  overview: (project: string | null) => ['team', 'overview', project] as const,
  inbox: (project: string | null) => ['team', 'inbox', project] as const,
};

export function projectsQueryOptions(revoked = false) {
  return {
    queryKey: queryKeys.projects,
    queryFn: () => apiGet<TeamProjectsResponse>('/api/team/projects'),
    enabled: !revoked,
    ...readQueryDefaults,
  } satisfies UseQueryOptions<TeamProjectsResponse>;
}

export function overviewQueryOptions(project: string | null, revoked = false) {
  return {
    queryKey: queryKeys.overview(project),
    queryFn: () =>
      apiGet<TeamOverviewResponse>(`/api/team/overview?project=${encodeURIComponent(project ?? '')}`),
    enabled: Boolean(project) && !revoked,
    ...readQueryDefaults,
  } satisfies UseQueryOptions<TeamOverviewResponse>;
}

interface RevocationState {
  revoked: boolean;
  listeners: Set<() => void>;
}

const revocationStates = new WeakMap<QueryClient, RevocationState>();

function revocationState(queryClient: QueryClient) {
  let state = revocationStates.get(queryClient);
  if (!state) {
    state = { revoked: false, listeners: new Set() };
    revocationStates.set(queryClient, state);
  }
  return state;
}

function notifyRevocation(queryClient: QueryClient) {
  for (const listener of revocationState(queryClient).listeners) listener();
}

export function isObservationReadRevoked(queryClient: QueryClient) {
  return revocationState(queryClient).revoked;
}

export function subscribeToObservationRevocation(queryClient: QueryClient, listener: () => void) {
  const state = revocationState(queryClient);
  state.listeners.add(listener);
  return () => state.listeners.delete(listener);
}

export function revokeObservationReads(queryClient: QueryClient) {
  const state = revocationState(queryClient);
  if (!state.revoked) {
    state.revoked = true;
    notifyRevocation(queryClient);
  }
  void queryClient.cancelQueries();
  queryClient.removeQueries();
}

export function resumeObservationReads(queryClient: QueryClient) {
  const state = revocationState(queryClient);
  if (state.revoked) {
    state.revoked = false;
    notifyRevocation(queryClient);
  }
}

export function createAppQueryClient(overrides?: { retry?: number | boolean }) {
  const queryClient = new QueryClient({
    queryCache: new QueryCache({
      onError: (error) => {
        if (isAuthError(error)) revokeObservationReads(queryClient);
      },
    }),
    defaultOptions: {
      queries: { ...readQueryDefaults, ...(overrides?.retry !== undefined ? { retry: overrides.retry } : {}) },
    },
  });
  return queryClient;
}
