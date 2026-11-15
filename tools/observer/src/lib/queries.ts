import { QueryCache, QueryClient, type UseQueryOptions } from '@tanstack/react-query';
import pLimit from 'p-limit';
import { apiGet, apiPost, ApiError, isAuthError } from './api';
import type {
  ReviewData,
  TeamActivityResponse,
  TeamIdentity,
  TeamOverviewResponse,
  TeamProjectsResponse,
  TeamReadResponse,
  WorkInboxData,
  WorkNextData,
  SourceContentData,
  WorkSnapshotData,
  PlanningCandidatesData,
  RequiredSourcesData,
  ProjectDocumentsData,
  OrdinaryDecisionsData,
} from './types';

// AWR-前后端能力审查完整清单.md:212 and reviewer-web.md:22 require a 5 s
// visible-page cadence; READ_TIMEOUT_MS is only the per-read deadline.
export const POLL_INTERVAL_MS = 5000;
export const SHELL_POLL_INTERVAL_MS = 30000;
export const SNAPSHOT_POLL_INTERVAL_MS = 60000;
/** For lists that only give context (the plan list's progress words): the selected work's own read keeps the fast cadence. */
export const SNAPSHOT_SLOW_POLL_INTERVAL_MS = 600000;

const snapshotReadLimit = pLimit(4);

function retryRead(failureCount: number, error: unknown) {
  return !isAuthError(error) && !(error instanceof ApiError && error.code === 'CursorExpired') && failureCount < 1;
}

// Cursor-bound pages retain their resource cadence to recheck current visibility.
export const readQueryDefaults = {
  refetchInterval: POLL_INTERVAL_MS,
  refetchOnWindowFocus: true,
  retry: retryRead,
} as const;

/** Query keys are namespaced per resource/tab. */
export const queryKeys = {
  projects: ['team', 'projects'] as const,
  overview: (project: string | null) => ['team', 'overview', project] as const,
  capabilities: (project: string | null) => ['team', 'capabilities', project] as const,
  activity: (project: string | null, workId?: string) =>
    ['team', 'activity', 'development', project, workId ?? null] as const,
  review: (project: string | null, workId: string | null, roundId?: string | null) =>
    ['team', 'review.inspect', project, workId, roundId ?? null] as const,
  inbox: (project: string | null) => ['team', 'inbox', project] as const,
  workNext: (project: string | null) => ['team', 'work.next', project] as const,
  workInbox: (project: string | null) => ['team', 'work.inbox', project] as const,
  workSnapshot: (project: string | null, workId: string, workstreamId: string) =>
    ['team', 'work.snapshot', project, workId, workstreamId] as const,
  sourceContent: (project: string, sourcePath: string, expectedSha256: string) =>
    ['team', 'source.content', project, sourcePath, expectedSha256] as const,
};

/**
 * The bridge only accepts a source read for a controlled project-relative path with a 64-digit
 * lowercase digest; anything else would be answered InvalidInput without reaching the service,
 * so the page offers no entry (and sends nothing) for such a reference.
 */
