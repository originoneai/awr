import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { act, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';
import i18n from './i18n';
import { compactTimeText } from './core/observation';
import { defaultVisibleText, findForbiddenVisibleText } from './lib/human-language-guard';
import { revokeObservationReads } from './lib/queries';
import type { TeamActivityActorKind, TeamActivityItem } from './lib/types';
import { DEMO_PROJECTS, renderApp } from './test-utils';

const NOW = 1_800_000_000_000;
const KNOWN_ACTOR = 'synthetic-actor-a';
const KNOWN_PERSON = 'synthetic-person-a';
const MISSING_ACTOR = 'synthetic-actor-not-in-directory';
const WORK_A = 'synthetic-work-a';
const WORK_B = 'synthetic-work-b';

function liveFixture(name: string) {
  return JSON.parse(readFileSync(resolve(`src/test-fixtures/activity/${name}`), 'utf8'));
}

type Identity = {
  name?: string | null;
  /** The client's self-registered name (AWR-PPD-062). */
  client?: string | null;
  /** The recorded actor kind; omitted leaves the kind out of the reply, as an older server does. */
  kind?: TeamActivityActorKind | null;
  /** Whether the row carries a client id at all. Real requests all do; the demo used to leave it out. */
  withClientId?: boolean;
};

function event(
  id: string,
  action: string,
  workId: string | null = WORK_A,
  actorId = KNOWN_ACTOR,
  offset = 0,
  identity: Identity = {},
): TeamActivityItem {
  // AWR-PPD-055 reply shape: the audit carries its own actor/client disclosure, and
  // AWR-PPD-062 adds the actor's kind. Every real request records a client id.
  return {
    id,
    action,
    actor_id: actorId,
    client_id: identity.withClientId ?? true ? `synthetic-client-${offset}` : null,
    actor: {
      person_id: KNOWN_PERSON,
      display_name: identity.name === undefined ? 'Alex Example' : identity.name,
      ...(identity.kind === undefined ? {} : { kind: identity.kind }),
    },
    client: { display_name: identity.client ?? null },
    created_at_unix_ms: NOW - offset * 60_000,
    credential_id: null,
    result: 'committed',
    work_id: workId,
  };
}

/** A pre-#725 reply: no actor/client objects at all, exactly what the live samples hold. */
function legacyEvent(id: string, action: string, workId: string | null, actorId: string, offset = 0): TeamActivityItem {
  return {
    id,
    action,
    actor_id: actorId,
    client_id: null,
    created_at_unix_ms: NOW - offset * 60_000,
    credential_id: null,
    result: 'committed',
    work_id: workId,
  };
}

const CATEGORY_EVENTS = [
  event('delivery', 'delivery.submit_and_request_review', WORK_A, KNOWN_ACTOR, 0),
  event('review', 'review.decide', WORK_B, MISSING_ACTOR, 1),
  event('decision', 'decision.request', WORK_A, KNOWN_ACTOR, 2),
  event('assignment', 'task.assign', WORK_B, KNOWN_ACTOR, 3),
  event('report', 'execution.report', WORK_A, KNOWN_ACTOR, 4),
  event('breakdown', 'planning.propose', null, KNOWN_ACTOR, 5),
  event('update', 'update.record', null, KNOWN_ACTOR, 6),
  event('other', 'access.changed', null, MISSING_ACTOR, 7),
];

function activityPayload(items: TeamActivityItem[] = CATEGORY_EVENTS, nextCursor: unknown = null) {
  return {
    ok: true,
    data: {
      items,
      next_cursor: nextCursor,
      development_source: 'committed_events_with_operation_attribution; historical client may be unavailable',
      chat_text_collected: false,
      tool_io_collected: false,
      request_retention: 'oldest_pruned_soft_capacity',
      request_retention_capacity: 10000,
      scope: 'project',
      unknown_means: 'completion_not_recorded; inspect business request before retrying',
    },
  };
}

function overviewModel(overrides: {
  taskCoverage?: string;
  memberCoverage?: string;
  tasks?: Array<Record<string, unknown>>;
  members?: Array<Record<string, unknown>>;
  /** The reply's own clock pair; defaults to the fixture NOW for both fields. */
  clock?: { nowUnixMs?: number; readAtUnixMs?: number };
} = {}) {
  const taskCoverage = overrides.taskCoverage ?? 'complete';
  const memberCoverage = overrides.memberCoverage ?? 'complete';
  const slice = (coverage: string, items: Array<Record<string, unknown>>) => ({ coverage, items });
  const part = (status: string) => ({ status, missing: status === 'complete' ? [] : [{ op: 'project context', reason: status }] });
  return {
    ok: true,
    project: 'demo',
    read_at_unix_ms: overrides.clock?.readAtUnixMs ?? NOW,
    input: {
      tasks: slice(taskCoverage, overrides.tasks ?? [
        { id: WORK_A, title: 'Synthetic task alpha' },
        { id: WORK_B, title: 'Synthetic task beta' },
      ]),
      members: slice(memberCoverage, overrides.members ?? [
        { personId: KNOWN_PERSON, displayName: 'Alex Example', groupId: 'synthetic-group' },
      ]),
      dependencies: slice('unread', []), decisions: slice('unread', []), reviewRounds: slice('unread', []),
      handoffs: slice('unread', []), approvals: slice('unread', []), activity: slice('unread', []),
      options: {
        nowUnixMs: overrides.clock?.nowUnixMs ?? NOW,
        stalledAfterDays: 7,
        views: { me: { personId: KNOWN_PERSON, projectId: 'demo' } },
      },
    },
    coverage: {
      tasks: part(taskCoverage), members: part(memberCoverage), dependencies: part('unread'),
      decisions: part('unread'), reviewRounds: part('unread'), handoffs: part('unread'),
      approvals: part('unread'), activity: part('unread'),
    },
    freshness: {
      report_received: { status: 'not_measured' },
      authoritative: { status: 'not_measured' },
      page_read: { status: 'measured', at_unix_ms: NOW },
    },
  };
}

type SetupOptions = {
  development?: unknown;
  requests?: unknown;
  developmentPages?: unknown[];
  activityGate?: Promise<void>;
  activityFailure?: { current: boolean };
  overview?: unknown;
  overviewFailure?: { current: boolean };
  secondPageFailure?: boolean;
  /** The shell read's reply; defaults to an unpinned deployment (viewer zone). */
  shell?: unknown;
};

function setupActivity(options: SetupOptions = {}) {
  const calls: Array<{ url: string; params: URLSearchParams }> = [];
  const developmentPages = options.developmentPages ?? [options.development ?? activityPayload()];
  const requestPages = [options.requests ?? activityPayload([event('request', 'audit.requests', null, KNOWN_ACTOR, 0)])];
  const fetchMock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
    const url = String(input);
    if (url.includes('/api/team/projects')) return json(DEMO_PROJECTS);
    if (url.includes('/api/team/shell')) {
      return json(options.shell ?? { ok: true, source: 'demo', organizationName: null, perspective: null });
    }
    if (url.includes('/api/team/overview-model')) {
      if (options.overviewFailure?.current) return json({ ok: false, error: { code: 'Unavailable', message: 'synthetic failure' } });
      return json(options.overview ?? overviewModel());
    }
    if (url.includes('/api/team/activity')) {
      if (options.activityGate) await options.activityGate;
      if (options.activityFailure?.current) return json({ ok: false, error: { code: 'Unavailable', message: 'synthetic failure' } });
      const parsed = new URL(url, 'http://localhost');
      const params = parsed.searchParams;
      calls.push({ url, params });
      const kind = params.get('kind');
      const pages = kind === 'requests' ? requestPages : developmentPages;
      const cursor = params.get('cursor');
      if (cursor && options.secondPageFailure) return json({ ok: false, error: { code: 'Unavailable', message: 'synthetic page failure' } });
      const index = cursor ? pages.findIndex((page) => (page as { data: { next_cursor?: unknown } }).data.next_cursor === cursor) + 1 : 0;
      const page = pages[index] ?? activityPayload([]);
      const typedPage = page as { data: { items: TeamActivityItem[]; [key: string]: unknown } };
      const items = typedPage.data.items.filter((item) => (
        (!params.get('work_id') || item.work_id === params.get('work_id'))
        && (!params.get('member_actor_id') || item.actor_id === params.get('member_actor_id'))
      ));
      return json({ ...typedPage, data: { ...typedPage.data, items } });
    }
    if (url.includes('/api/team/read')) {
      const query = JSON.parse(String(init?.body)).query;
      if (query.op === 'capabilities') {
        return json({
          ok: true,
          result: {
            identity: {
              actor_id: KNOWN_ACTOR,
              client_id: 'synthetic-current-client',
              role: 'reader',
              business_roles: ['observer'],
              can_read_project_audit: true,
            },
          },
        });
      }
    }
    return json({ ok: false, error: { code: 'NoRoute', message: 'Unexpected synthetic route' } });
  });
  vi.stubGlobal('fetch', fetchMock);
  return { calls, fetchMock };
}

