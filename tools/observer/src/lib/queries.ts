import { QueryCache, QueryClient, type UseQueryOptions } from '@tanstack/react-query';
import pLimit from 'p-limit';
import { apiGet, apiPost, isAuthError } from './api';
import type {
  ReviewData,
  TeamActivityResponse,
  TeamOverviewResponse,
  TeamProjectsResponse,
  TeamReadResponse,
  WorkInboxData,
  WorkNextData,
  WorkSnapshotData,
} from './types';

// AWR-前后端能力审查完整清单.md:212 and reviewer-web.md:22 require a 5 s
// visible-page cadence; READ_TIMEOUT_MS is only the per-read deadline.
export const POLL_INTERVAL_MS = 5000;

const snapshotReadLimit = pLimit(4);

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
  activity: (project: string | null, workId?: string) =>
    ['team', 'activity', 'development', project, workId ?? null] as const,
  review: (project: string | null, workId: string | null) =>
    ['team', 'review.inspect', project, workId] as const,
  inbox: (project: string | null) => ['team', 'inbox', project] as const,
  workNext: (project: string | null) => ['team', 'work.next', project] as const,
  workInbox: (project: string | null) => ['team', 'work.inbox', project] as const,
  workSnapshot: (project: string | null, workId: string, workstreamId: string) =>
    ['team', 'work.snapshot', project, workId, workstreamId] as const,
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

export function activityQueryOptions(project: string | null, revoked = false, workId?: string) {
  return {
    queryKey: queryKeys.activity(project, workId),
    queryFn: () => apiGet<TeamActivityResponse>(
      `/api/team/activity?project=${encodeURIComponent(project ?? '')}&kind=development${workId ? `&work_id=${encodeURIComponent(workId)}` : ''}`,
    ),
    enabled: Boolean(project) && !revoked,
    ...readQueryDefaults,
  } satisfies UseQueryOptions<TeamActivityResponse>;
}

interface TeamReadQuery {
  protocol_version: 1;
  op: 'work.next' | 'work.inbox' | 'work.snapshot' | 'review.inspect';
  work_id?: string;
  workstream_id?: string;
}

function teamRead<T>(project: string, query: TeamReadQuery, signal?: AbortSignal) {
  return apiPost<TeamReadResponse<T>>('/api/team/read', { project, query }, signal);
}

export function reviewQueryOptions(project: string | null, workId: string | null, revoked = false) {
  return {
    queryKey: queryKeys.review(project, workId),
    queryFn: ({ signal }: { signal: AbortSignal }) => teamRead<ReviewData>(project ?? '', {
      protocol_version: 1,
      op: 'review.inspect',
      work_id: workId ?? '',
    }, signal),
    enabled: Boolean(project && workId) && !revoked,
    ...readQueryDefaults,
  } satisfies UseQueryOptions<TeamReadResponse<ReviewData>>;
}

export function workNextQueryOptions(project: string | null, revoked = false) {
  return {
    queryKey: queryKeys.workNext(project),
    queryFn: ({ signal }: { signal: AbortSignal }) =>
      teamRead<WorkNextData>(project ?? '', { protocol_version: 1, op: 'work.next' }, signal),
    enabled: Boolean(project) && !revoked,
    ...readQueryDefaults,
  } satisfies UseQueryOptions<TeamReadResponse<WorkNextData>>;
}

export function workInboxQueryOptions(project: string | null, enabled: boolean, revoked = false) {
  return {
    queryKey: queryKeys.workInbox(project),
    queryFn: ({ signal }: { signal: AbortSignal }) =>
      teamRead<WorkInboxData>(project ?? '', { protocol_version: 1, op: 'work.inbox' }, signal),
    enabled: Boolean(project) && enabled && !revoked,
    ...readQueryDefaults,
  } satisfies UseQueryOptions<TeamReadResponse<WorkInboxData>>;
}

export function workSnapshotQueryOptions(
  project: string | null,
  workId: string,
  workstreamId: string,
  revoked = false,
) {
  return {
    queryKey: queryKeys.workSnapshot(project, workId, workstreamId),
    queryFn: ({ signal }: { signal: AbortSignal }) => snapshotReadLimit(() => {
      signal.throwIfAborted();
      return teamRead<WorkSnapshotData>(project ?? '', {
        protocol_version: 1,
        op: 'work.snapshot',
        work_id: workId,
        workstream_id: workstreamId,
      }, signal);
    }),
    enabled: Boolean(project && workId && workstreamId) && !revoked,
    ...readQueryDefaults,
  } satisfies UseQueryOptions<TeamReadResponse<WorkSnapshotData>>;
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
