import { describe, expect, it } from 'vitest';
import { ApiError } from './api';
import {
  createAppQueryClient,
  overviewQueryOptions,
  POLL_INTERVAL_MS,
  projectsQueryOptions,
  queryKeys,
  readQueryDefaults,
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

  it('applies the read defaults to wired queries', () => {
    const options = overviewQueryOptions('demo');
    expect(options.refetchInterval).toBe(POLL_INTERVAL_MS);
    expect(options.refetchOnWindowFocus).toBe(true);
    expect('placeholderData' in options).toBe(false);
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