function json(payload: unknown) {
  return new Response(JSON.stringify(payload), { status: 200, headers: { 'content-type': 'application/json' } });
}

function tableRows() {
  return [...document.querySelectorAll('.activity-table .ant-table-tbody > tr.ant-table-row')] as HTMLTableRowElement[];
}

function sectionCount() {
  return document.querySelector('.activity-section .obs-section-count')?.textContent;
}

async function changeLanguage(language: string) {
  await i18n.changeLanguage(language);
}

afterEach(async () => {
  vi.unstubAllGlobals();
  vi.useRealTimers();
  await i18n.changeLanguage('zh-CN');
});

describe('activity prototype structure and product language', () => {
  it('renders the exact header, two segmented controls, four columns and all seven categories', async () => {
    setupActivity();
    renderApp('/activity?project=demo');
    expect(await screen.findByRole('heading', { name: '活动与审计', level: 1 })).toBeTruthy();
    expect(screen.getByText('谁在什么时候做了什么')).toBeTruthy();
    expect(screen.getByRole('heading', { name: '发生过什么', level: 2 })).toBeTruthy();
    await waitFor(() => expect(tableRows()).toHaveLength(8));
    expect(within(screen.getByRole('group', { name: '项目' })).getAllByRole('button').map((button) => button.textContent))
      .toEqual(['Team mainline', 'Docs stream']);
    expect(within(screen.getByRole('group', { name: '类别' })).getAllByRole('button').map((button) => button.textContent))
      .toEqual(['全部', '交付', '复核', '决定', '派工', '报告', '拆解', '更新']);
    const headers = [...document.querySelectorAll('.activity-table thead th')].map((header) => header.textContent).filter(Boolean);
    expect(headers).toEqual(['什么时候', '谁', '做了什么', '哪件任务']);
    expect(tableRows()).toHaveLength(8);
    expect(sectionCount()).toBe('8');
    expect(screen.getByTestId('activity-collection-limits').textContent)
      .toBe('这里记的是操作，每一条都是某个人通过自己的 Agent 做的。不采集聊天内容，也不采集工具的输入输出。');

    await userEvent.setup().click(within(screen.getByRole('group', { name: '类别' })).getByRole('button', { name: '交付' }));
    expect(tableRows()).toHaveLength(1);
    expect(sectionCount()).toBe('1');
    expect(tableRows()[0].textContent).toContain('提交成果并请求复核');
    expect(tableRows()[0].textContent).not.toContain('复核决定');
  });

  it('changes projects through the URL and starts a fresh activity read', async () => {
    const { calls } = setupActivity();
    renderApp('/activity?project=demo&category=delivery');
    await waitFor(() => expect(tableRows()).toHaveLength(1));
    await userEvent.setup().click(within(screen.getByRole('group', { name: '项目' })).getByRole('button', { name: 'Docs stream' }));
    await waitFor(() => expect(calls.at(-1)?.params.get('project')).toBe('docs'));
    expect(calls.at(-1)?.params.get('cursor')).toBeNull();
    expect(within(screen.getByRole('group', { name: '类别' })).getByRole('button', { name: '全部' }).getAttribute('aria-pressed')).toBe('true');
  });

  it('writes the audit\'s own disclosure: client (name) for agent runs, name (via client) for people, and no raw ids', async () => {
    const items = [
      event('agent-run', 'execution.report', WORK_A, KNOWN_ACTOR, 0, { name: 'Alex Example', client: 'Synthetic Client', kind: 'agent' }),
      event('via-client', 'task.assign', WORK_B, KNOWN_ACTOR, 1, { name: 'Alex Example', client: 'Synthetic Client', kind: 'human' }),
      event('no-client', 'review.decide', WORK_A, KNOWN_ACTOR, 2, { name: 'Alex Example', client: null, kind: 'agent' }),
      event('undisclosed', 'decision.request', WORK_B, MISSING_ACTOR, 3, { name: null, client: null, kind: null }),
      legacyEvent('legacy', 'update.record', null, MISSING_ACTOR, 4),
      event('machine-name', 'planning.propose', null, KNOWN_ACTOR, 5, { name: 'observer-host-native-20261031T203834Z', client: null, kind: 'agent' }),
    ];
    setupActivity({ development: activityPayload(items) });
    renderApp('/activity?project=demo');
    await waitFor(() => expect(tableRows()).toHaveLength(6));
    const who = () => tableRows().map((row) => row.querySelector('td:nth-child(2)')?.textContent);
    expect(who()).toEqual([
      'Synthetic Client（Alex Example）',
      'Alex Example（通过 Synthetic Client）',
      'Alex Example',
      '未登记姓名',
      '未登记姓名',
      '未登记姓名',
    ]);
    for (const leaked of [KNOWN_ACTOR, MISSING_ACTOR, 'synthetic-client-0', KNOWN_PERSON]) {
      expect(document.body.textContent).not.toContain(leaked);
    }
    expect(findForbiddenVisibleText(defaultVisibleText(screen.getByTestId('page-activity')))).toEqual([]);
  });

  it('keeps the client unnamed when only one side of the attribution is disclosed', async () => {
    const items = [
      event('client-null-attributed', 'execution.report', WORK_A, KNOWN_ACTOR, 0, { name: 'Alex Example', client: null, kind: 'agent' }),
      event('client-name-unattributed', 'task.assign', WORK_B, KNOWN_ACTOR, 1, { name: null, client: 'Synthetic Client', kind: 'human' }),
    ];
    setupActivity({ development: activityPayload(items) });
    renderApp('/activity?project=demo');
    await waitFor(() => expect(tableRows()).toHaveLength(2));
    expect(tableRows()[0].textContent).toContain('Alex Example');
    expect(tableRows()[0].textContent).not.toContain('Synthetic Client');
    expect(tableRows()[1].textContent).toContain('未登记姓名（通过 Synthetic Client）');
  });

  // AWR-PPD-062: the recorded actor kind decides who acted. Every real request carries
  // a client id, so its presence or absence says nothing and no longer takes part.
  it('follows the recorded actor kind, whatever the row\'s client id says', async () => {
    const items = [
      event('agent-without-client-id', 'execution.report', WORK_A, KNOWN_ACTOR, 0, { client: 'Synthetic Client', kind: 'agent', withClientId: false }),
      event('human-with-client-id', 'task.assign', WORK_B, KNOWN_ACTOR, 1, { client: 'Synthetic Client', kind: 'human', withClientId: true }),
      event('kind-withheld', 'review.decide', WORK_A, KNOWN_ACTOR, 2, { client: 'Synthetic Client', kind: null, withClientId: true }),
      event('kind-missing', 'decision.request', WORK_B, KNOWN_ACTOR, 3, { client: 'Synthetic Client', withClientId: false }),
      event('system-actor', 'planning.propose', null, KNOWN_ACTOR, 4, { client: 'Synthetic Client', kind: 'system', withClientId: true }),
      event('agent-undisclosed-person', 'update.record', null, KNOWN_ACTOR, 5, { name: null, client: 'Synthetic Client', kind: 'agent', withClientId: true }),
    ];
    setupActivity({ development: activityPayload(items) });
    renderApp('/activity?project=demo');
    await waitFor(() => expect(tableRows()).toHaveLength(6));
    const who = () => tableRows().map((row) => row.querySelector('td:nth-child(2)')?.textContent);
    expect(who()).toEqual([
      'Synthetic Client（Alex Example）',
      'Alex Example（通过 Synthetic Client）',
      'Alex Example',
      'Alex Example',
      'Alex Example',
      'Synthetic Client（未登记姓名）',
    ]);
    expect(document.body.textContent).not.toContain('synthetic-client-');
    expect(findForbiddenVisibleText(defaultVisibleText(screen.getByTestId('page-activity')))).toEqual([]);
  });

  it('uses task titles, never work keys, and makes only rows with a resolved task navigable', async () => {
    setupActivity();
    const rendered = renderApp('/activity?project=demo');
    await waitFor(() => expect(tableRows()).toHaveLength(8));
    expect(tableRows()[0].textContent).toContain('Synthetic task alpha');
    expect(document.body.textContent).not.toContain(WORK_A);
    expect(tableRows()[0].dataset.openable).toBe('true');
    expect(tableRows()[5].dataset.openable).not.toBe('true');
    await userEvent.setup().click(tableRows()[0]);
    expect(rendered.router.state.location.pathname).toBe(`/tasks/${WORK_A}`);
    expect(rendered.router.state.location.search).toBe('?scope=demo');
  });

  it('uses the unregistered-task fallback but keeps resolvable tasks with empty or key-equal titles navigable', async () => {
    const items = [
      event('empty-title', 'review.decide', WORK_A, KNOWN_ACTOR, 0),
      event('key-title', 'review.decide', WORK_B, KNOWN_ACTOR, 1),
    ];
    setupActivity({
      development: activityPayload(items),
      overview: overviewModel({ tasks: [{ id: WORK_A, title: '' }, { id: WORK_B, title: WORK_B }] }),
    });
    renderApp('/activity?project=demo');
    await waitFor(() => expect(tableRows()).toHaveLength(2));
    for (const row of tableRows()) {
      expect(row.textContent).toContain('未登记任务名');
      expect(row.dataset.openable).toBe('true');
    }
    const links = [...document.querySelectorAll<HTMLAnchorElement>('.activity-task-link')];
    expect(links.map((link) => link.textContent)).toEqual(['未登记任务名', '未登记任务名']);
    expect(links[0].getAttribute('href')).toBe(`/tasks/${WORK_A}?scope=demo`);
    expect(document.body.textContent).not.toContain(WORK_A);
    expect(document.body.textContent).not.toContain(WORK_B);
  });

  it('ends a completed project read with only the prototype\'s footnote sentence', async () => {
    setupActivity();
    renderApp('/activity?project=demo');
    await waitFor(() => expect(tableRows()).toHaveLength(8));
    const note = screen.getByTestId('activity-collection-limits').parentElement;
    expect(note).toBe(screen.getByTestId('page-activity').querySelector('.activity-pnote'));
    expect(note?.childNodes).toHaveLength(1);
    expect(note?.textContent)
      .toBe('这里记的是操作，每一条都是某个人通过自己的 Agent 做的。不采集聊天内容，也不采集工具的输入输出。');
    for (const quiet of ['activity-scope', 'activity-coverage', 'activity-retention', 'activity-self-filter-note', 'activity-load-earlier', 'freshness-badge']) {
      expect(screen.queryByTestId(quiet)).toBeNull();
    }
  });

  it('still discloses a self scope, an unread or partial context and a request retention cap', async () => {
    const selfScope = activityPayload([], null);
    (selfScope.data as { scope?: string }).scope = 'self';
    setupActivity({
      development: selfScope,
      requests: activityPayload([event('request', 'audit.requests', null, KNOWN_ACTOR, 0)]),
      overview: overviewModel({ taskCoverage: 'unread', memberCoverage: 'partial' }),
    });
    renderApp('/activity?project=demo');
    expect(await screen.findByTestId('activity-scope')).toBeTruthy();
    expect(screen.getByTestId('activity-scope').textContent).toBe('当前只显示你自己的记录');
    expect(screen.getByTestId('activity-coverage').textContent).toMatch(/没有读全|尚未读取/);
    expect(screen.getByTestId('activity-scope').parentElement).toBe(screen.getByTestId('activity-coverage').parentElement);

    const retentionView = renderApp('/activity?project=demo&kind=requests');
    expect(await screen.findByTestId('activity-retention')).toBeTruthy();
    expect(screen.getByTestId('activity-retention').textContent).toBe('访问记录最多保留 10000 条，更早的记录会被清理');
    expect(screen.getByTestId('activity-retention').parentElement).not.toBe(screen.getByTestId('activity-scope').parentElement);
    retentionView.unmount();
  });

  it('shows the freshness badge only when a context plane is stale or failed', async () => {
    const stale = overviewModel();
    (stale.freshness as { authoritative?: { status: string } }).authoritative = { status: 'stale' };
    setupActivity({ overview: stale });
    const view = renderApp('/activity?project=demo');
    await waitFor(() => expect(tableRows()).toHaveLength(8));
    expect(screen.getByTestId('freshness-badge')).toBeTruthy();
    view.unmount();
  });

  it('loads earlier records with the section count always equal to visible table rows', async () => {
    const older = event('older', 'execution.report', WORK_B, KNOWN_ACTOR, 20);
    setupActivity({ developmentPages: [activityPayload([CATEGORY_EVENTS[0]], 'synthetic-next-page'), activityPayload([older])] });
    renderApp('/activity?project=demo');
    await waitFor(() => expect(tableRows()).toHaveLength(1));
    expect(sectionCount()).toBe('1');
    expect(screen.getByTestId('activity-load-earlier').closest('.activity-footer-controls')).toBeTruthy();
    await userEvent.setup().click(screen.getByTestId('activity-load-earlier'));
    await waitFor(() => expect(tableRows()).toHaveLength(2));
    expect(sectionCount()).toBe(String(tableRows().length));
    expect(screen.queryByTestId('activity-scope')).toBeNull();
    expect(screen.queryByTestId('freshness-badge')).toBeNull();
  });

  it('pins the exact time in the cell title to the viewer\'s own timezone', async () => {
    // 2026-11-20T04:30:05Z: 23:30 the previous day in New York, a different calendar
    // day from UTC, so a title formatted in any other zone cannot pass by accident.
    const at = Date.UTC(2026, 10, 20, 4, 30, 5);
    const realFormat = Intl.DateTimeFormat;
    // Libraries call Intl.DateTimeFormat both with and without `new`; the wrapper has to
    // survive both and only lie about the viewer's resolved timezone.
    const viewerTimeZone = function (this: unknown, ...args: unknown[]) {
      const instance = new (realFormat as new (...a: unknown[]) => Intl.DateTimeFormat)(...args);
      return new Proxy(instance, {
        get(target, property) {
          if (property === 'resolvedOptions') {
            return () => ({ ...target.resolvedOptions(), timeZone: 'America/New_York' });
          }
          const value = Reflect.get(target, property, target);
          return typeof value === 'function' ? value.bind(target) : value;
        },
      });
    } as unknown as typeof Intl.DateTimeFormat;
    vi.stubGlobal('Intl', Object.assign(Object.create(Intl), Intl, { DateTimeFormat: viewerTimeZone }));
    try {
      setupActivity({ development: activityPayload([{ ...event('tz', 'execution.report', WORK_A, KNOWN_ACTOR, 0), created_at_unix_ms: at }]) });
      const view = renderApp('/activity?project=demo');
      await waitFor(() => expect(tableRows()).toHaveLength(1));
      const title = tableRows()[0].querySelector('td:first-child .activity-time')?.getAttribute('title');
      const exact = (timeZone: string) => new realFormat('zh-CN', {
        year: 'numeric', month: '2-digit', day: '2-digit',
        hour: '2-digit', minute: '2-digit', second: '2-digit', hour12: false, timeZone,
      }).format(new Date(at));
      expect(exact('UTC')).not.toBe(exact('America/New_York'));
      expect(title).toBe(exact('America/New_York'));
      view.unmount();
    } finally {
      vi.unstubAllGlobals();
    }
  });

  it('renders a deep-linked detail without internal fields and explains an unknown result', async () => {
    const unknown = { ...event('unknown', 'audit.requests', null, KNOWN_ACTOR, 0), result: 'unknown' };
    setupActivity({ requests: activityPayload([unknown]) });
    renderApp('/activity?project=demo&kind=requests&event=unknown');
    const detail = await screen.findByTestId('activity-event-detail');
    expect(detail.textContent).toContain('Alex Example');
    expect(detail.textContent).toContain('读取访问记录');
    expect(detail.textContent).toContain('结果未知');
    expect(screen.getByTestId('activity-unknown-result').textContent)
      .toBe('还没有完成记录；请先核对原业务请求，再决定是否重试');
    for (const hidden of ['unknown_means', 'actor_id', 'client_id', 'credential_id', 'audit.requests', unknown.id]) {
      expect(defaultVisibleText(detail)).not.toContain(hidden);
    }
    expect(document.querySelector('[data-testid="technical-details"]')).toBeNull();
  });

  it('keeps a machine-shaped client name out of the sentence', async () => {
    const items = [
      event('machine-client', 'execution.report', WORK_A, KNOWN_ACTOR, 0, {
        name: 'Alex Example', client: 'observer-host-native-20261031T203834Z', kind: 'agent',
      }),
    ];
    setupActivity({ development: activityPayload(items) });
    renderApp('/activity?project=demo');
    await waitFor(() => expect(tableRows()).toHaveLength(1));
    expect(tableRows()[0].querySelector('td:nth-child(2)')?.textContent).toBe('Alex Example');
  });

  it('words an unread and a partial context differently and ends a self-scope read with that note alone', async () => {
    const cases: Array<[string, { taskCoverage: string; memberCoverage: string }, string]> = [
      ['unread tasks', { taskCoverage: 'unread', memberCoverage: 'complete' }, '数据读取情况：任务和成员资料尚未读取；姓名和任务名可能显示为未登记'],
      ['partial members', { taskCoverage: 'complete', memberCoverage: 'partial' }, '数据读取情况：部分任务或成员资料没有读全；姓名和任务名只按已读取内容翻译'],
    ];
    for (const [name, coverage, expected] of cases) {
      setupActivity({ overview: overviewModel(coverage) });
      const view = renderApp('/activity?project=demo');
      await waitFor(() => expect(tableRows()).toHaveLength(8));
      expect((await screen.findByTestId('activity-coverage')).textContent, name).toBe(expected);
      view.unmount();
    }
    const selfScope = activityPayload([event('self', 'execution.report', WORK_A, KNOWN_ACTOR, 0)], null);
    (selfScope.data as { scope?: string }).scope = 'self';
    setupActivity({ development: selfScope });
    renderApp('/activity?project=demo');
    // The task title proves the context read has finished, so the absences below are meaningful.
    await waitFor(() => expect(tableRows()[0]?.textContent).toContain('Synthetic task alpha'));
    expect(screen.queryByTestId('activity-coverage')).toBeNull();
    expect(screen.getByTestId('activity-scope').parentElement?.textContent).toBe('当前只显示你自己的记录');
  });

  it('shows the freshness badge for a failed plane as well as a stale one', async () => {
    const failed = overviewModel();
    (failed.freshness as { authoritative?: { status: string } }).authoritative = { status: 'failed' };
    setupActivity({ overview: failed });
    renderApp('/activity?project=demo');
    expect(await screen.findByTestId('freshness-badge')).toBeTruthy();
  });

  it('gives the time column the numeric style and shows no section count before the read', async () => {
    setupActivity();
    renderApp('/activity?project=demo');
    expect(sectionCount()).toBeUndefined();
    await waitFor(() => expect(tableRows()).toHaveLength(8));
    expect(tableRows()[0].querySelector('td:first-child')?.classList.contains('obs-table-numeric-column')).toBe(true);
    expect(sectionCount()).toBe('8');
  });
});

