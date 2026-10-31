import { act, screen } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import {
  DEMO_ACTIVITY,
  DEMO_PROJECT_OVERVIEW,
  DEMO_PROJECTS,
  DEMO_WORK_NEXT,
  demoWorkSnapshot,
  renderApp,
} from './test-utils';

function revocationFetch(calls: { count: number }, revoked: { value: boolean }) {
  const fetchMock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
    calls.count += 1;
    const url = String(input);
    let payload: unknown = { ok: false, error: { code: 'NoRoute', message: url } };
    if (revoked.value) {
      payload = { ok: false, error: { code: 'Unauthenticated', message: 'revoked' } };
    } else if (url.includes('/api/team/projects')) {
      payload = DEMO_PROJECTS;
    } else if (url.includes('/api/team/overview')) {
      payload = DEMO_PROJECT_OVERVIEW;
    } else if (url.includes('/api/team/activity')) {
      payload = DEMO_ACTIVITY;
    } else if (url.includes('/api/team/read')) {
      const request = JSON.parse(String(init?.body)) as { query: { op: string; work_id?: string } };
      if (request.query.op === 'work.next') payload = DEMO_WORK_NEXT;
      if (request.query.op === 'work.snapshot') payload = demoWorkSnapshot(request.query.work_id);
    }
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
  { path: '/projects', restrictedText: /Team mainline/, pageTestId: 'page-projects' },
  { path: '/workbench?scope=demo', restrictedText: /Build the filter API/, pageTestId: 'page-workbench' },
  { path: '/projects/demo?tab=overview', restrictedText: /Build the filter API/, pageTestId: 'page-project' },
])('rendered revocation on $path', ({ path, restrictedText, pageTestId }) => {
  it('clears restricted content, shows forbidden, and stops reads for 10 seconds', async () => {
    vi.useFakeTimers();
    const calls = { count: 0 };
    const revoked = { value: false };
    revocationFetch(calls, revoked);
    const { queryClient } = renderApp(path);

    await advance(0);
    expect(screen.getAllByText(restrictedText)).not.toHaveLength(0);
    expect(screen.getByTestId(pageTestId)).toBeTruthy();
    revoked.value = true;
    const callsAtRevocation = calls.count;

    await advance(10_000);

    expect(screen.queryByText(restrictedText)).toBeNull();
    expect(screen.getByTestId('state-forbidden')).toBeTruthy();
    expect(queryClient.getQueryCache().getAll().every((query) => query.state.data === undefined)).toBe(true);
    expect(calls.count - callsAtRevocation).toBeLessThanOrEqual(2);

    const callsAfterForbidden = calls.count;
    await advance(10_000);
    expect(calls.count).toBe(callsAfterForbidden);
    expect(screen.getByTestId('state-forbidden')).toBeTruthy();
  }, 20_000);
});

describe('an in-flight work.next read at the moment of revocation', () => {
  it('never lands restricted rows when its response is released after revocation', async () => {
    vi.useFakeTimers();
    let releaseWorkNext!: (response: Response) => void;
    const workNextGate = new Promise<Response>((resolve) => {
      releaseWorkNext = resolve;
    });
    const restrictedWorkNext = {
      ...DEMO_WORK_NEXT,
      result: {
        ...DEMO_WORK_NEXT.result,
        data: {
          ...DEMO_WORK_NEXT.result.data,
          items: [{
            ...DEMO_WORK_NEXT.result.data.items[0],
            work_id: 'secret-1',
            title: 'Secret restricted work',
          }],
          resume: [],
        },
      },
    };
    const revoked = { value: false };
    const authFailure = () =>
      new Response(JSON.stringify({ ok: false, error: { code: 'Unauthenticated', message: 'revoked' } }), {
        status: 200,
        headers: { 'content-type': 'application/json' },
      });
    const fetchMock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = String(input);
      if (revoked.value) return authFailure();
      if (url.includes('/api/team/projects')) {
        return new Response(JSON.stringify(DEMO_PROJECTS), {
          status: 200,
          headers: { 'content-type': 'application/json' },
        });
      }
      if (url.includes('/api/team/read')) {
        const request = JSON.parse(String(init?.body)) as { query: { op: string } };
        if (request.query.op === 'work.next') return workNextGate;
      }
      return new Response(JSON.stringify({ ok: false, error: { code: 'NoRoute', message: url } }), {
        status: 200,
        headers: { 'content-type': 'application/json' },
      });
    });
    vi.stubGlobal('fetch', fetchMock);
    const { queryClient } = renderApp('/workbench?scope=demo');

    await advance(0);
    expect(screen.getByText(/Team mainline/)).toBeTruthy();
    revoked.value = true;
    await advance(5_000);
    expect(screen.getByTestId('state-forbidden')).toBeTruthy();

    await act(async () => {
      releaseWorkNext(
        new Response(JSON.stringify(restrictedWorkNext), {
          status: 200,
          headers: { 'content-type': 'application/json' },
        }),
      );
    });
    await advance(10_000);

    expect(screen.queryByText(/Secret restricted work/)).toBeNull();
    expect(screen.queryByTestId('workbench-table')).toBeNull();
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
    // TanStack Query's removeQueries also cancels; both explicit cancellation
    // and removal protect consumers from late restricted responses.
    await expect(pending).rejects.toThrow();
    expect(queryClient.getQueryData(['team', 'overview', 'demo'])).toBeUndefined();
  });
});
