import { describe, expect, it } from 'vitest';
import { ApiError } from './api';
import {
  activityQueryOptions,
  createAppQueryClient,
  capabilitiesQueryOptions,
  overviewQueryOptions,
  POLL_INTERVAL_MS,
  projectsQueryOptions,
  queryKeys,
  readQueryDefaults,
  workSnapshotQueryOptions,
} from './queries';

describe('TanStack Query read configuration (observation contract bounds)', () => {
  it('polls at the documented cadence', () => {
    expect(readQueryDefaults.refetchInterval).toBe(POLL_INTERVAL_MS);
  });

  it('refetches on window focus without retaining data across a new query key', () => {
    expect(readQueryDefaults.refetchOnWindowFocus).toBe(true);
    expect('placeholderData' in readQueryDefaults).toBe(false);
  });

  it('namespaces query keys per tab/resource', () => {
    expect(queryKeys.projects).toEqual(['team', 'projects']);
    expect(queryKeys.overview('demo')).toEqual(['team', 'overview', 'demo']);
    expect(projectsQueryOptions().queryKey).toEqual(['team', 'projects']);
    expect(overviewQueryOptions('demo').queryKey).toEqual(['team', 'overview', 'demo']);
  });

  it('uses detail and list snapshot cadences while retaining focus refreshes', () => {
    const options = overviewQueryOptions('demo');
    expect(options.refetchInterval).toBe(30000);
    expect(projectsQueryOptions().refetchInterval).toBe(30000);
    expect(workSnapshotQueryOptions('demo', 'W1', 'WS1').refetchInterval).toBe(POLL_INTERVAL_MS);
    expect(workSnapshotQueryOptions('demo', 'W1', 'WS1', false, 'list').refetchInterval).toBe(60000);
    expect(options.refetchOnWindowFocus).toBe(true);
    expect(projectsQueryOptions().refetchOnWindowFocus).toBe(true);
    expect(workSnapshotQueryOptions('demo', 'W1', 'WS1').refetchOnWindowFocus).toBe(true);
    expect('placeholderData' in options).toBe(false);
  });

  it('polls embedded activity summaries at the list cadence', () => {
    const options = activityQueryOptions('demo');
    expect(options.refetchInterval).toBe(60000);
    expect(options.refetchOnWindowFocus).toBe(true);
  });

  it('shares the non-polling capabilities options', () => {
    const options = capabilitiesQueryOptions('demo');
    expect(options.queryKey).toEqual(['team', 'capabilities', 'demo']);
    expect(options.staleTime).toBe(Infinity);
    expect(options.refetchInterval).toBe(false);
    expect(options.refetchOnWindowFocus).toBe(false);
    expect(capabilitiesQueryOptions('demo', true).enabled).toBe(false);
  });

  it('does not fetch the overview without a selected project or after revocation', () => {
    expect(overviewQueryOptions(null).enabled).toBe(false);
    expect(overviewQueryOptions('demo', true).enabled).toBe(false);
    expect(projectsQueryOptions(true).enabled).toBe(false);
  });
});

describe('auth revocation clears restricted content', () => {
  it('clears every cached query on auth error codes', async () => {
    const queryClient = createAppQueryClient({ retry: false });
    queryClient.setQueryData(queryKeys.projects, { ok: true });
    queryClient.setQueryData(queryKeys.overview('demo'), { ok: true });
    await expect(
      queryClient.fetchQuery({
        queryKey: ['team', 'failing'],
        queryFn: () => Promise.reject(new ApiError('Forbidden', 'access denied')),
        retry: false,
      }),
    ).rejects.toThrow('access denied');
    expect(queryClient.getQueryCache().getAll()).toHaveLength(0);
  });

  it('revokes globally on activity Unauthenticated but not on an object Forbidden', async () => {
    const expiredClient = createAppQueryClient({ retry: false });
    expiredClient.setQueryData(queryKeys.projects, { ok: true });
    await expect(expiredClient.fetchQuery({
      queryKey: queryKeys.activity('demo'),
      queryFn: () => Promise.reject(new ApiError('Unauthenticated', 'expired')),
      retry: false,
    })).rejects.toThrow('expired');
    expect(expiredClient.getQueryCache().getAll()).toHaveLength(0);

    const forbiddenClient = createAppQueryClient({ retry: false });
    forbiddenClient.setQueryData(queryKeys.projects, { ok: true });
    await expect(forbiddenClient.fetchQuery({
      queryKey: queryKeys.activity('demo'),
      queryFn: () => Promise.reject(new ApiError('Forbidden', 'object denied')),
      retry: false,
    })).rejects.toThrow('object denied');
    expect(forbiddenClient.getQueryCache().getAll().length).toBeGreaterThan(0);
  });

  it('keeps cached queries on non-auth failures', async () => {
    const queryClient = createAppQueryClient({ retry: false });
    queryClient.setQueryData(queryKeys.projects, { ok: true });
    await expect(
      queryClient.fetchQuery({
        queryKey: ['team', 'failing'],
        queryFn: () => Promise.reject(new ApiError('ReadTimeout', 'slow')),
        retry: false,
      }),
    ).rejects.toThrow('slow');
    expect(queryClient.getQueryCache().getAll().length).toBeGreaterThan(0);
  });
});
