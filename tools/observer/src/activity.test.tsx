import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { act, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';
import i18n from './i18n';
import { formatUnixMs } from './lib/format';
import { defaultVisibleText, findForbiddenVisibleText } from './lib/human-language-guard';
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
  capabilitiesGate?: Promise<void>;
  capabilitiesError?: { code: string; message: string; recover?: { value: boolean } };
};

function setupActivity({
  projectAudit = false,
  development = fixture('observer-activity-development.json'),
  requests = fixture('observer-activity-requests.json'),
  developmentPages,
  requestsPages,
  capabilitiesGate,
  capabilitiesError,
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
        if (capabilitiesGate) await capabilitiesGate;
        if (capabilitiesError && !capabilitiesError.recover?.value) {
          const { code, message } = capabilitiesError;
          return json({ ok: false, error: { code, message } });
        }
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

afterEach(async () => {
  vi.unstubAllGlobals();
  await i18n.changeLanguage('zh-CN');
});

describe('object activity page (AWR-OBS-006 PR-A)', () => {
  it('serves development and requests as separate bounded kinds and explains the records in plain words', async () => {
    const { calls } = setupActivity();
    renderApp('/activity?project=demo');
    expect(await screen.findByTestId('page-activity')).toBeTruthy();
    expect((await screen.findAllByText('变更授权')).length).toBeGreaterThan(0);
    expect(calls.filter((call) => call.params.get('kind') === 'development')).toHaveLength(1);
    expect(calls.filter((call) => call.params.get('kind') === 'requests')).toHaveLength(0);
    expect(screen.getByTestId('activity-scope').textContent).toBe('当前只显示你自己的记录');
    expect(screen.getByTestId('activity-collection-limits').textContent).toBe('这里不包含聊天文字，也不包含工具的输入输出');
    expect(screen.queryByTestId('activity-retention')).toBeNull();
    expect(findForbiddenVisibleText(defaultVisibleText(screen.getByTestId('page-activity')))).toEqual([]);

    const user = userEvent.setup();
    await user.click(screen.getByRole('tab', { name: '访问记录' }));
    expect((await screen.findAllByText('读取访问记录')).length).toBeGreaterThan(0);
    expect(calls.filter((call) => call.params.get('kind') === 'requests')).toHaveLength(1);
    expect(screen.getByTestId('activity-retention').textContent).toBe('访问记录最多保留 10000 条，更早的记录会被清理');
    expect(findForbiddenVisibleText(defaultVisibleText(screen.getByTestId('page-activity')))).toEqual([]);
    expect(calls.filter((call) => ['audit.history', 'audit.export', 'audit.count'].includes(call.params.get('kind') ?? ''))).toHaveLength(0);
  });

  it.each([
    ['zh-CN', '已提交', '显示范围未提供', '未提供'],
    ['en-US', 'Committed', 'The scope of these records was not provided', 'Not provided'],
  ])('uses shared status and missing-value vocabulary in %s', async (language, status, scopeMissing, missing) => {
    const base = fixture('observer-activity-development.json');
    setupActivity({
      development: {
        data: {
          ...base.data,
          scope: null,
          request_retention: null,
          development_source: null,
          unknown_means: null,
          chat_text_collected: null,
          tool_io_collected: null,
        },
      },
    });
    await i18n.changeLanguage(language);
    renderApp('/activity?project=demo');
    const row = await screen.findByTestId('activity-row');
    expect(row.textContent).toContain(status);
    expect(screen.getByTestId('activity-scope').textContent).toBe(scopeMissing);
    // Nothing is claimed about chat text, tool input or retention when the service did not say.
    expect(screen.queryByTestId('activity-collection-limits')).toBeNull();
    expect(screen.queryByTestId('activity-retention')).toBeNull();
    const user = userEvent.setup();
    await user.click(row);
    expect(screen.getByTestId('activity-event-result').textContent).toBe(status);
    const technical = screen.getAllByTestId('technical-details').at(-1)!;
    await user.click(within(technical).getByRole('button'));
    for (const label of ['scope', 'request_retention', 'development_source', 'chat_text_collected', 'tool_io_collected', 'unknown_means']) {
      expect((await screen.findByTestId(`technical-detail-${label}`)).textContent).toContain(missing);
    }
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
    // The second page repeats the first fifty rows: each audit id is counted once and the repeat is said in plain words.
    expect(screen.getByTestId('activity-note-duplicate-id').textContent).toBe('发现重复记录，原始事件已全部保留');
    expect(screen.getByTestId('activity-count').textContent).toBe('已显示 212 条记录');
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
    expect(screen.queryByTestId('activity-self-filter-note')).toBeNull();

    const { calls: otherCalls } = setupActivity();
    renderApp('/activity?project=demo&actor=OTHER');
    await screen.findByTestId('page-activity');
    expect(otherCalls.every((call) => call.params.get('member_actor_id') === null)).toBe(true);
  });

  it('waits for identity before filtering a project-audit actor deep link', async () => {
    const actor = 'manager-agent-native-20261101T045929Z-ee9ddb44e6e7-4856fa';
    let releaseCapabilities!: () => void;
    const capabilitiesGate = new Promise<void>((resolve) => { releaseCapabilities = resolve; });
    const { calls } = setupActivity({
      projectAudit: true,
      development: fixture('derived-audit-development-project-scope-run09.json'),
      capabilitiesGate,
    });
    renderApp(`/activity?project=demo&actor=${actor}`);
    await act(async () => { await Promise.resolve(); });
    expect(screen.queryByTestId('activity-self-filter-note')).toBeNull();
    expect(calls).toHaveLength(0);

    await act(async () => {
      releaseCapabilities();
      await capabilitiesGate;
    });
    expect(await screen.findByTestId('page-activity')).toBeTruthy();
    expect(calls).toHaveLength(1);
    expect(calls[0].params.get('member_actor_id')).toBe(actor);
    expect(screen.queryByTestId('activity-self-filter-note')).toBeNull();
  });

  it('shows a useful identity failure and retries without an unfiltered read', async () => {
    const actor = 'manager-agent-native-20261101T045929Z-ee9ddb44e6e7-4856fa';
    const recovery = { value: false };
    const { calls } = setupActivity({
      projectAudit: true,
      development: fixture('derived-audit-development-project-scope-run09.json'),
      capabilitiesError: { code: 'ReadTimeout', message: 'slow', recover: recovery },
    });
    renderApp(`/activity?project=demo&actor=${actor}`);
    const failure = await screen.findByTestId('activity-identity-error');
    expect(failure.textContent).toContain('身份信息暂时读取不到，请重试');
    expect(failure.textContent).not.toContain('CapabilitiesReadError');
    expect(failure.textContent).not.toContain('ReadTimeout');
    expect(calls).toHaveLength(0);

    const user = userEvent.setup();
    recovery.value = true;
    await user.click(screen.getByTestId('activity-identity-retry'));
    expect(await screen.findByTestId('page-activity')).toBeTruthy();
    expect(calls).toHaveLength(1);
    expect(calls[0].params.get('member_actor_id')).toBe(actor);
  });

  it('uses member_actor_id for an authorized project-scope actor filter', async () => {
    const actor = 'manager-agent-native-20261101T045929Z-ee9ddb44e6e7-4856fa';
    const { calls } = setupActivity({ projectAudit: true, development: fixture('derived-audit-development-project-scope-run09.json') });
    renderApp(`/activity?project=demo&actor=${actor}`);
    await screen.findByTestId('page-activity');
    expect(calls.some((call) => call.params.get('member_actor_id') === actor)).toBe(true);
  });

  it('selects one audit row by event id, shows it in plain words and never treats it as a business request id', async () => {
    const { calls } = setupActivity();
    const event = '01M6A0PZ0CB6PZ6X8JQ4TSFQWN';
    const { router } = renderApp('/activity?project=demo&kind=requests');
    const user = userEvent.setup();
    await user.click((await screen.findAllByTestId('activity-row'))[0]);
    expect(router.state.location.search).toContain(`event=${event}`);
    const detail = await screen.findByTestId('activity-event-detail');
    expect(detail.textContent).toContain('你');
    expect(detail.textContent).toContain('读取访问记录');
    expect(detail.textContent).toContain('未知');
    for (const hidden of [event, SELF_ACTOR, 'audit.requests']) expect(defaultVisibleText(detail)).not.toContain(hidden);
    expect(findForbiddenVisibleText(defaultVisibleText(detail))).toEqual([]);

    await user.click(within(detail).getByRole('button', { name: /技术细节/ }));
    expect((await screen.findByTestId('activity-event-id')).textContent).toContain(event);
    expect(screen.getByTestId('activity-event-actor-id').textContent).toContain(SELF_ACTOR);
    expect(screen.getByTestId('technical-detail-action').textContent).toContain('audit.requests');
    expect(screen.getByTestId('activity-event-not-request').textContent).toContain('不是业务请求号');
    expect(router.state.location.search).toContain(`event=${event}`);
    expect(calls.some((call) => call.url.includes('command.inspect'))).toBe(false);
  });

  it('keeps unknown results unknown in plain words, keeps unknown_means verbatim in the technical details, and offers no retry or reconciliation control', async () => {
    const { calls } = setupActivity();
    const event = '01M6A0PZ0CB6PZ6X8JQ4TSFQWN';
    renderApp(`/activity?project=demo&kind=requests&event=${event}`);
    const alert = await screen.findByTestId('activity-unknown-result');
    expect(alert.textContent).toBe('还没有完成记录；请先核对原业务请求，再决定是否重试');
    expect(alert.textContent).not.toContain('completion_not_recorded');
    expect(screen.queryByRole('button', { name: /重试|对账|Retry|Reconcile/i })).toBeNull();
    const user = userEvent.setup();
    await user.click(within(screen.getAllByTestId('technical-details').at(-1)!).getByRole('button'));
    expect((await screen.findByTestId('technical-detail-unknown_means')).textContent).toContain(UNKNOWN_MEANS);
    expect(calls.some((call) => call.url.includes('command.inspect'))).toBe(false);
  });

  it('filters access changes only within the loaded development records and says so', async () => {
    const all = fixture('derived-audit-development-project-scope-all-run09.json').data.items as Array<Record<string, unknown>>;
    const { calls } = setupActivity({ projectAudit: true, development: pagedDevelopment(all, null) });
    renderApp('/activity?project=demo');
    const user = userEvent.setup();
    const accessTab = await screen.findByText('授权变更');
    await user.click(accessTab);
    expect((await screen.findByTestId('activity-access-note')).textContent).toBe('这里只列出已加载的开发记录里的授权变更，不是完整的授权历史');
    expect((await screen.findAllByText('变更授权')).length).toBeGreaterThan(0);
    expect(screen.queryByText('结束工作')).toBeNull();
    expect(calls.filter((call) => call.params.get('kind') === 'requests')).toHaveLength(0);
    expect(calls.some((call) => ['audit.history', 'audit.export', 'audit.count'].includes(call.params.get('kind') ?? ''))).toBe(false);
  });

  it('keeps polling a single loaded activity page', async () => {
    const { calls } = setupActivity();
    renderApp('/activity?project=demo');
    await screen.findByTestId('activity-row');
    const initialCalls = calls.length;
    await new Promise((resolve) => setTimeout(resolve, 6500));
    expect(calls.length).toBeGreaterThan(initialCalls);
  }, 10000);

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

  it('shows operators as numbered stand-ins and never as raw identifiers', async () => {
    const all = fixture('derived-audit-development-project-scope-all-run09.json').data.items as Array<Record<string, unknown>>;
    setupActivity({ projectAudit: true, development: pagedDevelopment(all.slice(0, 60), null) });
    renderApp('/activity?project=demo');
    const rows = await screen.findAllByTestId('activity-row');
    expect(rows.length).toBe(60);
    const text = rows.map((row) => row.textContent ?? '').join(' ');
    expect(text).toMatch(/操作者 1/);
    expect(text).not.toContain('manager-agent-native');
    expect(text).not.toContain('-ee9ddb44e6e7-');
    expect(findForbiddenVisibleText(defaultVisibleText(screen.getByTestId('page-activity')))).toEqual([]);
    expect(screen.getByTestId('activity-operators-note').textContent).toContain('真实标识在每条记录的技术细节里');
  });

  it('names a work by its readable key, an opaque id by a neutral phrase, and keeps the real id for the link and the technical details', async () => {
    const base = fixture('observer-activity-development.json');
    const opaque = '01M6A0PZ0CB6PZ6X8JQ4TSFQWN';
    const development = { data: { ...base.data, items: [
      { ...base.data.items[0], id: 'named-row', work_id: 'issue-filter-api' },
      { ...base.data.items[0], id: 'opaque-row', work_id: opaque, created_at_unix_ms: base.data.items[0].created_at_unix_ms - 1 },
    ] } };
    setupActivity({ development });
    renderApp('/activity?project=demo&event=opaque-row');
    expect((await screen.findByTestId('activity-work-link-issue-filter-api')).textContent).toBe('issue-filter-api');
    const opaqueLink = screen.getByTestId(`activity-work-link-${opaque}`);
    expect(opaqueLink.textContent).toBe('某项工作');
    expect(opaqueLink.getAttribute('href')).toBe(`/projects/demo?tab=acceptance&work=${opaque}`);
    const detail = screen.getByTestId('activity-event-detail');
    expect(defaultVisibleText(detail)).not.toContain(opaque);
    const user = userEvent.setup();
    await user.click(within(detail).getByRole('button', { name: /技术细节/ }));
    expect((await screen.findByTestId('technical-detail-work_id')).textContent).toContain(opaque);
  });

  it('groups an unlisted operation as other operation, says so once, and keeps the raw name in the technical details', async () => {
    const base = fixture('observer-activity-development.json');
    const items = [
      { ...base.data.items[0], id: 'odd-1', action: 'invalid.query' },
      { ...base.data.items[0], id: 'odd-2', action: 'another.unlisted', created_at_unix_ms: base.data.items[0].created_at_unix_ms - 1 },
    ];
    setupActivity({ development: { data: { ...base.data, items } } });
    renderApp('/activity?project=demo&event=odd-1');
    const rows = await screen.findAllByTestId('activity-row');
    expect(rows.map((row) => row.textContent)).toEqual([expect.stringContaining('其他操作'), expect.stringContaining('其他操作')]);
    expect(screen.getAllByTestId('activity-note-unmapped-action')).toHaveLength(1);
    expect(screen.getByTestId('activity-note-unmapped-action').textContent).toBe('未收录的操作已归为其他操作');
    for (const row of rows) expect(row.textContent).not.toMatch(/invalid\.query|another\.unlisted/);
    const user = userEvent.setup();
    await user.click(within(screen.getByTestId('activity-event-detail')).getByRole('button', { name: /技术细节/ }));
    expect((await screen.findByTestId('technical-detail-action')).textContent).toContain('invalid.query');
  });

  it('shows a readable "all" entry in each filter when nothing is filtered, and the chosen work when one is', async () => {
    setupActivity({ projectAudit: true, development: fixture('derived-audit-development-project-scope-run09.json') });
    const { unmount } = renderApp('/activity?project=demo');
    await screen.findAllByTestId('activity-row');
    expect(within(screen.getByTestId('activity-work-filter')).getByText('全部工作')).toBeTruthy();
    expect(within(screen.getByTestId('activity-actor-filter')).getByText('全部操作者')).toBeTruthy();
    unmount();
    setupActivity({ projectAudit: true, development: fixture('derived-audit-development-project-scope-run09.json') });
    renderApp('/activity?project=demo&work=issue-filter-page');
    await screen.findAllByTestId('activity-row');
    expect(within(screen.getByTestId('activity-work-filter')).getByText('issue-filter-page')).toBeTruthy();
    expect(within(screen.getByTestId('activity-work-filter')).queryByText('全部工作')).toBeNull();
  });

  it('clears activity details and silences reads after revocation', async () => {
    const { calls } = setupActivity();
    const { queryClient } = renderApp('/activity?project=demo');
    await screen.findByTestId('activity-row');
    act(() => revokeObservationReads(queryClient));
    expect(await screen.findByTestId('state-forbidden')).toBeTruthy();
    expect(screen.queryByTestId('activity-row')).toBeNull();
    const settled = calls.length;
    await new Promise((resolve) => setTimeout(resolve, 6500));
    expect(calls.length).toBe(settled);
  }, 10000);
});
