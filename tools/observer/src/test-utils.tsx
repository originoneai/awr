import { QueryClientProvider } from '@tanstack/react-query';
import { render } from '@testing-library/react';
import { createMemoryRouter, RouterProvider } from 'react-router';
import { vi } from 'vitest';
import { createAppQueryClient } from './lib/queries';
import { routes } from './router';

export const DEMO_PROJECTS = {
  ok: true,
  projects: [
    { key: 'demo', title: 'Team mainline', role: 'project_admin' },
    { key: 'docs', title: 'Docs stream', role: 'reviewer' },
  ],
  session: { session_id: 'demo-anonymous', expires_at_ms: Date.now() + 3600000 },
  view: 'team',
};

export const DEMO_OVERVIEW = {
  ok: true,
  project: 'demo',
  works: [],
  workstreams: [],
  members: [],
  handoffs: [],
  reviews: [],
  coverage: {
    scope: { status: 'succeeded', enumerated: 0, truncated: false, next_cursor: null, read_at_unix_ms: 1761840000000 },
    streams: [],
    works_enumerated: 0,
    works_total: 0,
    succeeded: 0,
    failed: 0,
    truncated: false,
  },
  freshness: {
    report_received: { status: 'not_measured' },
    authoritative: { status: 'not_measured' },
    page_read: { status: 'measured', at_unix_ms: 1761840000000 },
  },
  schema: 'awr-team-web-loop-fixture/v1',
};

type FetchStubMap = Record<string, unknown>;

/** Stub window.fetch with per-path JSON envelopes (path prefix match). */
export function stubFetch(map: FetchStubMap) {
  const fetchMock = vi.fn(async (input: RequestInfo | URL) => {
    const url = String(input);
    for (const [prefix, payload] of Object.entries(map)) {
      if (url.includes(prefix)) {
        return new Response(JSON.stringify(payload), {
          status: 200,
          headers: { 'content-type': 'application/json' },
        });
      }
    }
    return new Response(JSON.stringify({ ok: false, error: { code: 'NoRoute', message: url } }), {
      status: 200,
      headers: { 'content-type': 'application/json' },
    });
  });
  vi.stubGlobal('fetch', fetchMock);
  return fetchMock;
}

export function renderApp(initialPath: string) {
  const queryClient = createAppQueryClient({ retry: false });
  const router = createMemoryRouter(routes, { initialEntries: [initialPath] });
  const utils = render(
    <QueryClientProvider client={queryClient}>
      <RouterProvider router={router} />
    </QueryClientProvider>,
  );
  return { router, queryClient, ...utils };
}
