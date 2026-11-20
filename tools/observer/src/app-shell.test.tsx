import { readFileSync } from 'node:fs';
import { screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';
import i18n from './i18n';
import { DEMO_OVERVIEW, DEMO_PROJECTS, DEMO_WORK_NEXT, renderApp, stubFetch } from './test-utils';
import { craftTokens } from './theme';

// Vitest disables CSS imports, so load the real shell stylesheet for computed-display assertions.
const shellStyles = document.createElement('style');
shellStyles.textContent = readFileSync('src/global.css', 'utf8');
document.head.append(shellStyles);

const CURRENT_MEMBER_OVERVIEW = {
  ...DEMO_OVERVIEW,
  identity: { actor_id: 'member-1', role: 'developer', business_roles: ['developer'] },
  members: [{ person_id: 'member-1', display_name: '陈舟' }],
};

const INBOX_AT = 1761840000000;

/** A real-shaped overview-model reply whose inbox holds `count` pending decisions. */
function inboxOverviewModel({ count = 0, partial = false }: { count?: number; partial?: boolean } = {}) {
  const complete = (items: unknown[]) => ({ coverage: 'complete', items });
  const part = (status: string) => ({ status, missing: [], ...(status === 'complete' ? {} : { reason: 'not-read' }) });
  return {
    ok: true,
    project: 'demo',
    read_at_unix_ms: INBOX_AT,
    input: {
      tasks: complete([{
        id: 'work-1', title: 'Synthetic work', ownerPersonId: 'member-1', agentName: null,
        acceptance: [], report: { completed: false, reportedAtUnixMs: INBOX_AT }, submittedResultAtUnixMs: null,
        integrated: false, started: true, agentActive: false, externalWait: null,
      }]),
      dependencies: complete([]),
      decisions: complete(Array.from({ length: count }, (_, index) => ({
        id: `inbox-decision-${index + 1}`, taskId: 'work-1', state: 'open', current: true,
        question: `Synthetic question ${index + 1}`, impact: null,
        allowedResponderIds: ['member-1'], createdAtUnixMs: INBOX_AT - 86_400_000,
      }))),
      reviewRounds: complete([]),
      handoffs: complete([]),
      approvals: partial
        ? { coverage: 'unread' as const, items: [], reason: 'not-read' as const }
        : complete([]),
      members: complete([{ personId: 'member-1', displayName: '陈舟' }]),
      activity: complete([]),
      options: {
        nowUnixMs: INBOX_AT, stalledAfterDays: 3,
        views: { me: { personId: 'member-1', projectId: 'demo' } },
      },
    },
    coverage: {
      tasks: part('complete'), dependencies: part('complete'),
      decisions: part('complete'), reviewRounds: part('complete'), handoffs: part('complete'),
      approvals: partial ? part('unread') : part('complete'),
      members: part('complete'), activity: part('complete'),
    },
    freshness: {
      report_received: { status: 'not_measured' },
      authoritative: { status: 'measured', at_unix_ms: INBOX_AT },
      page_read: { status: 'measured', at_unix_ms: INBOX_AT },
    },
  };
}

function json(payload: unknown) {
  return new Response(JSON.stringify(payload), { status: 200, headers: { 'content-type': 'application/json' } });
}

function stubShell({ count = 0, partial = false, overview = CURRENT_MEMBER_OVERVIEW }: {
  count?: number;
  partial?: boolean;
  overview?: typeof CURRENT_MEMBER_OVERVIEW;
} = {}) {
  const fetchMock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
    const url = String(input);
    if (url.includes('/api/team/projects')) return json(DEMO_PROJECTS);
    if (url.includes('/api/team/overview-model')) return json(inboxOverviewModel({ count, partial }));
    if (url.includes('/api/team/overview')) return json(overview);
    if (url.includes('/api/team/read')) {
      const query = JSON.parse(String(init?.body)).query;
      if (query.op === 'work.next') return json(DEMO_WORK_NEXT);
    }
    return json({ ok: false, error: { code: 'NoRoute', message: 'Synthetic missing read' } });
  });
  vi.stubGlobal('fetch', fetchMock);
  return fetchMock;
}

afterEach(async () => {
  document.documentElement.dataset.theme = 'light';
  await i18n.changeLanguage('zh-CN');
});

