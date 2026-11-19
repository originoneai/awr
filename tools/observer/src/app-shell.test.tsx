import { screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';
import i18n from './i18n';
import { DEMO_OVERVIEW, DEMO_PROJECTS, DEMO_WORK_NEXT, renderApp, stubFetch } from './test-utils';

const CURRENT_MEMBER_OVERVIEW = {
  ...DEMO_OVERVIEW,
  identity: { actor_id: 'member-1', role: 'developer', business_roles: ['developer'] },
  members: [{ person_id: 'member-1', display_name: '陈舟' }],
};

function json(payload: unknown) {
  return new Response(JSON.stringify(payload), { status: 200, headers: { 'content-type': 'application/json' } });
}

function decision(key: string) {
  return {
    decision_id: key,
    work_id: 'work-1',
    workstream_id: 'stream-1',
    work_title: 'Synthetic work',
    question: 'Should the synthetic copy stay?',
    impact: 'Synthetic impact',
    reply: null,
    state: 'open',
    baseline_matches_current: true,
    requester: { person_id: 'member-2', display_name: '周宁' },
    allowed_responders: [{ person_id: 'member-1', display_name: '陈舟' }],
    resolution: null,
    created_at_unix_ms: 1761840000000,
  };
}

function stubShell({ count = 0, nextCursor = null, overview = CURRENT_MEMBER_OVERVIEW }: {
  count?: number;
  nextCursor?: string | null;
  overview?: typeof CURRENT_MEMBER_OVERVIEW;
} = {}) {
  const fetchMock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
    const url = String(input);
    if (url.includes('/api/team/projects')) return json(DEMO_PROJECTS);
    if (url.includes('/api/team/overview')) return json(overview);
    if (url.includes('/api/team/read')) {
      const query = JSON.parse(String(init?.body)).query;
      if (query.op === 'decisions.list') {
        return json({
          ok: true,
          result: {
            data: {
              items: Array.from({ length: count }, (_, index) => decision(`decision-${index + 1}`)),
              next_cursor: nextCursor,
              truncated: nextCursor !== null,
              execution_authorized: false,
              as_of: { at_unix_ms: 1761840000000 },
            },
          },
        });
      }
      if (query.op === 'work.next') return json(DEMO_WORK_NEXT);
    }
    return json({ ok: false, error: { code: 'NoRoute', message: 'Synthetic missing read' } });
  });
  vi.stubGlobal('fetch', fetchMock);
  return fetchMock;
}

afterEach(async () => {
  await i18n.changeLanguage('zh-CN');
});

