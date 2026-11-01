import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { act, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { formatUnixMs } from './lib/format';
import { revokeObservationReads } from './lib/queries';
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
  const capabilityCalls: string[] = [];
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
        capabilityCalls.push(url);
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
  return { calls, capabilityCalls };
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
    expect(screen.getByTestId('activity-retention').textContent).toContain('oldest_pruned_soft_capacity');
    expect(screen.getByTestId('activity-retention').textContent).toContain('10000');
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
      pagedDevelopment(all, 'cursor-2'),
      pagedDevelopment(all.slice(150), null),
    ];
    const { calls } = setupActivity({ projectAudit: true, developmentPages: pages });
    renderApp('/activity?project=demo&limit=100');
    expect((await screen.findAllByTestId('activity-row')).length).toBe(50);
    const user = userEvent.setup();
    await user.click(screen.getByTestId('activity-load-earlier'));
    expect((await screen.findAllByTestId('activity-row')).length).toBe(212);
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

  it('links non-null work ids to the exact outcome and never creates a null-work link', async () => {
    setupActivity({ projectAudit: true, development: fixture('derived-audit-development-project-scope-run09.json') });
    renderApp('/activity?project=demo');
    const link = (await screen.findAllByTestId('activity-work-link-issue-filter-page'))[0];
    expect(link.getAttribute('href')).toBe('/projects/demo?tab=acceptance&work=issue-filter-page');
  });

  it('uses work_id for the server-side work filter', async () => {
    const { calls } = setupActivity({ projectAudit: true, development: fixture('derived-audit-development-project-scope-run09.json') });
    renderApp('/activity?project=demo&work=issue-filter-api');
    await screen.findByTestId('page-activity');
    expect(calls[0].params.get('work_id')).toBe('issue-filter-api');
    expect(screen.queryByTestId('activity-work-link-issue-filter-page')).toBeNull();
  });

  it('does not link a null work_id', async () => {
    setupActivity();
    renderApp('/activity?project=demo');
    expect(await screen.findByTestId('activity-work-empty-01M69TZWWAF2FWZ0D1NHX1NJ0A')).toBeTruthy();
    expect(screen.queryByRole('link', { name: 'issue-filter-api' })).toBeNull();
  });

  it('allows self actor filtering but ignores another actor in self scope', async () => {
    const { calls: ownCalls } = setupActivity();
    renderApp(`/activity?project=demo&actor=${SELF_ACTOR}`);
    await screen.findByTestId('page-activity');
    expect(ownCalls.some((call) => call.params.get('member_actor_id') === SELF_ACTOR)).toBe(true);

    const { calls: otherCalls } = setupActivity();
    renderApp('/activity?project=demo&actor=OTHER');
    await screen.findByTestId('page-activity');
    expect(otherCalls.every((call) => call.params.get('member_actor_id') === null)).toBe(true);
  });

  it('uses member_actor_id for an authorized project-scope actor filter', async () => {
    const actor = 'manager-agent-native-20261101T045929Z-ee9ddb44e6e7-4856fa';
    const { calls } = setupActivity({ projectAudit: true, development: fixture('derived-audit-development-project-scope-run09.json') });
    renderApp(`/activity?project=demo&actor=${actor}`);
    await screen.findByTestId('page-activity');
    expect(calls.some((call) => call.params.get('member_actor_id') === actor)).toBe(true);
  });

  it('selects one audit row by event id and never treats it as a business request id', async () => {
    const { calls } = setupActivity();
    const event = '01M6A0PZ0CB6PZ6X8JQ4TSFQWN';
    const { router } = renderApp('/activity?project=demo&kind=requests');
    const user = userEvent.setup();
    await user.click((await screen.findAllByTestId('activity-row'))[0]);
    expect(router.state.location.search).toContain(`event=${event}`);
    const detail = await screen.findByTestId('activity-event-detail');
    expect(detail.textContent).toContain(event);
    expect(detail.textContent).toContain(SELF_ACTOR);
    expect(detail.textContent).toContain('audit.requests');
    expect(detail.textContent).toContain('unknown');
    expect(screen.getByTestId('activity-event-not-request').textContent).toContain('不是业务 request_id');
    expect(router.state.location.search).toContain(`event=${event}`);
    expect(calls.some((call) => call.url.includes('command.inspect'))).toBe(false);
  });

  it('keeps unknown results unknown with verbatim unknown_means and no retry or reconciliation control', async () => {
    const { calls } = setupActivity();
    const event = '01M6A0PZ0CB6PZ6X8JQ4TSFQWN';
    renderApp(`/activity?project=demo&kind=requests&event=${event}`);
    expect((await screen.findByTestId('activity-unknown-result')).textContent).toContain(UNKNOWN_MEANS);
    expect(screen.queryByRole('button', { name: /重试|对账|Retry|Reconcile/i })).toBeNull();
    expect(calls.some((call) => call.url.includes('command.inspect'))).toBe(false);
  });

  it('filters access.changed only within loaded development and labels the limitation', async () => {
    const all = fixture('derived-audit-development-project-scope-all-run09.json').data.items as Array<Record<string, unknown>>;
    const { calls } = setupActivity({ projectAudit: true, development: pagedDevelopment(all, null) });
    renderApp('/activity?project=demo');
    const user = userEvent.setup();
    const accessTab = await screen.findByText('授权变更');
    await user.click(accessTab);
    expect((await screen.findByTestId('activity-access-note')).textContent).toContain('已加载 development');
    expect(screen.getByTestId('activity-access-note').textContent).toContain('action=access.changed');
    expect((await screen.findAllByText('access.changed')).length).toBeGreaterThan(0);
    expect(screen.queryByText('session.end')).toBeNull();
    expect(calls.filter((call) => call.params.get('kind') === 'requests')).toHaveLength(0);
    expect(calls.some((call) => ['audit.history', 'audit.export', 'audit.count'].includes(call.params.get('kind') ?? ''))).toBe(false);
  });

  it('stops polling activity pages and capabilities after the second page is loaded', async () => {
    const all = fixture('derived-audit-development-project-scope-all-run09.json').data.items as Array<Record<string, unknown>>;
    const pages = [
      pagedDevelopment(all.slice(0, 50), 'cursor-1'),
      pagedDevelopment(all, 'cursor-2'),
      pagedDevelopment(all.slice(150), null),
    ];
    const { calls, capabilityCalls } = setupActivity({ projectAudit: true, developmentPages: pages });
    renderApp('/activity?project=demo&limit=100');
    expect((await screen.findAllByTestId('activity-row')).length).toBe(50);
    const user = userEvent.setup();
    await user.click(screen.getByTestId('activity-load-earlier'));
    expect((await screen.findAllByTestId('activity-row')).length).toBe(212);
    const activityCount = calls.length;
    const capabilityCount = capabilityCalls.length;
    await new Promise((resolve) => setTimeout(resolve, 6000));
    expect(calls.length).toBe(activityCount);
    expect(capabilityCalls.length).toBe(capabilityCount);
  }, 10000);

  it('labels a null work_id as no work, not not measured', async () => {
    setupActivity();
    renderApp('/activity?project=demo');
    const empty = await screen.findByTestId('activity-work-empty-01M69TZWWAF2FWZ0D1NHX1NJ0A');
    expect(empty.textContent).toBe('无关联工作');
    expect(empty.textContent).not.toBe('未测量');
  });

  it('encodes work ids that need URL encoding in outcome links', async () => {
    const base = fixture('observer-activity-development.json');
    const development = { data: { ...base.data, items: [{ ...base.data.items[0], work_id: 'work/id ?' }] } };
    setupActivity({ development });
    renderApp('/activity?project=demo');
    const link = await screen.findByTestId('activity-work-link-work/id ?');
    expect(link.getAttribute('href')).toBe('/projects/demo?tab=acceptance&work=work%2Fid%20%3F');
  });

  it('shows the unknown alert only for result unknown', async () => {
    setupActivity();
    renderApp('/activity?project=demo&kind=requests&event=01M6A0PZ04TGSJR97FA2E0QGHD');
    expect(await screen.findByTestId('activity-event-detail')).toBeTruthy();
    expect(screen.queryByTestId('activity-unknown-result')).toBeNull();
  });

  it('does not show load earlier without a next cursor', async () => {
    setupActivity();
    renderApp('/activity?project=demo');
    await screen.findByTestId('activity-row');
    expect(screen.queryByTestId('activity-load-earlier')).toBeNull();
  });

  it('formats table times with the shared formatter rather than raw milliseconds', async () => {
    const timestamp = 1793479144330;
    setupActivity();
    renderApp('/activity?project=demo');
    const row = await screen.findByTestId('activity-row');
    expect(row.textContent).toContain(formatUnixMs(timestamp, 'zh-CN'));
    expect(row.textContent).not.toContain(String(timestamp));
  });

  it('treats an empty next_cursor as no more pages', async () => {
    const base = fixture('observer-activity-development.json');
    setupActivity({ development: { data: { ...base.data, next_cursor: '' } } });
    renderApp('/activity?project=demo');
    await screen.findByTestId('activity-row');
    expect(screen.queryByTestId('activity-load-earlier')).toBeNull();
  });

  it('explains when a self-scope actor filter is ignored', async () => {
    setupActivity();
    renderApp('/activity?project=demo&actor=OTHER');
    expect((await screen.findByTestId('activity-self-filter-note')).textContent).toContain('仅能查看本人活动，已忽略筛选');
  });

  it('clears activity details and silences reads after revocation', async () => {
    const { calls } = setupActivity();
    const { queryClient } = renderApp('/activity?project=demo');
    await screen.findByTestId('activity-row');
    act(() => revokeObservationReads(queryClient));
    expect(await screen.findByTestId('state-forbidden')).toBeTruthy();
    expect(screen.queryByTestId('activity-row')).toBeNull();
    const settled = calls.length;
    await new Promise((resolve) => setTimeout(resolve, 1200));
    expect(calls.length).toBe(settled);
  });
});
