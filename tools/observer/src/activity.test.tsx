import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { DEMO_PROJECTS, renderApp } from './test-utils';

const SELF_ACTOR = 'observer-host-native-20261031T203834Z-6427d37b0df9-aadc36';
const UNKNOWN_MEANS = 'completion_not_recorded; inspect business request before retrying';

// Live replies: observer-activity-*.json and service-audit-*.json.
// Project-scope fixtures are derived from service source shape and run-09 rows,
// not a live reply. Their _note stays out of the rendered page and Git history.
function fixture(name: string) {
  return JSON.parse(readFileSync(resolve(`src/test-fixtures/activity/${name}`), 'utf8'));
}

type ActivityPayload = {
  data: {
    items: Array<Record<string, unknown>>;
    next_cursor?: unknown;
    scope?: string;
    [key: string]: unknown;
  };
  ok?: boolean;
};

type SetupOptions = {
  projectAudit?: boolean;
  development?: ActivityPayload;
  requests?: ActivityPayload;
  developmentPages?: ActivityPayload[];
  requestsPages?: ActivityPayload[];
};

function setupActivity({
  projectAudit = false,
  development = fixture('observer-activity-development.json'),
  requests = fixture('observer-activity-requests.json'),
  developmentPages,
  requestsPages,
}: SetupOptions = {}) {
  const calls: Array<{ url: string; params: URLSearchParams }> = [];
  const devPages = developmentPages ?? [development];
  const requestPages = requestsPages ?? [requests];
  const fetchMock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
    const url = String(input);
    if (url.includes('/api/team/activity')) {
      const parsed = new URL(url, 'http://localhost');
      const params = parsed.searchParams;
      calls.push({ url, params });
      const kind = params.get('kind');
      const pages = kind === 'requests' ? requestPages : devPages;
      const cursor = params.get('cursor');
      const index = cursor ? pages.findIndex((page) => page.data.next_cursor === cursor) + 1 : 0;
      const page = pages[index] ?? { data: { items: [], next_cursor: null, scope: pages[0].data.scope } };
      const items = page.data.items.filter((item) => (
        (!params.get('work_id') || item.work_id === params.get('work_id'))
        && (!params.get('member_actor_id') || item.actor_id === params.get('member_actor_id'))
      ));
      return json({ ok: true, data: { ...page.data, items } });
    }
    if (url.includes('/api/team/projects')) return json(DEMO_PROJECTS);
    if (url.includes('/api/team/read')) {
      const query = JSON.parse(String(init?.body)).query;
      if (query.op === 'capabilities') {
        return json({
          ok: true,
          result: {
            protocol: 'awr-team-workstream', protocol_version: 1,
            data: { identity: { actor_id: SELF_ACTOR, client_id: 'observer-client', role: 'reader', business_roles: ['observer'], can_read_project_audit: projectAudit },
          },
        } });
      }
    }
    return json({ ok: false, error: { code: 'NoRoute', message: url } });
  });
  vi.stubGlobal('fetch', fetchMock);
  return { calls };
}

function json(payload: unknown) {
  return new Response(JSON.stringify(payload), { status: 200, headers: { 'content-type': 'application/json' } });
}

function pagedDevelopment(items: Array<Record<string, unknown>>, nextCursor: unknown): ActivityPayload {
  const base = fixture('derived-audit-development-project-scope-run09.json');
  return { data: { ...base.data, items, next_cursor: nextCursor } };
}

afterEach(() => vi.unstubAllGlobals());

describe('object activity page (AWR-OBS-006 PR-A)', () => {
  it('serves development and requests as separate bounded kinds with full disclosures', async () => {
    const { calls } = setupActivity();
    renderApp('/activity?project=demo');
    expect(await screen.findByTestId('page-activity')).toBeTruthy();
    expect((await screen.findAllByText('access.changed')).length).toBeGreaterThan(0);
    expect(calls.filter((call) => call.params.get('kind') === 'development')).toHaveLength(1);
    expect(calls.filter((call) => call.params.get('kind') === 'requests')).toHaveLength(0);
    expect(screen.getByTestId('activity-scope').textContent).toContain('self');
    expect(screen.getByTestId('activity-development-source').textContent).toContain('committed_events_with_operation_attribution');
    expect(screen.getByTestId('activity-collection-limits').textContent).toContain('chat_text_collected: false');
    expect(screen.getByTestId('activity-unknown-means').textContent).toContain(UNKNOWN_MEANS);
    expect(screen.getByTestId('activity-collection-limits').textContent).toContain('tool_io_collected: false');

    const user = userEvent.setup();
    await user.click(screen.getByRole('tab', { name: '访问记录' }));
    expect((await screen.findAllByText('audit.requests')).length).toBeGreaterThan(0);
    expect(calls.filter((call) => call.params.get('kind') === 'requests')).toHaveLength(1);
    expect(calls.filter((call) => ['audit.history', 'audit.export', 'audit.count'].includes(call.params.get('kind') ?? ''))).toHaveLength(0);
  });

  it('loads older keyset pages without duplicates and keeps limit at or below 100', async () => {
    const all = fixture('derived-audit-development-project-scope-all-run09.json').data.items as Array<Record<string, unknown>>;
    const pages = [
      pagedDevelopment(all.slice(0, 50), 'cursor-1'),
      pagedDevelopment(all.slice(50, 150), 'cursor-2'),
      pagedDevelopment(all.slice(150), null),
    ];
    const { calls } = setupActivity({ projectAudit: true, developmentPages: pages });
    renderApp('/activity?project=demo&limit=100');
    expect((await screen.findAllByTestId('activity-row')).length).toBe(50);
    const user = userEvent.setup();
    await user.click(screen.getByTestId('activity-load-earlier'));
    expect((await screen.findAllByTestId('activity-row')).length).toBe(150);
    await user.click(screen.getByTestId('activity-load-earlier'));
    expect((await screen.findAllByTestId('activity-row')).length).toBe(212);
    const cursors = calls.map((call) => call.params.get('cursor'));
    expect(cursors).toEqual([null, 'cursor-1', 'cursor-2']);
    expect(calls.every((call) => Number(call.params.get('limit')) <= 100)).toBe(true);
  });

  it('falls back to 50 for an invalid limit and clamps larger limits to 100', async () => {
    const { calls } = setupActivity();
    renderApp('/activity?project=demo&limit=abc');
    await screen.findByTestId('page-activity');
    expect(calls[0].params.get('limit')).toBe('50');
    const { calls: largeCalls } = setupActivity();
    renderApp('/activity?project=demo&limit=1000');
    await screen.findByTestId('page-activity');
    expect(largeCalls[0].params.get('limit')).toBe('100');
  });
});
