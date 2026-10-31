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
