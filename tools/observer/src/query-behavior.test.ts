import { describe, expect, it, vi } from 'vitest';
import { ApiError } from './lib/api';
import { createAppQueryClient, overviewQueryOptions, POLL_INTERVAL_MS, queryKeys, readQueryDefaults, workNextQueryOptions } from './lib/queries';

describe('observation query behavior', () => {
  it('uses a five-second polling interval rather than the per-read timeout', () => {
    expect(POLL_INTERVAL_MS).toBe(5000);
    expect(readQueryDefaults.refetchInterval).toBe(POLL_INTERVAL_MS);
  });

  it('retries a transient read failure once', async () => {
    const queryClient = createAppQueryClient();
    let attempts = 0;
    const result = await queryClient.fetchQuery({
      queryKey: ['team', 'transient-retry'],
      queryFn: async () => {
        attempts += 1;
        if (attempts === 1) throw new ApiError('ReadTimeout', 'slow');
        return { ok: true };
      },
    });
    expect(result).toEqual({ ok: true });
    expect(attempts).toBe(2);
  });

  it('does not retry an auth failure', async () => {
    const queryClient = createAppQueryClient();
    let attempts = 0;
    await expect(queryClient.fetchQuery({
      queryKey: ['team', 'auth-no-retry'],
      queryFn: async () => {
        attempts += 1;
        throw new ApiError('Unauthenticated', 'revoked');
      },
    })).rejects.toThrow('revoked');
    expect(attempts).toBe(1);
  });

  it('encodes the selected project in the overview query string', () => {
    const options = overviewQueryOptions('demo /?&=');
    expect(options.queryFn).toBeDefined();
    const fetchMock = vi.fn(async (_input: RequestInfo | URL) => new Response(JSON.stringify({ ok: true }), {
      status: 200,
      headers: { 'content-type': 'application/json' },
    }));
    vi.stubGlobal('fetch', fetchMock);
    return Promise.resolve(options.queryFn!()).then(() => {
      expect(fetchMock.mock.calls[0][0]).toBe('/api/team/overview?project=demo%20%2F%3F%26%3D');
    });
  });

  it('keeps project query keys isolated by scope', () => {
    expect(queryKeys.overview('demo')).not.toEqual(queryKeys.overview('docs'));
    expect(queryKeys.workNext('demo')).not.toEqual(queryKeys.workNext('docs'));
    expect(queryKeys.workSnapshot('demo', 'W1', 'WS1')).not.toEqual(queryKeys.workSnapshot('demo', 'W2', 'WS1'));
  });

  it('uses the generic read route and five-second defaults for work.next', async () => {
    const options = workNextQueryOptions('demo');
    const fetchMock = vi.fn(async (_input: RequestInfo | URL, _init?: RequestInit) => new Response(JSON.stringify({ ok: true }), {
      status: 200,
      headers: { 'content-type': 'application/json' },
    }));
    vi.stubGlobal('fetch', fetchMock);
    await options.queryFn!({ signal: new AbortController().signal } as never);
    expect(fetchMock.mock.calls[0][0]).toBe('/api/team/read');
    expect(JSON.parse(String(fetchMock.mock.calls[0][1]?.body))).toEqual({
      project: 'demo',
      query: { protocol_version: 1, op: 'work.next' },
    });
    expect(options.refetchInterval).toBe(POLL_INTERVAL_MS);
    expect(options.refetchOnWindowFocus).toBe(true);
  });
});
