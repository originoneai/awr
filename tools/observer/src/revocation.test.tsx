import { act, screen } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { DEMO_OVERVIEW, DEMO_PROJECTS, renderApp } from './test-utils';

function revocationFetch(calls: { count: number }, revoked: { value: boolean }) {
  const fetchMock = vi.fn(async (input: RequestInfo | URL) => {
    calls.count += 1;
    const url = String(input);
    const payload = revoked.value
      ? { ok: false, error: { code: 'Unauthenticated', message: 'revoked' } }
      : url.includes('/api/team/projects')
        ? DEMO_PROJECTS
        : url.includes('/api/team/overview')
          ? DEMO_OVERVIEW
          : { ok: false, error: { code: 'NoRoute', message: url } };
    return new Response(JSON.stringify(payload), {
      status: 200,
      headers: { 'content-type': 'application/json' },
    });
  });
  vi.stubGlobal('fetch', fetchMock);
  return fetchMock;
}

async function advance(ms: number) {
  await act(async () => {
    await vi.advanceTimersByTimeAsync(ms);
  });
}

afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

describe.each([
  { path: '/projects', restrictedText: /Team mainline/ },
  { path: '/workbench?scope=demo', restrictedText: /Team mainline/ },
])('rendered revocation on $path', ({ path, restrictedText }) => {
  it('clears restricted content, shows forbidden, and stops reads for 10 seconds', async () => {
    vi.useFakeTimers();
    const calls = { count: 0 };
    const revoked = { value: false };
    revocationFetch(calls, revoked);
    const { queryClient } = renderApp(path);

    await advance(0);
    expect(screen.getByText(restrictedText)).toBeTruthy();
    expect(screen.getByTestId(path.startsWith('/projects') ? 'page-projects' : 'freshness-badge')).toBeTruthy();
    revoked.value = true;
    const callsAtRevocation = calls.count;

    await advance(10_000);

    expect(screen.queryByText(restrictedText)).toBeNull();
    expect(screen.getByTestId('state-forbidden')).toBeTruthy();
    expect(screen.queryByTestId('freshness-badge')).toBeNull();
    expect(queryClient.getQueryCache().getAll().every((query) => query.state.data === undefined)).toBe(true);
    expect(calls.count - callsAtRevocation).toBeLessThanOrEqual(2);

    const callsAfterForbidden = calls.count;
    await advance(10_000);
    expect(calls.count).toBe(callsAfterForbidden);
    expect(screen.getByTestId('state-forbidden')).toBeTruthy();
  }, 20_000);
});

describe('an in-flight read at the moment of revocation', () => {
  it('never lands restricted data when its response is released after the revocation', async () => {
    vi.useFakeTimers();
    // The overview's first response stays in flight until we release it.
    let releaseOverview!: (response: Response) => void;
    const overviewGate = new Promise<Response>((resolve) => {
      releaseOverview = resolve;
    });
    const restrictedOverview = {
      ...DEMO_OVERVIEW,
      works: [{ key: 'secret-1', title: 'Secret restricted work' }],
      coverage: { ...DEMO_OVERVIEW.coverage, works_enumerated: 1, works_total: 1, succeeded: 1 },
    };
    const revoked = { value: false };
    const authFailure = () =>
      new Response(JSON.stringify({ ok: false, error: { code: 'Unauthenticated', message: 'revoked' } }), {
        status: 200,
        headers: { 'content-type': 'application/json' },
      });
    const fetchMock = vi.fn(async (input: RequestInfo | URL) => {
      const url = String(input);
      if (revoked.value) return authFailure();
      if (url.includes('/api/team/overview')) return overviewGate;
      if (url.includes('/api/team/projects')) {
        return new Response(JSON.stringify(DEMO_PROJECTS), {
          status: 200,
          headers: { 'content-type': 'application/json' },
        });
      }
      return new Response(JSON.stringify({ ok: false, error: { code: 'NoRoute', message: url } }), {
        status: 200,
        headers: { 'content-type': 'application/json' },
      });
    });
    vi.stubGlobal('fetch', fetchMock);
    const { queryClient } = renderApp('/workbench?scope=demo');

    await advance(0);
    // The projects read has landed; the overview read is still in flight.
    expect(screen.getByText(/Team mainline/)).toBeTruthy();
    revoked.value = true;
    // The next poll fails with Unauthenticated and revokes the session.
    await advance(5_000);
    expect(screen.getByTestId('state-forbidden')).toBeTruthy();

    // Only now does the in-flight overview answer, carrying restricted data.
    await act(async () => {
      releaseOverview(
        new Response(JSON.stringify(restrictedOverview), {
          status: 200,
          headers: { 'content-type': 'application/json' },
        }),
      );
    });
    await advance(10_000);

    // Its data never reaches the screen, and the cancelled query never
    // stores it either (this is what cancelQueries protects).
    expect(screen.queryByText(/Secret restricted work/)).toBeNull();
    expect(screen.queryByTestId('coverage-summary')).toBeNull();
    expect(screen.getByTestId('state-forbidden')).toBeTruthy();
    expect(queryClient.getQueryCache().getAll().every((query) => query.state.data === undefined)).toBe(true);
  }, 20_000);
});

describe('revokeObservationReads cancels in-flight reads', () => {
  it('a consumer holding the in-flight promise never receives restricted data', async () => {
    const { createAppQueryClient, revokeObservationReads } = await import('./lib/queries');
    const queryClient = createAppQueryClient({ retry: false });
    let release!: (value: unknown) => void;
    const gate = new Promise((resolve) => {
      release = resolve;
    });
    const pending = queryClient.fetchQuery({
      queryKey: ['team', 'overview', 'demo'],
      queryFn: () => gate,
      retry: false,
    });
    revokeObservationReads(queryClient);
    release({ works: [{ key: 'secret-1', title: 'Secret restricted work' }] });
    // The in-flight read rejects as cancelled, so no consumer can receive
    // restricted data after the revocation. Note for fault injection:
    // deleting `cancelQueries()` alone is an EQUIVALENT mutant here — in
    // TanStack Query v5 `QueryCache.remove()` calls `Query.destroy()`, which
    // itself cancels the retryer (query-core query.js destroy → cancel), so
    // `removeQueries()` already cancels in-flight reads. Both APIs are
    // present in revokeObservationReads; this test pins the behaviour.
    await expect(pending).rejects.toThrow();
    expect(queryClient.getQueryData(['team', 'overview', 'demo'])).toBeUndefined();
  });
});
