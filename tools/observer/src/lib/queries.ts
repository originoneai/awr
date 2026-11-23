import { QueryCache, QueryClient, type UseQueryOptions } from '@tanstack/react-query';
import pLimit from 'p-limit';
import { apiGet, apiPost, ApiError, isAuthError } from './api';
import { compareAuthoritative } from './contract';
import type {
  ReviewData,
  TeamActivityResponse,
  TeamIdentity,
  TeamOverviewResponse,
  TeamOverviewModelResponse,
  TeamPeriodUpdateResponse,
  TeamPeriodUpdatesResponse,
  TeamPortfolioResponse,
  TeamProjectsResponse,
  TeamReadResponse,
  TeamShellResponse,
  WorkInboxData,
  WorkNextData,
  SourceContentData,
  WorkSnapshotData,
  PlanningCandidatesData,
  RequiredSourcesData,
  ProjectDocumentsData,
  OrdinaryDecision,
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
  shell: ['team', 'shell'] as const,
  projects: ['team', 'projects'] as const,
  portfolio: ['team', 'portfolio'] as const,
  overview: (project: string | null) => ['team', 'overview', project] as const,
  overviewModel: (project: string | null) => ['team', 'overview-model', project] as const,
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
  sourceContent: (project: string, sourcePath: string, expectedSha256: string, workId?: string) =>
    ['team', 'source.content', project, sourcePath, expectedSha256, ...(workId ? [workId] : [])] as const,
  periodUpdates: (project: string | null, cursor: string | null = null) =>
    ['team', 'updates.list', project, cursor] as const,
  periodUpdate: (project: string | null, updateId: string | null) =>
    ['team', 'update.inspect', project, updateId] as const,
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

export function shellQueryOptions(revoked = false) {
  return {
    queryKey: queryKeys.shell,
    queryFn: () => apiGet<TeamShellResponse>('/api/team/shell'),
    // The shell's own facts (read source, perspective heights) change rarely; the shell
    // cadence keeps them current without adding a page-level read. Revocation silences
    // this read like every other restricted read.
    enabled: !revoked,
    ...readQueryDefaults,
    refetchInterval: SHELL_POLL_INTERVAL_MS,
  } satisfies UseQueryOptions<TeamShellResponse>;
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

/**
 * The bridge checks the session on every poll and caches only within that session.
 * This client-side guard applies the same authoritative ordering contract when
 * two browser requests settle out of order; an older reply returns the held newer model.
 */
export function overviewModelQueryOptions(project: string | null, revoked = false) {
  return {
    queryKey: queryKeys.overviewModel(project),
    queryFn: async ({ signal, client }: { signal: AbortSignal; client: QueryClient }) => {
      const state = revocationState(client);
      if (revoked || state.revoked) throw new DOMException('Observation access revoked', 'AbortError');
      const key = project ?? '';
      const ordering = state.overviewModels.get(key) ?? {};
      state.overviewModels.set(key, ordering);
      signal.throwIfAborted();
      const incoming = await apiGet<TeamOverviewModelResponse>(
        `/api/team/overview-model?project=${encodeURIComponent(key)}`,
      );
      signal.throwIfAborted();
      if (state.revoked) throw new DOMException('Observation access revoked', 'AbortError');
      const held = ordering.held;
      if (held) {
        const verdict = compareAuthoritative(held.freshness.authoritative, incoming.freshness.authoritative);
        if (verdict === 'keep') return held;
        if (verdict === 'incomparable') return { ...held, ordering: 'incomparable' as const };
      }
      ordering.held = incoming;
      return incoming;
    },
    enabled: Boolean(project) && !revoked,
    ...readQueryDefaults,
    refetchInterval: POLL_INTERVAL_MS,
  } satisfies UseQueryOptions<TeamOverviewModelResponse>;
}

/**
 * Single member-scoped portfolio composition (AWR-PPD-060): one read summarizes every
 * visible project without N per-project calls. Applies the same authoritative ordering
 * and revocation guards as overviewModelQueryOptions.
 */
export function portfolioQueryOptions(revoked = false, enabled = true) {
  return {
    queryKey: queryKeys.portfolio,
    queryFn: async ({ signal, client }: { signal: AbortSignal; client: QueryClient }) => {
      const state = revocationState(client);
      if (revoked || state.revoked) throw new DOMException('Observation access revoked', 'AbortError');
      signal.throwIfAborted();
      const incoming = await apiGet<TeamPortfolioResponse>('/api/team/portfolio');
      signal.throwIfAborted();
      if (state.revoked) throw new DOMException('Observation access revoked', 'AbortError');
      const result = incoming?.result;
      const data = result?.data;
      if (incoming?.ok !== true || result?.protocol_version !== 1 || result?.op !== 'portfolio.overview'
        || result?.selection_basis !== 'member' || data?.execution_authorized !== false
        || !Array.isArray(data?.projects) || !Number.isSafeInteger(data?.total_projects)
        || data.total_projects < data.projects.length || typeof data?.truncated !== 'boolean') {
        throw new ApiError('InvalidResponse', 'Portfolio observations are incomplete');
      }
      const held = state.portfolio.held;
      if (held && incoming.freshness?.authoritative && held.freshness?.authoritative) {
        const verdict = compareAuthoritative(held.freshness.authoritative, incoming.freshness.authoritative);
        if (verdict === 'keep') return held;
        if (verdict === 'incomparable') return { ...held, ordering: 'incomparable' as const };
      }
      state.portfolio.held = incoming;
      return incoming;
    },
    enabled: enabled && !revoked,
    ...readQueryDefaults,
    refetchInterval: POLL_INTERVAL_MS,
  } satisfies UseQueryOptions<TeamPortfolioResponse>;
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
  decision_state?: OrdinaryDecision['state'] | 'all';
}

export function ordinaryDecisionsQueryOptions(project: string, state: OrdinaryDecision['state'] | 'all', cursor: string | null, revoked: boolean) {
  return {
    queryKey: ['team', 'decisions.list', project, state, cursor],
    queryFn: ({ signal }: { signal: AbortSignal }) => teamRead<OrdinaryDecisionsData>(project, {
      protocol_version: 1, op: 'decisions.list', decision_state: state, limit: 10, max_context_bytes: 262144,
      ...(cursor ? { cursor } : {}),
    }, signal),
    enabled: Boolean(project) && !revoked,
    ...readQueryDefaults,
    refetchOnMount: 'always',
    retry: (count, error) => !(error instanceof ApiError && error.code === 'Unsupported') && retryRead(count, error),
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
    refetchOnMount: 'always',
    retry: (count, error) => !(error instanceof ApiError && error.code === 'Unsupported') && retryRead(count, error),
    refetchInterval: SNAPSHOT_POLL_INTERVAL_MS,
  } satisfies UseQueryOptions<TeamReadResponse<ProjectDocumentsData>>;
}

function teamRead<T>(project: string, query: TeamReadQuery, signal?: AbortSignal) {
  return apiPost<TeamReadResponse<T>>('/api/team/read', { project, query }, signal);
}

function validPlanRecord(record: unknown, kind: 'approval' | 'publication' | 'activation' | 'rejection'): boolean {
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
  if (kind === 'rejection') return typeof value.reason === 'string' && value.reason.trim().length > 0
    && new TextEncoder().encode(value.reason).length <= 4096 && !value.reason.includes('\0')
    && (value.display_name === null || typeof value.display_name === 'string');
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
          && (item.rejection === undefined || validPlanRecord(item.rejection, 'rejection'))
          && (!item.rejection?.value || item.state === 'rejected')
          && (item.state !== 'rejected' || (Boolean(item.rejection?.value) || item.rejection?.reason === 'permission_restricted')
            && !item.approval.value && !item.publication.value && !item.activation.value)
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
  workId?: string,
) {
  return {
    queryKey: queryKeys.sourceContent(project, sourcePath, expectedSha256, workId),
    queryFn: ({ signal, client }: { signal: AbortSignal; client: QueryClient }) => {
      // Scheduled observers can retain options from before the revocation render.
      if (isObservationReadRevoked(client)) throw new DOMException('Observation access revoked', 'AbortError');
      signal.throwIfAborted();
      return teamRead<SourceContentData>(project, {
        protocol_version: 1,
        op: 'source.content',
        ...(workId ? { work_id: workId } : {}),
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

const PLANE_STATES = new Set(['measured', 'stale', 'failed', 'not_measured']);
const PERIOD_UPDATE_ENVELOPE_KEYS = [
  'protocol_version', 'scope_id', 'selection_basis', 'coordinator_epoch',
  'project_status', 'source_snapshot_id', 'project_revision', 'data',
] as const;
const PERIOD_UPDATE_AS_OF_KEYS = ['at_unix_ms', 'source_snapshot_id', 'project_revision'] as const;
const PERIOD_UPDATE_RECORD_KEYS = [
  'update_id', 'period_kind', 'period_key', 'starts_on', 'ends_on', 'status', 'summary',
  'author', 'recorded_at_unix_ms', 'version', 'supersedes_update_id', 'fact_snapshot_ref', 'current',
] as const;
const PERIOD_UPDATE_AUTHOR_KEYS = ['person_id', 'display_name'] as const;
const PERIOD_UPDATE_LIST_DATA_KEYS = [
  'project_id', 'items', 'next_cursor', 'truncated', 'as_of', 'execution_authorized',
] as const;
const PERIOD_UPDATE_INSPECT_DATA_KEYS = [
  'project_id', 'update', 'fact_snapshot', 'fact_snapshot_coverage', 'as_of', 'execution_authorized',
] as const;
const PERIOD_UPDATE_FACT_SNAPSHOT_KEYS = [
  'verified', 'completed_without_independent_review', 'waits_opened', 'waits_cleared',
  'review_returned', 'delivered_unverified', 'decisions_made',
] as const;
const PERIOD_UPDATE_WAIT_KINDS = new Set(['decision', 'dependency', 'review', 'external', 'unknown']);

function isPlainRecord(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === 'object' && !Array.isArray(value);
}

function hasExactKeys(value: unknown, keys: readonly string[]): value is Record<string, unknown> {
  return isPlainRecord(value) && Object.keys(value).length === keys.length
    && keys.every((key) => Object.prototype.hasOwnProperty.call(value, key));
}

function isBoundedId(value: unknown): value is string {
  return typeof value === 'string' && value.length > 0 && value.length <= 128 && !/[\x00-\x1f\x7f]/.test(value);
}

function isBoundedCursor(value: unknown): value is string {
  return typeof value === 'string' && value.length > 0 && value.length <= 4096 && !/[\x00-\x1f\x7f]/.test(value);
}

function validCalendarDate(value: unknown): value is string {
  if (typeof value !== 'string' || !/^\d{4}-\d{2}-\d{2}$/.test(value)) return false;
  const year = Number(value.slice(0, 4));
  const month = Number(value.slice(5, 7));
  const day = Number(value.slice(8, 10));
  const leap = year % 4 === 0 && (year % 100 !== 0 || year % 400 === 0);
  const days = [0, 31, leap ? 29 : 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31][month] ?? 0;
  return year >= 1 && month >= 1 && month <= 12 && day >= 1 && day <= days;
}

function validPeriodKey(kind: unknown, key: unknown): boolean {
  if (typeof key !== 'string' || key.length === 0 || key.length > 64 || /[\p{Cc}]/u.test(key)) return false;
  if (kind === 'week') return /^\d{4}-W(0[1-9]|[1-4]\d|5[0-3])$/.test(key);
  if (kind === 'month') return /^\d{4}-(0[1-9]|1[0-2])$/.test(key);
  return false;
}

function validBridgeFreshness(value: unknown): boolean {
  if (!hasExactKeys(value, ['report_received', 'authoritative', 'page_read'])) return false;
  return ['report_received', 'authoritative', 'page_read'].every((plane) => {
    const entry = value[plane];
    return isPlainRecord(entry) && typeof entry.status === 'string' && PLANE_STATES.has(entry.status);
  });
}

function validBridgeWrapper(value: unknown): value is Record<string, unknown> {
  if (!isPlainRecord(value) || value.ok !== true || !validBridgeFreshness(value.freshness)) return false;
  const allowed = new Set(['ok', 'result', 'query', 'freshness']);
  return Object.keys(value).every((key) => allowed.has(key))
    && Object.prototype.hasOwnProperty.call(value, 'result');
}

function validPeriodUpdateEnvelope(result: unknown): result is Record<string, unknown> {
  return hasExactKeys(result, PERIOD_UPDATE_ENVELOPE_KEYS)
    && result.protocol_version === 1
    && result.scope_id === 'main'
    && result.selection_basis === 'project'
    && isBoundedId(result.coordinator_epoch)
    && isBoundedId(result.project_status)
    && isBoundedId(result.source_snapshot_id)
    && typeof result.project_revision === 'string' && /^(0|[1-9]\d*)$/.test(result.project_revision)
    && Number.isSafeInteger(Number(result.project_revision))
    && isPlainRecord(result.data);
}

function validPeriodUpdateAsOf(asOf: unknown, result: Record<string, unknown>): boolean {
  return hasExactKeys(asOf, PERIOD_UPDATE_AS_OF_KEYS)
    && typeof asOf.at_unix_ms === 'number' && Number.isSafeInteger(asOf.at_unix_ms) && asOf.at_unix_ms >= 0
    && asOf.source_snapshot_id === result.source_snapshot_id
    && asOf.project_revision === result.project_revision;
}

function validPeriodUpdateRecord(item: unknown, requireCurrent: boolean): item is Record<string, unknown> {
  if (!hasExactKeys(item, PERIOD_UPDATE_RECORD_KEYS)
    || !isBoundedId(item.update_id)
    || (item.period_kind !== 'week' && item.period_kind !== 'month')
    || !validPeriodKey(item.period_kind, item.period_key)
    || !validCalendarDate(item.starts_on)
    || !validCalendarDate(item.ends_on)
    || item.ends_on < item.starts_on
    || (item.status !== 'ok' && item.status !== 'risk' && item.status !== 'off')
    || typeof item.summary !== 'string' || item.summary.trim().length === 0
    || Array.from(item.summary).length > 200 || /[\p{Cc}]/u.test(item.summary)
    || !hasExactKeys(item.author, PERIOD_UPDATE_AUTHOR_KEYS)
    || !isBoundedId(item.author.person_id)
    || typeof item.author.display_name !== 'string' || item.author.display_name.trim().length === 0
    || Array.from(item.author.display_name).length > 128 || /[\p{Cc}]/u.test(item.author.display_name)
    || typeof item.recorded_at_unix_ms !== 'number' || !Number.isSafeInteger(item.recorded_at_unix_ms)
    || item.recorded_at_unix_ms < 0
    || typeof item.version !== 'string' || !/^[1-9]\d*$/.test(item.version)
    || !Number.isSafeInteger(Number(item.version))
    || !(item.fact_snapshot_ref === null || isBoundedId(item.fact_snapshot_ref))
    || (requireCurrent ? item.current !== true : typeof item.current !== 'boolean')) {
    return false;
  }
  return item.version === '1'
    ? item.supersedes_update_id === null
    : isBoundedId(item.supersedes_update_id) && item.supersedes_update_id !== item.update_id;
}

function validCountedWorkIds(value: unknown, unique: boolean): boolean {
  if (!hasExactKeys(value, ['count', 'work_ids'])
    || !Array.isArray(value.work_ids)
    || typeof value.count !== 'number' || !Number.isSafeInteger(value.count)
    || value.count !== value.work_ids.length) {
    return false;
  }
  for (let i = 0; i < value.work_ids.length; i += 1) {
    const id = value.work_ids[i];
    if (!isBoundedId(id)) return false;
    if (i > 0 && (unique ? (value.work_ids[i - 1] as string) >= id : (value.work_ids[i - 1] as string) > id)) {
      return false;
    }
  }
  return true;
}

function validWaitMetric(value: unknown): boolean {
  if (!hasExactKeys(value, ['count', 'kinds'])
    || !Array.isArray(value.kinds)
    || typeof value.count !== 'number' || !Number.isSafeInteger(value.count) || value.count < 0) {
    return false;
  }
  const seenKinds = new Set<string>();
  let total = 0;
  for (const entry of value.kinds) {
    if (!hasExactKeys(entry, ['kind', 'count', 'work_ids'])
      || typeof entry.kind !== 'string' || !PERIOD_UPDATE_WAIT_KINDS.has(entry.kind) || seenKinds.has(entry.kind)
      || !Array.isArray(entry.work_ids) || entry.work_ids.length === 0
      || typeof entry.count !== 'number' || !Number.isSafeInteger(entry.count)
      || entry.count !== entry.work_ids.length) {
      return false;
    }
    for (let i = 0; i < entry.work_ids.length; i += 1) {
      const id = entry.work_ids[i];
      if (!isBoundedId(id)) return false;
      if (i > 0 && (entry.work_ids[i - 1] as string) > id) return false;
    }
    seenKinds.add(entry.kind);
    total += entry.count;
  }
  return total === value.count;
}

function validPeriodUpdateFactSnapshot(snapshot: unknown): boolean {
  return hasExactKeys(snapshot, PERIOD_UPDATE_FACT_SNAPSHOT_KEYS)
    && validCountedWorkIds(snapshot.verified, true)
    && validCountedWorkIds(snapshot.completed_without_independent_review, true)
    && validWaitMetric(snapshot.waits_opened)
    && validWaitMetric(snapshot.waits_cleared)
    && validCountedWorkIds(snapshot.review_returned, true)
    && validCountedWorkIds(snapshot.delivered_unverified, true)
    && validCountedWorkIds(snapshot.decisions_made, false);
}

function validPeriodUpdateFactSnapshotCoverage(coverage: unknown): boolean {
  return hasExactKeys(coverage, ['complete', 'hidden_work_count'])
    && typeof coverage.complete === 'boolean'
    && typeof coverage.hidden_work_count === 'number' && Number.isSafeInteger(coverage.hidden_work_count)
    && coverage.hidden_work_count >= 0
    && coverage.complete === (coverage.hidden_work_count === 0);
}

function comparePeriodUpdateKeyDesc(left: Record<string, unknown>, right: Record<string, unknown>): number {
  for (const field of ['starts_on', 'ends_on', 'period_key', 'update_id'] as const) {
    if (left[field] !== right[field]) return (left[field] as string) > (right[field] as string) ? -1 : 1;
  }
  return 0;
}

export function validatePeriodUpdatesResponse(
  incoming: unknown,
  project: string,
  cursor: string | null = null,
): incoming is TeamPeriodUpdatesResponse {
  if (!validBridgeWrapper(incoming) || !validPeriodUpdateEnvelope(incoming.result)) return false;
  const data = incoming.result.data as Record<string, unknown>;
  if (!hasExactKeys(data, PERIOD_UPDATE_LIST_DATA_KEYS)
    || data.project_id !== project
    || !Array.isArray(data.items) || data.items.length > 100
    || !(data.next_cursor === null || isBoundedCursor(data.next_cursor))
    || (cursor !== null && data.next_cursor === cursor)
    || typeof data.truncated !== 'boolean' || data.truncated !== (data.next_cursor !== null)
    || data.execution_authorized !== false
    || !validPeriodUpdateAsOf(data.as_of, incoming.result)) {
    return false;
  }
  const seenIds = new Set<string>();
  for (let i = 0; i < data.items.length; i += 1) {
    const item = data.items[i];
    if (!validPeriodUpdateRecord(item, true)) return false;
    const id = item.update_id as string;
    if (seenIds.has(id)) return false;
    if (i > 0 && comparePeriodUpdateKeyDesc(data.items[i - 1] as Record<string, unknown>, item) >= 0) return false;
    seenIds.add(id);
  }
  return true;
}

export function validatePeriodUpdateResponse(
  incoming: unknown,
  project: string,
  updateId: string,
): incoming is TeamPeriodUpdateResponse {
  if (!validBridgeWrapper(incoming) || !validPeriodUpdateEnvelope(incoming.result)) return false;
  const data = incoming.result.data as Record<string, unknown>;
  if (!hasExactKeys(data, PERIOD_UPDATE_INSPECT_DATA_KEYS)
    || data.project_id !== project
    || data.execution_authorized !== false
    || !validPeriodUpdateAsOf(data.as_of, incoming.result)
    || !validPeriodUpdateRecord(data.update, false)
    || data.update.update_id !== updateId) {
    return false;
  }
  if (data.update.fact_snapshot_ref === null) {
    return data.fact_snapshot === null && data.fact_snapshot_coverage === null;
  }
  return validPeriodUpdateFactSnapshot(data.fact_snapshot)
    && validPeriodUpdateFactSnapshotCoverage(data.fact_snapshot_coverage);
}

/**
 * Project period updates list (AWR-PPD-012): reads `/api/team/updates` with strict
 * reply-shape validation, observation-contract authoritative ordering, and immediate
 * revocation clearing. Malformed replies throw `InvalidResponse` (partial-failure path)
 * without polluting the query cache or sibling queries.
 */
export function periodUpdatesQueryOptions(
  project: string | null,
  cursor: string | null = null,
  revoked = false,
) {
  return {
    queryKey: queryKeys.periodUpdates(project, cursor),
    queryFn: async ({ signal, client }: { signal: AbortSignal; client: QueryClient }) => {
      const state = revocationState(client);
      if (revoked || state.revoked) throw new DOMException('Observation access revoked', 'AbortError');
      const projectKey = project ?? '';
      const orderingKey = `${projectKey}\0${cursor ?? ''}`;
      const ordering = state.periodUpdates.get(orderingKey) ?? {};
      state.periodUpdates.set(orderingKey, ordering);
      signal.throwIfAborted();
      const params = new URLSearchParams({ project: projectKey });
      if (cursor !== null) params.set('cursor', cursor);
      const incoming = await apiGet<TeamPeriodUpdatesResponse>(`/api/team/updates?${params.toString()}`);
      signal.throwIfAborted();
      if (state.revoked) throw new DOMException('Observation access revoked', 'AbortError');
      if (!validatePeriodUpdatesResponse(incoming, projectKey, cursor)) {
        throw new ApiError('InvalidResponse', 'Period updates observations failed contract validation');
      }
      const held = ordering.held;
      if (held) {
        const verdict = compareAuthoritative(held.freshness.authoritative, incoming.freshness.authoritative);
        if (verdict === 'keep') return held;
        if (verdict === 'incomparable') return { ...held, ordering: 'incomparable' as const };
      }
      ordering.held = incoming;
      return incoming;
    },
    enabled: Boolean(project) && !revoked,
    ...readQueryDefaults,
    refetchInterval: POLL_INTERVAL_MS,
  } satisfies UseQueryOptions<TeamPeriodUpdatesResponse>;
}

/**
 * Single project period update inspection (AWR-PPD-012): reads `/api/team/update`
 * with strict reply-shape validation (including PPD-011 immutable fact snapshots),
 * observation-contract authoritative ordering, and immediate revocation clearing.
 */
export function periodUpdateQueryOptions(
  project: string | null,
  updateId: string | null,
  revoked = false,
) {
  return {
    queryKey: queryKeys.periodUpdate(project, updateId),
    queryFn: async ({ signal, client }: { signal: AbortSignal; client: QueryClient }) => {
      const state = revocationState(client);
      if (revoked || state.revoked) throw new DOMException('Observation access revoked', 'AbortError');
      const projectKey = project ?? '';
      const idKey = updateId ?? '';
      const orderingKey = `${projectKey}\0${idKey}`;
      const ordering = state.periodUpdate.get(orderingKey) ?? {};
      state.periodUpdate.set(orderingKey, ordering);
      signal.throwIfAborted();
      const params = new URLSearchParams({ project: projectKey, id: idKey });
      const incoming = await apiGet<TeamPeriodUpdateResponse>(`/api/team/update?${params.toString()}`);
      signal.throwIfAborted();
      if (state.revoked) throw new DOMException('Observation access revoked', 'AbortError');
      if (!validatePeriodUpdateResponse(incoming, projectKey, idKey)) {
        throw new ApiError('InvalidResponse', 'Period update observation failed contract validation');
      }
      const held = ordering.held;
      if (held) {
        const verdict = compareAuthoritative(held.freshness.authoritative, incoming.freshness.authoritative);
        if (verdict === 'keep') return held;
        if (verdict === 'incomparable') return { ...held, ordering: 'incomparable' as const };
      }
      ordering.held = incoming;
      return incoming;
    },
    enabled: Boolean(project && updateId) && !revoked,
    ...readQueryDefaults,
    refetchInterval: POLL_INTERVAL_MS,
  } satisfies UseQueryOptions<TeamPeriodUpdateResponse>;
}

interface OverviewModelOrdering {
  held?: TeamOverviewModelResponse;
}

interface PortfolioOrdering {
  held?: TeamPortfolioResponse;
}

interface PeriodUpdatesOrdering {
  held?: TeamPeriodUpdatesResponse;
}

interface PeriodUpdateOrdering {
  held?: TeamPeriodUpdateResponse;
}

interface RevocationState {
  revoked: boolean;
  listeners: Set<() => void>;
  overviewModels: Map<string, OverviewModelOrdering>;
  portfolio: PortfolioOrdering;
  periodUpdates: Map<string, PeriodUpdatesOrdering>;
  periodUpdate: Map<string, PeriodUpdateOrdering>;
}

const revocationStates = new WeakMap<QueryClient, RevocationState>();

function revocationState(queryClient: QueryClient) {
  let state = revocationStates.get(queryClient);
  if (!state) {
    state = {
      revoked: false,
      listeners: new Set(),
      overviewModels: new Map(),
      portfolio: {},
      periodUpdates: new Map(),
      periodUpdate: new Map(),
    };
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
  state.overviewModels.clear();
  state.portfolio.held = undefined;
  state.periodUpdates.clear();
  state.periodUpdate.clear();
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