describe('AppShell navigation', () => {
  it('renders the 240px sidebar with primary and administration groups and bottom theme switch', () => {
    stubFetch({ '/api/team/projects': DEMO_PROJECTS, '/api/team/overview': DEMO_OVERVIEW });
    renderApp('/workbench');
    const sider = document.querySelector<HTMLElement>('.shell-sider');
    expect(sider?.style.width).toBe('240px');
    expect(sider?.querySelector('[data-testid="awr-brand"]')).not.toBeNull();
    for (const label of ['工作台', '项目', '待处理', '经验', '成员与 Agent', '活动与审计', '管理设置']) {
      expect(screen.getByRole('menuitem', { name: new RegExp(label) })).toBeTruthy();
    }
    expect(screen.getByText('管理', { selector: '.shell-side-label' })).toBeTruthy();
    expect(screen.queryByRole('menuitem', { name: /账户与连接|高级工具|知识与技能/ })).toBeNull();
    const inner = document.querySelector('.shell-sider-inner');
    expect(inner?.lastElementChild?.classList.contains('shell-theme')).toBe(true);
    expect(inner?.lastElementChild?.querySelector('[data-testid="theme-toggle"]')).not.toBeNull();
    expect(document.querySelector('.ant-menu-item-selected')?.classList.contains('ant-menu-item-selected')).toBe(true);
  });

  it('uses the page-read open-item count and dot, then opens the inbox from the bell', async () => {
    stubShell({ count: 2 });
    const user = userEvent.setup();
    const { router } = renderApp('/inbox?scope=demo');
    expect((await screen.findByTestId('inbox-count')).textContent).toBe('2');
    expect(document.querySelectorAll('.ant-badge-dot')).toHaveLength(1);
    await user.click(screen.getByRole('link', { name: '工作台' }));
    await waitFor(() => expect(router.state.location.pathname).toBe('/workbench'));
    await user.click(screen.getByRole('button', { name: '待处理 2 件' }));
    expect(router.state.location.pathname).toBe('/inbox');
    expect(router.state.location.search).toBe('?scope=demo');
  });

  it('marks a partially read pending count instead of presenting it as a total', async () => {
    stubShell({ count: 10, nextCursor: 'synthetic-next-page' });
    renderApp('/inbox?scope=demo');
    expect((await screen.findByTestId('inbox-count')).textContent).toBe('10+');
  });

  it('keeps every ancestor breadcrumb clickable and truncates only the current page', async () => {
    stubShell();
    const user = userEvent.setup();
    const { router } = renderApp('/projects/demo?tab=plan');
    await waitFor(() => expect(screen.getByTestId('scope-switcher').textContent).toContain('Team mainline'));
    const breadcrumbs = screen.getByRole('navigation', { name: '所在位置' });
    expect(breadcrumbs.querySelector('[data-testid="scope-switcher"]')).not.toBeNull();
    expect(breadcrumbs.querySelector('a[href="/projects?scope=demo"]')).not.toBeNull();
    const projectLink = screen.getByRole('link', { name: 'Team mainline' });
    expect(projectLink.getAttribute('href')).toBe('/projects/demo');
    const current = screen.getByText<HTMLElement>('工作计划', { selector: '[aria-current="page"]' });
    expect(current.style.textOverflow).toBe('ellipsis');
    expect(current.style.overflow).toBe('hidden');
    expect(current.style.whiteSpace).toBe('nowrap');
    await user.click(projectLink);
    expect(router.state.location.pathname).toBe('/projects/demo');
    expect(router.state.location.search).toBe('');
  });

  it('uses business labels for unresolved detail routes rather than route identifiers', () => {
    stubShell();
    renderApp('/tasks/private-task?scope=demo');
    expect(screen.getByText('任务', { selector: '[aria-current="page"]' })).toBeTruthy();
    expect(screen.queryByText('private-task')).toBeNull();
  });

  it('provides the search and perspective slots without pretending they are implemented', () => {
    stubFetch({ '/api/team/projects': DEMO_PROJECTS, '/api/team/overview': DEMO_OVERVIEW });
    renderApp('/workbench');
    expect(screen.getByRole('button', { name: /搜索/ })).toHaveProperty('disabled', true);
    expect(screen.getByTestId('perspective-switcher')).toHaveProperty('disabled', true);
    expect(screen.getByRole('button', { name: '待处理' })).toHaveProperty('disabled', false);
    expect(screen.queryByTestId('inbox-count')).toBeNull();
    expect(document.querySelector('.ant-badge-dot')).toBeNull();
  });

  it('uses the current member initial when the page read matches the current identity', async () => {
    stubShell();
    renderApp('/projects/demo?tab=overview');
    const avatar = await screen.findByTestId('current-member-avatar');
    await waitFor(() => expect(avatar.textContent).toBe('陈'));
    expect(avatar.getAttribute('aria-label')).toBe('当前成员：陈舟');
  });

  it('adds no shell-only read and falls back without exposing an actor identifier', async () => {
    const fetchMock = stubShell({ overview: { ...CURRENT_MEMBER_OVERVIEW, members: [] } });
    renderApp('/workbench?scope=demo');
    const avatar = await screen.findByTestId('current-member-avatar');
    await screen.findByTestId('freshness-badge');
    expect(avatar.textContent).toBe('你');
    expect(avatar.textContent).not.toContain('member-1');
    const urls = fetchMock.mock.calls.map(([input]) => String(input));
    expect(urls.some((url) => url.includes('/api/team/overview'))).toBe(false);
    const operations = fetchMock.mock.calls.filter(([input]) => String(input).includes('/api/team/read'))
      .map(([, init]) => JSON.parse(String(init?.body)).query.op as string);
    expect(operations).not.toContain('decisions.list');
  });

  it('keeps page read coverage and freshness inside the unchanged page content', async () => {
    stubShell();
    renderApp('/workbench?scope=demo');
    const freshness = await screen.findByTestId('freshness-badge');
    expect(freshness.closest('.shell-content')).not.toBeNull();
    expect(document.querySelector('.shell-header')?.contains(freshness)).toBe(false);
  });

  it('preserves the selected project when opening the inbox from a project page', async () => {
    stubShell({ count: 1 });
    const user = userEvent.setup();
    const { router } = renderApp('/projects/demo');
    await waitFor(() => expect(screen.getByTestId('scope-switcher').textContent).toContain('Team mainline'));
    const inbox = screen.getByRole('link', { name: /待处理/ });
    expect(inbox.getAttribute('href')).toBe('/inbox?scope=demo');
    await user.click(inbox);
    expect(router.state.location.pathname).toBe('/inbox');
    expect(router.state.location.search).toBe('?scope=demo');
  });
});