describe('AppShell navigation', () => {
  it('renders the 232px sidebar with primary and administration groups and bottom theme switch', () => {
    stubFetch({ '/api/team/projects': DEMO_PROJECTS, '/api/team/overview': DEMO_OVERVIEW });
    renderApp('/workbench');
    const sider = document.querySelector<HTMLElement>('.shell-sider');
    expect(sider?.style.width).toBe('232px');
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

  it('matches the owner image brand and prototype navigation geometry', () => {
    stubFetch({ '/api/team/projects': DEMO_PROJECTS, '/api/team/overview': DEMO_OVERVIEW });
    renderApp('/workbench');
    const brand = screen.getByTestId('awr-brand');
    expect(craftTokens.brand).toEqual({
      rowHeight: 32,
      markWidth: 24,
      markHeight: 18.75,
      gap: 10,
      paddingInline: 12,
      marginBottom: 20,
      wordmarkFontSize: 17,
      wordmarkFontWeight: 600,
      wordmarkLineHeight: 20,
    });
    // jsdom returns custom-property expressions; Chromium resolves their exact pixel values in e2e.
    const brandStyles = getComputedStyle(brand);
    expect(brandStyles.height).toBe('var(--craft-brand-row-height)');
    expect(brandStyles.marginBottom).toBe('var(--craft-brand-margin-bottom)');
    expect(brandStyles.gap).toBe('var(--craft-brand-gap)');
    for (const testId of ['brand-mark-color', 'brand-mark-white']) {
      const mark = screen.getByTestId<HTMLImageElement>(testId);
      expect(mark.getAttribute('width')).toBe('24');
      expect(mark.getAttribute('height')).toBe('18.75');
      expect(mark.alt).toBe('');
      expect(mark.getAttribute('aria-hidden')).toBe('true');
    }
    expect(brand.querySelector('svg')).toBeNull();
    expect(brand.querySelectorAll('img')).toHaveLength(2);
    const wordmarkStyles = getComputedStyle(screen.getByTestId('brand-wordmark'));
    expect(wordmarkStyles.fontSize).toBe('var(--craft-brand-wordmark-font-size)');
    expect(wordmarkStyles.fontWeight).toBe('var(--craft-brand-wordmark-font-weight)');
    expect(wordmarkStyles.lineHeight).toBe('var(--craft-brand-wordmark-line-height)');
    const menu = document.querySelector<HTMLElement>('.shell-nav');
    expect(menu?.style.display).toBe('grid');
    expect(menu?.style.gap).toBe('2px');
    expect(document.querySelector<HTMLElement>('.shell-nav .ant-menu-item')?.style.paddingInline).toBe('12px');
    const label = document.querySelector<HTMLElement>('.shell-side-label');
    expect(label?.style.padding).toBe('24px 12px 8px');
    expect(label?.style.fontSize).toBe('12px');
    expect(label?.style.lineHeight).toBe('16px');
  });

  it('uses CSS alone to show only the color mark in light and only the white mark in dark', () => {
    stubFetch({ '/api/team/projects': DEMO_PROJECTS, '/api/team/overview': DEMO_OVERVIEW });
    renderApp('/workbench');
    const color = screen.getByTestId<HTMLImageElement>('brand-mark-color');
    const white = screen.getByTestId<HTMLImageElement>('brand-mark-white');
    document.documentElement.dataset.theme = 'light';
    expect(getComputedStyle(color).display).toBe('inline-block');
    expect(getComputedStyle(white).display).toBe('none');
    document.documentElement.dataset.theme = 'dark';
    expect(getComputedStyle(color).display).toBe('none');
    expect(getComputedStyle(white).display).toBe('inline-block');

    const expected = {
      color: '/awr-logo-color-72.png 1x, /awr-logo-color-144.png 2x, /awr-logo-color-288.png 4x',
      white: '/awr-logo-white-72.png 1x, /awr-logo-white-144.png 2x, /awr-logo-white-288.png 4x',
    };
    for (const [mark, variant] of [[color, 'color'], [white, 'white']] as const) {
      const src = mark.getAttribute('src') ?? '';
      const srcset = mark.getAttribute('srcset') ?? '';
      expect(src).toBe(`/awr-logo-${variant}-72.png`);
      expect(srcset).toBe(expected[variant]);
      for (const candidate of [src, ...srcset.split(', ').map((entry) => entry.split(' ')[0])]) {
        expect(candidate).toMatch(/^\/(?!\/)/);
        expect(new URL(candidate, window.location.href).origin).toBe(window.location.origin);
      }
      expect(`${src} ${srcset}`).not.toMatch(/https?:|data:/i);
    }
  });

  it('uses the shared inbox model count and dot, then opens the inbox from the bell', async () => {
    stubShell({ count: 2 });
    const user = userEvent.setup();
    const { router } = renderApp('/inbox?scope=demo');
    expect((await screen.findByTestId('inbox-count')).textContent).toBe('2');
    expect(document.querySelectorAll('.ant-badge-dot')).toHaveLength(1);
    expect(screen.getByTestId('inbox-queue').textContent).toContain('Synthetic question 1');
    await user.click(screen.getByRole('link', { name: '工作台' }));
    await waitFor(() => expect(router.state.location.pathname).toBe('/workbench'));
    await user.click(screen.getByRole('button', { name: '待处理 2 件' }));
    expect(router.state.location.pathname).toBe('/inbox');
    expect(router.state.location.search).toBe('?scope=demo');
  });

  it('renders the pending count as the prototype accent pill', async () => {
    stubShell({ count: 2 });
    renderApp('/inbox?scope=demo');
    const count = await screen.findByTestId('inbox-count');
    expect(count.style.minWidth).toBe('20px');
    expect(count.style.height).toBe('20px');
    expect(count.style.padding).toBe('0px 6px');
    expect(count.style.borderRadius).toBe('10px');
    expect(count.style.background).toBe('var(--craft-color-accent)');
    expect(count.style.color).toBe('var(--craft-color-on-accent)');
    expect(count.style.fontSize).toBe('12px');
    expect(count.style.lineHeight).toBe('20px');
    expect(count.style.textAlign).toBe('center');
    expect(count.style.fontVariantNumeric).toBe('tabular-nums');
  });

  it('marks a partially read pending count instead of presenting it as a total', async () => {
    stubShell({ count: 10, partial: true });
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

  it('enables search and hides the single available perspective', () => {
    stubFetch({ '/api/team/projects': DEMO_PROJECTS, '/api/team/overview': DEMO_OVERVIEW });
    renderApp('/workbench');
    expect(screen.getByRole('button', { name: /搜索/ })).toHaveProperty('disabled', false);
    expect(screen.queryByTestId('perspective-switcher')).toBeNull();
    expect(screen.queryByRole('button', { name: '做事的人' })).toBeNull();
    expect(screen.getByRole('button', { name: '待处理' })).toHaveProperty('disabled', false);
    expect(screen.queryByTestId('inbox-count')).toBeNull();
    expect(document.querySelector('.ant-badge-dot')).toBeNull();
  });

  it('renders no limitation text or perspective chrome in the real header with unknown facts', () => {
    stubFetch({ '/api/team/projects': DEMO_PROJECTS, '/api/team/overview': DEMO_OVERVIEW });
    renderApp('/workbench');
    const header = document.querySelector('.shell-header');
    expect(header?.querySelector('.shell-perspective')).toBeNull();
    for (const text of ['视角', '我的工作', '小组职责与项目关系未知', '小组与部门登记情况未知', '尚未登记部门']) {
      expect(header?.textContent).not.toContain(text);
    }
  });

  it('uses the current member initial when the page read matches the current identity', async () => {
    stubShell();
    renderApp('/projects/demo?tab=overview');
    const avatar = await screen.findByTestId('current-member-avatar');
    await waitFor(() => expect(avatar.textContent).toBe('陈'));
    expect(screen.getByTestId('avatar-menu-trigger').getAttribute('aria-label')).toBe('陈舟：账户与语言');
  });

  it('offers account, Agent connection, and language entries from the avatar menu', async () => {
    stubShell();
    const user = userEvent.setup();
    renderApp('/workbench?scope=demo');
    await user.click(await screen.findByTestId('avatar-menu-trigger'));
    expect((await screen.findByRole('link', { name: '账户与连接' })).getAttribute('href')).toBe('/account?scope=demo');
    expect(screen.getAllByRole('link', { name: '连接我的 Agent' })
      .some((link) => link.getAttribute('href') === '/agent-connect?scope=demo')).toBe(true);
    await user.click(screen.getByText('English'));
    await waitFor(() => expect(i18n.language).toBe('en-US'));
    expect(screen.getByRole('menuitem', { name: /Workbench/ })).toBeTruthy();
  });

  it('reads only the shared inbox model for shell indicators and never falls back to an actor identifier', async () => {
    const fetchMock = stubShell({ overview: { ...CURRENT_MEMBER_OVERVIEW, members: [] } });
    renderApp('/workbench?scope=demo');
    const avatar = await screen.findByTestId('current-member-avatar');
    await screen.findByTestId('freshness-badge');
    expect(avatar.textContent).toBe('你');
    expect(avatar.textContent).not.toContain('member-1');
    const urls = fetchMock.mock.calls.map(([input]) => String(input));
    expect(urls.some((url) => url.includes('/api/team/overview?'))).toBe(false);
    const operations = fetchMock.mock.calls.filter(([input]) => String(input).includes('/api/team/read'))
      .map(([, init]) => JSON.parse(String(init?.body)).query.op as string);
    expect(operations).not.toContain('decisions.list');
    // The shell's pending pill and the inbox page read the same composed model, nothing else.
    expect(urls.some((url) => url.includes('/api/team/overview-model?project='))).toBe(true);
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