describe.each([
  { language: 'zh-CN', loading: '正在读取活动记录', empty: '这个类别下没有记录。', failed: '活动记录暂时读取不到', partial: '部分活动记录暂时读取不到', forbidden: '暂时无法查看', unknown: '结果未知' },
  { language: 'en-US', loading: 'Loading activity', empty: 'No records in this category.', failed: 'Activity cannot be loaded', partial: 'Some activity could not be loaded', forbidden: 'Access unavailable', unknown: 'Result unknown' },
])('activity read states in $language', ({ language, loading, empty, failed, partial, forbidden, unknown }) => {
  it('keeps data, loading, empty and failed reads inside the designed page', async () => {
    await changeLanguage(language);
    let release!: () => void;
    const gate = new Promise<void>((resolve) => { release = resolve; });
    setupActivity({ activityGate: gate });
    const pending = renderApp('/activity?project=demo');
    expect((await screen.findByTestId('activity-loading')).textContent).toContain(loading);
    pending.unmount();
    await act(async () => { release(); await gate; });

    setupActivity({ development: activityPayload([]) });
    const emptyView = renderApp('/activity?project=demo');
    expect((await screen.findByTestId('activity-empty')).textContent).toBe(empty);
    expect(screen.queryByTestId('freshness-badge')).toBeNull();
    emptyView.unmount();

    const failure = { current: true };
    setupActivity({ activityFailure: failure });
    const failedView = renderApp('/activity?project=demo');
    expect((await screen.findByTestId('activity-read-error')).textContent).toContain(failed);
    expect(screen.getByTestId('activity-read-error').textContent).not.toContain('Unavailable');
    failure.current = false;
    failedView.unmount();

    setupActivity({ development: activityPayload([{ ...CATEGORY_EVENTS[0], result: 'unknown' }]) });
    renderApp('/activity?project=demo&event=delivery');
    expect((await screen.findByTestId('activity-unknown-result')).textContent.length).toBeGreaterThan(0);
    expect(screen.getAllByText(unknown).length).toBeGreaterThan(0);
  });

  it('retains confirmed rows on partial failure and explains partial or unread context coverage', async () => {
    await changeLanguage(language);
    setupActivity({
      developmentPages: [activityPayload([CATEGORY_EVENTS[0]], 'synthetic-next-page'), activityPayload([CATEGORY_EVENTS[1]])],
      secondPageFailure: true,
      overview: overviewModel({ taskCoverage: 'unread', memberCoverage: 'partial' }),
    });
    renderApp('/activity?project=demo');
    await waitFor(() => expect(tableRows()).toHaveLength(1));
    expect(screen.getByTestId('activity-coverage').textContent).toMatch(language === 'zh-CN' ? /尚未读取|没有读全/ : /not been read|incomplete/i);
    await userEvent.setup().click(screen.getByTestId('activity-load-earlier'));
    expect((await screen.findByTestId('activity-partial')).textContent).toContain(partial);
    expect(tableRows()).toHaveLength(1);
    expect(sectionCount()).toBe('1');
  });

  it('clears rows and stops activity polling immediately after revocation', async () => {
    await changeLanguage(language);
    const { calls } = setupActivity();
    const { queryClient } = renderApp('/activity?project=demo');
    await waitFor(() => expect(tableRows()).toHaveLength(8));
    act(() => revokeObservationReads(queryClient));
    expect((await screen.findByTestId('state-forbidden')).textContent).toContain(forbidden);
    expect(tableRows()).toHaveLength(0);
    const settled = calls.length;
    await new Promise((resolve) => setTimeout(resolve, 5_300));
    expect(calls.length).toBe(settled);
  }, 10_000);
});