export function readableSourceRef(path: string | null | undefined, sha256: string | null | undefined): boolean {
  const forbiddenInPath = [/^\//, /\.\./, /\\/, /:/, /[\x00-\x1f\x7f]/];
  return typeof path === 'string' && path.length > 0 && path.length <= 512
    && !forbiddenInPath.some((pattern) => pattern.test(path))
    && typeof sha256 === 'string' && /^[0-9a-f]{64}$/.test(sha256);
}

export function projectsQueryOptions(revoked = false) {
  return {
    queryKey: queryKeys.projects,
    queryFn: () => apiGet<TeamProjectsResponse>('/api/team/projects'),
    enabled: !revoked,
    ...readQueryDefaults,
    refetchInterval: SHELL_POLL_INTERVAL_MS,
  } satisfies UseQueryOptions<TeamProjectsResponse>;
}

export function overviewQueryOptions(project: string | null, revoked = false) {
  return {
    queryKey: queryKeys.overview(project),
    queryFn: () =>
      apiGet<TeamOverviewResponse>(`/api/team/overview?project=${encodeURIComponent(project ?? '')}`),
    enabled: Boolean(project) && !revoked,
    ...readQueryDefaults,
    refetchInterval: SHELL_POLL_INTERVAL_MS,
  } satisfies UseQueryOptions<TeamOverviewResponse>;
}

/** Capabilities is the flat discovery response; object reads carry a data envelope. */
interface CapabilitiesResponse {
  result: { identity?: TeamIdentity; queries?: string[] };
}

export function capabilitiesQueryOptions(project: string, revoked = false) {
  return {
    queryKey: queryKeys.capabilities(project),
    queryFn: () => apiPost<CapabilitiesResponse>(
      '/api/team/read',
      { project, query: { protocol_version: 1, op: 'capabilities' } },
    ),
    enabled: Boolean(project) && !revoked,
    ...readQueryDefaults,
    staleTime: Infinity,
    refetchInterval: false as const,
    refetchOnWindowFocus: false as const,
  } satisfies UseQueryOptions<CapabilitiesResponse>;
}

export function activityQueryOptions(project: string | null, revoked = false, workId?: string) {
  return {
    queryKey: queryKeys.activity(project, workId),
    meta: { failureScope: 'object' },
    queryFn: () => apiGet<TeamActivityResponse>(
      `/api/team/activity?project=${encodeURIComponent(project ?? '')}&kind=development${workId ? `&work_id=${encodeURIComponent(workId)}` : ''}`,
    ),
    enabled: Boolean(project) && !revoked,
    ...readQueryDefaults,
    // Embedded project/work summaries use the list cadence; the dedicated
    // activity page keeps its own 5 s first-page query in lib/activity.ts.
    refetchInterval: SNAPSHOT_POLL_INTERVAL_MS,
  } satisfies UseQueryOptions<TeamActivityResponse>;
}

interface TeamReadQuery {
  protocol_version: 1;
  op: 'work.next' | 'work.inbox' | 'work.snapshot' | 'review.inspect' | 'source.content' | 'planning.candidates' | 'sources.inspect' | 'documents.list' | 'decisions.list';
  work_id?: string;
  workstream_id?: string;
  review_round_id?: string;
  source_path?: string;
  expected_sha256?: string;
  cursor?: string;
  search?: string;
  limit?: number;
  max_context_bytes?: number;
  decision_state?: 'open' | 'all';
}

export function ordinaryDecisionsQueryOptions(project: string, state: 'open' | 'all', cursor: string | null, revoked: boolean) {
  return {
    queryKey: ['team', 'decisions.list', project, state, cursor],
    queryFn: ({ signal }: { signal: AbortSignal }) => teamRead<OrdinaryDecisionsData>(project, {
      protocol_version: 1, op: 'decisions.list', decision_state: state, limit: 10, max_context_bytes: 262144,
      ...(cursor ? { cursor } : {}),
    }, signal),
    enabled: Boolean(project) && !revoked,
    ...readQueryDefaults,
    refetchInterval: SNAPSHOT_POLL_INTERVAL_MS,
  } satisfies UseQueryOptions<TeamReadResponse<OrdinaryDecisionsData>>;
}

export function projectDocumentsQueryOptions(project: string, search: string, cursor: string | null, revoked: boolean) {
  return {
    queryKey: ['team', 'documents.list', project, search, cursor],
    queryFn: ({ signal }: { signal: AbortSignal }) => teamRead<ProjectDocumentsData>(project, {
      protocol_version: 1, op: 'documents.list', limit: 20, max_context_bytes: 262144,
      ...(search ? { search } : {}), ...(cursor ? { cursor } : {}),
    }, signal),
    enabled: Boolean(project) && !revoked,
    ...readQueryDefaults,
    refetchInterval: SNAPSHOT_POLL_INTERVAL_MS,
  } satisfies UseQueryOptions<TeamReadResponse<ProjectDocumentsData>>;
}

function teamRead<T>(project: string, query: TeamReadQuery, signal?: AbortSignal) {
  return apiPost<TeamReadResponse<T>>('/api/team/read', { project, query }, signal);
}

function validPlanRecord(record: unknown, kind: 'approval' | 'publication' | 'activation'): boolean {
  if (!record || typeof record !== 'object' || Array.isArray(record)) return false;
  const wrapper = record as Record<string, unknown>;
  if (wrapper.value === null) return typeof wrapper.reason === 'string' && wrapper.reason.length > 0;
  if (!wrapper.value || typeof wrapper.value !== 'object' || Array.isArray(wrapper.value) || wrapper.reason !== null) return false;
  const value = wrapper.value as Record<string, unknown>;
  if (typeof value.actor_id !== 'string' || !value.actor_id
    || typeof value.at_unix_ms !== 'number' || !Number.isFinite(value.at_unix_ms) || value.at_unix_ms < 0) return false;
  if (kind === 'approval') return typeof value.self_approved === 'boolean'
    && (value.display_name === null || typeof value.display_name === 'string');
  if (kind === 'publication') return typeof value.source_writeback_pending === 'boolean';
  return typeof value.matches_current_snapshot === 'boolean'
    && typeof value.activated_snapshot_id === 'string' && value.activated_snapshot_id.length > 0;
}

export function planningCandidatesQueryOptions(project: string, stream: string | null, cursor: string | null, revoked: boolean) {
  return {
    queryKey: ['team', 'planning.candidates', project, stream, cursor],
    queryFn: async ({ signal }: { signal: AbortSignal }) => {
      const response = await teamRead<PlanningCandidatesData>(project, {
        protocol_version: 1, op: 'planning.candidates', limit: 5, max_context_bytes: 262144,
        ...(stream ? { workstream_id: stream } : {}), ...(cursor ? { cursor } : {}),
      }, signal);
      const data = response.result?.data;
      if (!Array.isArray(data?.items) || !Number.isFinite(data.as_of?.at_unix_ms)
        || !(data.next_cursor === null || typeof data.next_cursor === 'string')
        || !data.items.every((item) => item && typeof item.candidate_id === 'string'
          && typeof item.state === 'string' && Number.isSafeInteger(item.draft_revision)
          && item.author && Number.isFinite(item.updated_at_unix_ms)
          && typeof item.baseline?.matches_current === 'boolean'
          && validPlanRecord(item.approval, 'approval') && validPlanRecord(item.publication, 'publication')
          && validPlanRecord(item.activation, 'activation')
          && (!item.publication.value || Boolean(item.approval.value))
          && (!item.activation.value || item.publication.value?.source_writeback_pending === false)
          && Array.isArray(item.current_diff?.items) && item.current_diff.items.every((change) =>
            change && typeof change.title === 'string' && typeof change.work_id === 'string'
            && (change.fields == null || Array.isArray(change.fields) && change.fields.every((field) =>
              field && typeof field.field === 'string' && field.before && field.after)))
          && ['added', 'removed', 'changed', 'unchanged'].every((key) => Number.isSafeInteger(item.current_diff.counts?.[key as keyof typeof item.current_diff.counts])))) {
        throw new ApiError('InvalidResponse', 'Planning observations are incomplete');
      }
      return response;
    },
    enabled: Boolean(project) && !revoked,
    ...readQueryDefaults,
    refetchInterval: SNAPSHOT_POLL_INTERVAL_MS,
  } satisfies UseQueryOptions<TeamReadResponse<PlanningCandidatesData>>;
}

export function requiredSourcesQueryOptions(project: string, stream: string | null, workId: string | null, cursor: string | null, revoked: boolean) {
  return {
    queryKey: ['team', 'sources.inspect', project, stream, workId, cursor],
    queryFn: async ({ signal }: { signal: AbortSignal }) => {
      const response = await teamRead<RequiredSourcesData>(project, {
        protocol_version: 1, op: 'sources.inspect', limit: 20, max_context_bytes: 262144,
        ...(workId ? { work_id: workId } : stream ? { workstream_id: stream } : {}),
        ...(cursor ? { cursor } : {}),
      }, signal);
      const data = response.result?.data;
      if (!Array.isArray(data?.items) || typeof data.collection?.complete !== 'boolean'
        || !Number.isSafeInteger(data.collection.required_total) || data.collection.required_total < data.items.length
        || data.total !== data.collection.required_total || !Number.isFinite(data.as_of?.at_unix_ms)
        || !Number.isFinite(data.collection.checked_at_unix_ms)
        || !(data.next_cursor === null || typeof data.next_cursor === 'string')
        || !data.items.every((item) => item && Number.isSafeInteger(item.source_position)
          && item.source_position > 0 && item.source_position <= data.total
          && (item.path === null || typeof item.path === 'string')
          && ['consistent', 'changed', 'missing', 'forbidden', 'unverified'].includes(item.status)
          && (!data.collection.complete || item.status === 'consistent')
          && (item.status !== 'forbidden' || item.path === null))
        || new Set(data.items.map((item) => item.source_position)).size !== data.items.length) {
        throw new ApiError('InvalidResponse', 'Required document observations are incomplete');
      }
      return response;
    },
    enabled: Boolean(project) && !revoked,
    ...readQueryDefaults,
    refetchInterval: SNAPSHOT_POLL_INTERVAL_MS,
  } satisfies UseQueryOptions<TeamReadResponse<RequiredSourcesData>>;
}

export function reviewQueryOptions(
  project: string | null,
  workId: string | null,
  roundId: string | null,
  revoked = false,
) {
  return {
    queryKey: queryKeys.review(project, workId, roundId),
    meta: { failureScope: 'object' },
    queryFn: ({ signal }: { signal: AbortSignal }) => teamRead<ReviewData>(project ?? '', {
      protocol_version: 1,
      op: 'review.inspect',
      work_id: workId ?? '',
      review_round_id: roundId ?? '',
    }, signal),
    enabled: Boolean(roundId) && Boolean(workId) && !revoked,
    ...readQueryDefaults,
  } satisfies UseQueryOptions<TeamReadResponse<ReviewData>>;
}

/** A source.content read, asked for only after a person asks: one shot, no polling, no refetch on focus, cached per (project, path, digest). */
export function sourceContentQueryOptions(
  project: string,
  sourcePath: string,
  expectedSha256: string,
  revoked = false,
) {
  return {
    queryKey: queryKeys.sourceContent(project, sourcePath, expectedSha256),
    queryFn: ({ signal, client }: { signal: AbortSignal; client: QueryClient }) => {
      // Scheduled observers can retain options from before the revocation render.
      if (isObservationReadRevoked(client)) throw new DOMException('Observation access revoked', 'AbortError');
      signal.throwIfAborted();
      return teamRead<SourceContentData>(project, {
        protocol_version: 1,
        op: 'source.content',
        source_path: sourcePath,
        expected_sha256: expectedSha256,
        max_context_bytes: 262144,
      }, signal);
    },
    enabled: !revoked,
    staleTime: Infinity,
    gcTime: Infinity,
    retry: false,
    refetchOnWindowFocus: false,
    // A read nobody asked for again is never re-sent: not when the window regains focus,
    // not when the network reconnects (a failed read has no data, so react-query would
    // otherwise count it stale and re-read it on reconnect), and not on a fresh mount
    // of the same cached read.
    refetchOnReconnect: false,
    refetchOnMount: false,
    refetchInterval: false,
  } satisfies UseQueryOptions<TeamReadResponse<SourceContentData>>;
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
  cadence: 'detail' | 'list' | 'slow' = 'detail',
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
    refetchInterval: cadence === 'list'
      ? SNAPSHOT_POLL_INTERVAL_MS
      : cadence === 'slow' ? SNAPSHOT_SLOW_POLL_INTERVAL_MS : POLL_INTERVAL_MS,
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
      onError: (error, query) => {
        // Object-scoped denials stay local. Authentication expiry always clears
        // the session. Retain the original namespaces for existing callers.
        const kind = query.queryKey[1];
        const sessionExpired = error instanceof ApiError
          && (error.code === 'Unauthenticated' || error.code === 'SessionExpired');
        const objectScoped = query.meta?.failureScope === 'object' || kind === 'review.inspect' || kind === 'activity';
        if (objectScoped && !sessionExpired) return;
        if (isAuthError(error)) revokeObservationReads(queryClient);
      },
    }),
    defaultOptions: {
      queries: { ...readQueryDefaults, ...(overrides?.retry !== undefined ? { retry: overrides.retry } : {}) },
    },
  });
  return queryClient;
}
