import { QueryClientProvider } from '@tanstack/react-query';
import { renderHook, waitFor } from '@testing-library/react';
import type { ReactNode } from 'react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { DEMO_PROJECT_WORKS, demoWorkSnapshot } from '../test-utils';
import { createAppQueryClient } from './queries';
import { useWorkSnapshots } from './work-snapshots';

afterEach(() => {
  vi.unstubAllGlobals();
});

function setup(revoked: boolean, works = DEMO_PROJECT_WORKS) {
  const fetchMock = vi.fn(async (_input: RequestInfo | URL, init?: RequestInit) => {
    const request = JSON.parse(String(init?.body)) as { query: { work_id?: string } };
    return new Response(JSON.stringify(demoWorkSnapshot(request.query.work_id)), { status: 200, headers: { 'content-type': 'application/json' } });
  });
  vi.stubGlobal('fetch', fetchMock);
  const client = createAppQueryClient();
  const wrapper = ({ children }: { children: ReactNode }) => <QueryClientProvider client={client}>{children}</QueryClientProvider>;
  const hook = renderHook(() => useWorkSnapshots('demo', works, revoked), { wrapper });
  return { fetchMock, hook, client };
}

describe('useWorkSnapshots', () => {
  it('reads each listed work once, on the list cadence, in the order of the list', async () => {
    const { fetchMock, hook, client } = setup(false);
    await waitFor(() => expect(hook.result.current.every((snapshot) => !snapshot.isPending)).toBe(true));
    expect(hook.result.current).toHaveLength(DEMO_PROJECT_WORKS.length);
    const ids = fetchMock.mock.calls.map(([, init]) => (JSON.parse(String(init?.body)) as { query: { work_id: string } }).query.work_id);
    expect(ids.sort()).toEqual(DEMO_PROJECT_WORKS.map((work) => work.key).sort());
    expect(hook.result.current.map((snapshot) => snapshot.data?.result.data.work_id)).toEqual(DEMO_PROJECT_WORKS.map((work) => work.key));
    const intervals = client.getQueryCache().getAll().map((query) => ('refetchInterval' in query.options ? query.options.refetchInterval : undefined));
    expect(intervals).toEqual(DEMO_PROJECT_WORKS.map(() => 60000));
  });

  it('reads nothing at all while the observation is revoked', async () => {
    const { fetchMock, hook } = setup(true);
    await new Promise((resolve) => setTimeout(resolve, 200));
    expect(fetchMock).not.toHaveBeenCalled();
    expect(hook.result.current.every((snapshot) => snapshot.isPending)).toBe(true);
  });

  it('gives nothing back and reads nothing for an empty list', async () => {
    const { fetchMock, hook } = setup(false, []);
    await new Promise((resolve) => setTimeout(resolve, 100));
    expect(hook.result.current).toEqual([]);
    expect(fetchMock).not.toHaveBeenCalled();
  });
});