describe('display clock: the read\'s own clock in the shell-pinned zone', () => {
  // The canonical fixture's own timestamps: its derivation "now" is 2026-11-19 12:00 UTC
  // and the prototype's ledger rows read exactly these wall-clock times in UTC.
  const FIXTURE_NOW = Date.UTC(2026, 10, 19, 12, 0);
  const at = (unixMs: number): TeamActivityItem => ({
    id: `clock-${unixMs}`,
    action: 'execution.report',
    actor_id: KNOWN_ACTOR,
    client_id: null,
    actor: { person_id: KNOWN_PERSON, display_name: 'Alex Example' },
    client: { display_name: null },
    created_at_unix_ms: unixMs,
    credential_id: null,
    result: 'committed',
    work_id: WORK_A,
  });
  const CANONICAL_ITEMS = [
    at(FIXTURE_NOW - 12 * 60_000),
    at(Date.UTC(2026, 10, 19, 9, 40)),
    at(Date.UTC(2026, 10, 18, 15, 20)),
    at(Date.UTC(2026, 10, 18, 10, 2)),
  ];

  it('rides the fixture clock answered right now, so the times read like the prototype', async () => {
    // The overview reply carries the fixture derivation clock and was read at the real
    // present; the shell pins the display zone to UTC like the canonical demo does.
    setupActivity({
      shell: { ok: true, source: 'demo', organizationName: null, perspective: null, displayTimeZone: 'UTC' },
      development: activityPayload(CANONICAL_ITEMS),
      overview: overviewModel({ clock: { nowUnixMs: FIXTURE_NOW, readAtUnixMs: Date.now() } }),
    });
    renderApp('/activity?project=demo');
    await waitFor(() => expect(tableRows()).toHaveLength(4));
    const times = tableRows().map((row) => row.querySelector('.activity-time')?.textContent);
    expect(times).toEqual(['12 分钟前', '今天 09:40', '昨天 15:20', '11 月 18 日 10:02']);
  });

  it('falls back to the viewer clock while no overview read has answered', async () => {
    vi.useFakeTimers({ toFake: ['Date'], shouldAdvanceTime: true });
    try {
      vi.setSystemTime(FIXTURE_NOW + 20 * 60_000);
      setupActivity({
        overviewFailure: { current: true },
        development: activityPayload(CANONICAL_ITEMS),
      });
      renderApp('/activity?project=demo');
      await waitFor(() => expect(tableRows()).toHaveLength(4));
      // No read clock exists, so the viewer clock (faked here) is the reference; the zone
      // is whatever the viewer runs in, so the calendar rows are asserted through the same
      // shared formatter against that zone rather than hardcoded strings.
      const zone = Intl.DateTimeFormat().resolvedOptions().timeZone;
      const expected = CANONICAL_ITEMS.map((item) => compactTimeText(item.created_at_unix_ms, {
        language: 'zh-CN', nowUnixMs: FIXTURE_NOW + 20 * 60_000, timeZone: zone,
      }));
      expect(tableRows().map((row) => row.querySelector('.activity-time')?.textContent)).toEqual(expected);
      expect(expected[0]).toBe('32 分钟前');
    } finally {
      vi.useRealTimers();
    }
  });

  it('keeps the viewer\'s own zone when the shell pins none', async () => {
    setupActivity({
      development: activityPayload(CANONICAL_ITEMS),
      overview: overviewModel({ clock: { nowUnixMs: FIXTURE_NOW, readAtUnixMs: Date.now() } }),
    });
    renderApp('/activity?project=demo');
    await waitFor(() => expect(tableRows()).toHaveLength(4));
    // The reference clock still rides the read's (fixture) clock; only the zone differs, so
    // the calendar rows read the viewer zone's own wording through the same formatter.
    const zone = Intl.DateTimeFormat().resolvedOptions().timeZone;
    const expected = CANONICAL_ITEMS.map((item) => compactTimeText(item.created_at_unix_ms, {
      language: 'zh-CN', nowUnixMs: FIXTURE_NOW, timeZone: zone,
    }));
    expect(tableRows().map((row) => row.querySelector('.activity-time')?.textContent)).toEqual(expected);
    expect(expected[0]).toBe('12 分钟前');
  });

  it('reads a zone the runtime cannot resolve as unpinned', async () => {
    // The pinned zone is a fact from the bridge; a bogus value fails closed to the
    // viewer's own zone instead of throwing or being trusted verbatim.
    setupActivity({
      shell: { ok: true, source: 'demo', organizationName: null, perspective: null, displayTimeZone: 'Not/AZone' },
      development: activityPayload(CANONICAL_ITEMS),
      overview: overviewModel({ clock: { nowUnixMs: FIXTURE_NOW, readAtUnixMs: Date.now() } }),
    });
    renderApp('/activity?project=demo');
    await waitFor(() => expect(tableRows()).toHaveLength(4));
    const zone = Intl.DateTimeFormat().resolvedOptions().timeZone;
    const expected = CANONICAL_ITEMS.map((item) => compactTimeText(item.created_at_unix_ms, {
      language: 'zh-CN', nowUnixMs: FIXTURE_NOW, timeZone: zone,
    }));
    expect(tableRows().map((row) => row.querySelector('.activity-time')?.textContent)).toEqual(expected);
  });
});

describe('verbatim live activity replies', () => {
  it.each(['zh-CN', 'en-US'])('renders development and request replies without raw fields or empty-value artifacts in %s', async (language) => {
    await changeLanguage(language);
    for (const [kind, fixture] of [
      ['development', 'observer-activity-development.json'],
      ['requests', 'observer-activity-requests.json'],
    ] as const) {
      const payload = liveFixture(fixture);
      setupActivity(kind === 'development' ? { development: payload } : { requests: payload });
      const view = renderApp(`/activity?project=demo&kind=${kind}`);
      await screen.findByTestId('page-activity');
      await waitFor(() => expect(document.querySelector('.activity-loading')).toBeNull());
      const text = defaultVisibleText(screen.getByTestId('page-activity'));
      expect(text).not.toMatch(/\b(?:undefined|null|NaN)\b/);
      for (const raw of ['actor_id', 'client_id', 'credential_id', 'work_id', 'created_at_unix_ms', 'unknown_means', 'development_source']) {
        expect(text).not.toContain(raw);
      }
      expect(findForbiddenVisibleText(text)).toEqual([]);
      view.unmount();
    }
  });
});
