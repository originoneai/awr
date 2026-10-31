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

export const DEMO_WORK_ITEMS = [
  {
    navigation: 'resume',
    work_id: 'W1',
    workstream_id: 'WS1',
    state: 'claimed',
    title: 'Build the filter API',
    responsibility: { relation: 'owned_by_me', owner: 'lin', version: '1' },
    next_query: { op: 'work.observe', protocol_version: 1, work_id: 'W1', workstream_id: 'WS1' },
  },
  {
    navigation: 'prepare',
    work_id: 'W2',
    workstream_id: 'WS2',
    state: null,
    title: 'Prepare the support page',
    responsibility: { relation: 'pool', owner: null, version: '1' },
    next_query: { op: 'work.prepare', protocol_version: 1, work_id: 'W2', workstream_id: 'WS2' },
  },
];

export const DEMO_WORK_NEXT = {
  ok: true,
  result: {
    data: {
      identity: { actor_id: 'lin', role: 'developer', business_roles: ['developer'] },
      items: DEMO_WORK_ITEMS,
      resume: [],
      next_cursor: null,
      next_action: null,
      recheck_on: [],
      execution_authorized: false,
    },
    project_revision: '1',
    protocol_version: 1,
  },
  query: { protocol_version: 1, op: 'work.next' },
  freshness: {
    report_received: { status: 'not_measured' },
    authoritative: { status: 'not_measured' },
    page_read: { status: 'measured', at_unix_ms: 1761840000000 },
  },
};

export const DEMO_WORK_INBOX = {
  ok: true,
  result: {
    data: {
      items: [{
        item_key: 'inbox-1',
        title: 'Review the API handoff',
        title_truncated: false,
        work_id: 'INBOX1',
        workstream_id: 'WS1',
        contract_hash: 'contract-1',
        guidance: {
          code: 'review_required',
          action: { op: 'work.observe', note: 'Review the recorded handoff.', query: {} },
          because: ['A handoff needs attention'],
        },
        next_query: { op: 'work.observe', protocol_version: 1, work_id: 'INBOX1', workstream_id: 'WS1' },
      }],
      next_cursor: null,
      execution_authorized: false,
    },
    scope_id: 'main',
    source_snapshot_id: 'snapshot-1',
    coordinator_epoch: 'epoch-1',
    project_revision: '1',
    protocol_version: 1,
  },
  query: { protocol_version: 1, op: 'work.inbox' },
  freshness: DEMO_WORK_NEXT.freshness,
};

export function demoWorkSnapshot(workId = 'W1') {
  return {
    ok: true,
    result: {
      data: {
        work_id: workId,
        observation: {
          last_activity_at_unix_ms: 1761839900000,
          observed_at_unix_ms: 1761839999000,
          progress: null,
          reporting: { version: 1, client_reported_at_unix_ms: 1761839800000 },
          runtime: { state: 'claimed', work_version: '1' },
          responsibility: { relation: 'owned_by_me', owner: 'lin', version: '1' },
          session: null,
          claim: null,
        },
        visible_contract: {
          work_id: workId,
          goals: ['Keep support filters useful'],
          acceptance: ['Do not mutate input records'],
          required_dependencies: ['upstream-contract-v1'],
        },
        published_contract: null,
        required_specs: [],
        snapshot: { queried_at_unix_ms: 1761839999000 },
        guidance: {
          code: 'report_at_boundary',
          action: { op: 'session.checkpoint', note: 'Record the verified outcome.' },
        },
        next_step: null,
      },
      scope_id: 'main',
      source_snapshot_id: 'snapshot-1',
      coordinator_epoch: 'epoch-1',
      project_revision: '1',
      protocol_version: 1,
    },
    query: { protocol_version: 1, op: 'work.snapshot', work_id: workId },
    freshness: DEMO_WORK_NEXT.freshness,
  };
}

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
