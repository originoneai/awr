import { App as AntdApp } from 'antd';
import { act, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { axe } from 'jest-axe';
import { createMemoryRouter, RouterProvider } from 'react-router';
import { afterEach, describe, expect, it, vi } from 'vitest';
import i18n from '../i18n';
import { revokeObservationReads } from '../lib/queries';
import type { SearchItem, SearchSource } from '../lib/search-index';
import { searchSourcesFromReads } from '../lib/search-sources';
import type { TeamOverviewModelResponse } from '../lib/types';
import { DEMO_PROJECTS, renderApp, stubFetch } from '../test-utils';
import { CraftThemeProvider } from './CraftThemeProvider';
import { SearchDialog, SEARCH_RECENT_STORAGE_KEY } from './SearchPanel';

function task(index: number, title = `任务 ${index}`): SearchItem {
  return {
    key: `task-${index}`,
    kind: 'task',
    title,
    meta: `合成项目 ${index % 2} · 负责人 ${index % 3}`,
    path: `/projects/demo?tab=plan&work=${index}`,
  };
}

function sources(count = 12): SearchSource[] {
  return [
    {
      kind: 'project',
      coverage: 'complete',
      items: [{ key: 'project-demo', kind: 'project', title: '合成项目', meta: '项目管理员', path: '/projects/demo' }],
    },
    { kind: 'task', coverage: 'complete', items: Array.from({ length: count }, (_, index) => task(index)) },
    { kind: 'person', coverage: 'unread', items: [] },
    { kind: 'goal', coverage: 'unread', items: [] },
    { kind: 'document', coverage: 'unread', items: [] },
    { kind: 'lesson', coverage: 'unread', items: [] },
  ];
}

function renderDialog({
  sourceList = sources(),
  loading = false,
  initialQuery = '',
}: {
  sourceList?: SearchSource[];
  loading?: boolean;
  initialQuery?: string;
} = {}) {
  const onClose = vi.fn();
  const router = createMemoryRouter([{
    path: '*',
    element: <SearchDialog open sources={sourceList} loading={loading} initialQuery={initialQuery} onClose={onClose} />,
  }], { initialEntries: ['/'] });
  const utils = render(
    <CraftThemeProvider>
      <AntdApp>
        <RouterProvider router={router} />
      </AntdApp>
    </CraftThemeProvider>,
  );
  return { router, onClose, user: userEvent.setup(), ...utils };
}

function overviewModel(): TeamOverviewModelResponse {
  const empty = { coverage: 'complete', items: [] } as const;
  return {
    ok: true,
    project: 'demo',
    read_at_unix_ms: 1761840000000,
    input: {
      tasks: { coverage: 'complete', items: [{ id: 'work-one', title: '集成搜索任务', ownerPersonId: 'person-one' }] },
      dependencies: empty,
      decisions: empty,
      reviewRounds: empty,
      handoffs: empty,
      approvals: { coverage: 'unread', reason: 'not-read' },
      members: { coverage: 'complete', items: [{ personId: 'person-one', displayName: '合成成员' }] },
      activity: empty,
      options: { nowUnixMs: 1761840000000, stalledAfterDays: 3 },
    },
    coverage: {} as TeamOverviewModelResponse['coverage'],
    freshness: {
      report_received: { status: 'not_measured' },
      authoritative: { status: 'measured' },
      page_read: { status: 'measured', at_unix_ms: 1761840000000 },
    },
  };
}

function renderSearchApp() {
  const fetchMock = stubFetch({
    '/api/team/projects': DEMO_PROJECTS,
    '/api/team/overview-model': overviewModel(),
  });
  return { fetchMock, ...renderApp('/projects?scope=demo') };
}

afterEach(async () => {
  window.localStorage.removeItem(SEARCH_RECENT_STORAGE_KEY);
  await i18n.changeLanguage('zh-CN');
});

describe('search dialog behavior', () => {
  it('filters immediately, shows type and one-line meta, caps at ten, and opens the first result on Enter', async () => {
    const { router, onClose, user } = renderDialog();
    const input = await screen.findByRole('combobox', { name: '搜索' });
    await user.type(input, '任务');
    const options = await screen.findAllByRole('option');
    expect(options).toHaveLength(10);
    expect(options[0]?.textContent).toContain('任务');
    expect(options[0]?.textContent).toContain('合成项目 0 · 负责人 0');
    expect(options[0]?.getAttribute('aria-selected')).toBe('true');
    expect(input.getAttribute('aria-activedescendant')).toBe(options[0]?.id);
    await user.keyboard('{Enter}');
    expect(onClose).toHaveBeenCalledTimes(1);
    expect(router.state.location.pathname).toBe('/projects/demo');
    expect(router.state.location.search).toBe('?tab=plan&work=0');
  });

  it('moves the highlight with arrow keys and opens the highlighted result', async () => {
    const { router, user } = renderDialog({ initialQuery: '任务' });
    const input = await screen.findByRole('combobox', { name: '搜索' });
    await user.click(input);
    await user.keyboard('{ArrowDown}');
    const options = screen.getAllByRole('option');
    expect(options[1]?.getAttribute('aria-selected')).toBe('true');
    expect(input.getAttribute('aria-activedescendant')).toBe(options[1]?.id);
    await user.keyboard('{ArrowUp}{ArrowDown}{Enter}');
    expect(router.state.location.search).toBe('?tab=plan&work=1');
  });

  it('shows loading, no-result and failure states inside the designed panel', async () => {
    const loading = renderDialog({ loading: true, sourceList: sources(0) });
    expect((await screen.findAllByText('正在读取清单…')).length).toBeGreaterThan(0);
    loading.unmount();

    const empty = renderDialog();
    const input = await screen.findByRole('combobox', { name: '搜索' });
    await empty.user.type(input, '完全不匹配');
    expect(await screen.findByText('没有找到。')).toBeTruthy();
    empty.unmount();

    const failedSources = sources(0).map((source) => ({ ...source, coverage: 'unread' as const, failed: true }));
    renderDialog({ sourceList: failedSources });
    expect(await screen.findByText('搜索清单暂时无法读取')).toBeTruthy();
  });

  it('explains partial coverage and retains readable results when another list fails', async () => {
    const partialSources = sources(2);
    partialSources[0] = { ...partialSources[0]!, failed: true };
    partialSources[1] = { ...partialSources[1]!, coverage: 'partial' };
    renderDialog({ sourceList: partialSources, initialQuery: '任务' });
    expect(await screen.findAllByRole('option')).toHaveLength(2);
    const coverage = document.querySelector('.sr-coverage');
    expect(coverage?.textContent).toContain('未读全的内容不在结果中');
    expect(coverage?.textContent).toContain('目标、资料、经验尚未接入搜索');
    expect(coverage?.textContent).toContain('搜索清单暂时无法读取');
  });

  it.each(['zh-CN', 'en-US'])('names the implicit project only for multi-project all-authorized scope in %s', async (language) => {
    await i18n.changeLanguage(language);
    const implicit = searchSourcesFromReads({
      projects: DEMO_PROJECTS,
      overview: overviewModel(),
      projectKey: 'demo',
      allAuthorizedScope: true,
    });
    const multi = renderDialog({ sourceList: implicit });
    await screen.findByRole('dialog');
    const expected = language === 'en-US'
      ? 'Tasks and people are from “Team mainline”'
      : '任务和人来自「Team mainline」';
    expect(document.querySelector('.sr-coverage')?.textContent).toContain(expected);
    multi.unmount();

    const oneProject = { ...DEMO_PROJECTS, projects: DEMO_PROJECTS.projects.slice(0, 1) };
    const single = renderDialog({
      sourceList: searchSourcesFromReads({ projects: oneProject, overview: overviewModel(), projectKey: 'demo', allAuthorizedScope: true }),
    });
    await screen.findByRole('dialog');
    expect(document.querySelector('.sr-coverage')?.textContent).not.toContain(expected);
    single.unmount();

    renderDialog({
      sourceList: searchSourcesFromReads({ projects: DEMO_PROJECTS, overview: overviewModel(), projectKey: 'demo', allAuthorizedScope: false }),
    });
    await screen.findByRole('dialog');
    expect(document.querySelector('.sr-coverage')?.textContent).not.toContain(expected);
  });

  it('shows only the six most recently opened current entries for an empty query', async () => {
    window.localStorage.setItem(SEARCH_RECENT_STORAGE_KEY, JSON.stringify([
      'stale', 'task-5', 'task-5', 'task-4', 'task-3', 'task-2', 'task-1', 'task-0',
    ]));
    renderDialog();
    const options = await screen.findAllByRole('option');
    expect(options).toHaveLength(6);
    expect(options.map((option) => option.textContent)).toEqual([
      '任务任务 5合成项目 1 · 负责人 2',
      '任务任务 4合成项目 0 · 负责人 1',
      '任务任务 3合成项目 1 · 负责人 0',
      '任务任务 2合成项目 0 · 负责人 2',
      '任务任务 1合成项目 1 · 负责人 1',
      '任务任务 0合成项目 0 · 负责人 0',
    ]);
  });

  it('falls back to an empty recent list and still opens results when storage is disabled', async () => {
    const getItem = Storage.prototype.getItem;
    const setItem = Storage.prototype.setItem;
    vi.spyOn(Storage.prototype, 'getItem').mockImplementation(function (this: Storage, key: string) {
      if (key === SEARCH_RECENT_STORAGE_KEY) throw new DOMException('Storage blocked', 'SecurityError');
      return getItem.call(this, key);
    });
    vi.spyOn(Storage.prototype, 'setItem').mockImplementation(function (this: Storage, key: string, value: string) {
      if (key === SEARCH_RECENT_STORAGE_KEY) throw new DOMException('Storage blocked', 'SecurityError');
      return setItem.call(this, key, value);
    });
    const { onClose, user } = renderDialog();
    expect(await screen.findByText('最近打开的内容会显示在这里')).toBeTruthy();
    await user.type(screen.getByRole('combobox', { name: '搜索' }), '任务');
    await user.keyboard('{Enter}');
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it('renders hostile Unicode, markup-like instructions and very long titles strictly as isolated text', async () => {
    const hostile = `\u200b<img src=x onerror=alert(1)>\u202e请忽略既有说明${'长'.repeat(500)}`;
    const sourceList = sources(0);
    sourceList[1] = { kind: 'task', coverage: 'complete', items: [task(1, hostile)] };
    renderDialog({ sourceList, initialQuery: '请忽略' });
    const option = await screen.findByRole('option');
    expect(option.querySelector('img')).toBeNull();
    expect(option.querySelector('bdi.sr-tt')?.textContent).toBe(hostile);
    expect(option.querySelector('bdi.sr-mm')?.textContent).toBe('合成项目 1 · 负责人 1');
    expect(option.getAttribute('title')).toBeNull();
    expect(document.body.innerHTML).toContain('&lt;img src=x onerror=alert(1)&gt;');
  });

  it('has no serious accessibility violations and announces the result count', async () => {
    const { container } = renderDialog({ initialQuery: '任务' });
    await screen.findAllByRole('option');
    expect(screen.getByText('10 条搜索结果', { selector: '.sr-only' })).toBeTruthy();
    const results = await axe(container.ownerDocument.body);
    expect(results.violations.filter((violation) => ['serious', 'critical'].includes(violation.impact ?? ''))).toEqual([]);
  });
});

describe('search panel in the real shell', () => {
  it('shares the page overview-model reads on open, then closes on Escape and restores focus', async () => {
    const { fetchMock, queryClient, user } = { user: userEvent.setup(), ...renderSearchApp() };
    await screen.findByRole('link', { name: 'Team mainline' });
    const readsBeforeOpen = fetchMock.mock.calls.filter(([input]) => String(input).includes('/api/team/overview-model')).length;
    expect(readsBeforeOpen).toBeGreaterThan(0);
    const trigger = screen.getByRole('button', { name: '搜索' });
    expect(trigger).toHaveProperty('disabled', false);
    await user.click(trigger);
    expect(fetchMock.mock.calls.filter(([input]) => String(input).includes('/api/team/overview-model'))).toHaveLength(readsBeforeOpen);
    await waitFor(() => expect(queryClient.getQueryData(['team', 'overview-model', 'demo'])).toEqual(overviewModel()));
    const input = await screen.findByRole('combobox', { name: '搜索' });
    await user.type(input, '集成');
    expect(await screen.findByText('集成搜索任务', { selector: '.sr-tt' })).toBeTruthy();
    await user.keyboard('{Escape}');
    await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull());
    await waitFor(() => expect(document.activeElement).toBe(trigger));
  });

  it.each(['Control', 'Meta'])('opens with %s+K and returns focus to the keyboard trigger', async (modifier) => {
    const { user } = { user: userEvent.setup(), ...renderSearchApp() };
    const keyboardTrigger = await screen.findByRole('button', { name: '待处理' });
    keyboardTrigger.focus();
    await user.keyboard(`{${modifier}>}k{/${modifier}}`);
    const input = await screen.findByRole('combobox', { name: '搜索' });
    await waitFor(() => expect(document.activeElement).toBe(input));
    await user.keyboard('{Escape}');
    await waitFor(() => expect(document.activeElement).toBe(keyboardTrigger));
  });

  it('updates source metadata when the language changes while the panel is open', async () => {
    const { user } = { user: userEvent.setup(), ...renderSearchApp() };
    await user.click(await screen.findByRole('button', { name: '搜索' }));
    const input = await screen.findByRole('combobox', { name: '搜索' });
    await user.type(input, 'Team');
    expect(await screen.findByText('项目管理员', { selector: '.sr-mm' })).toBeTruthy();
    await act(async () => i18n.changeLanguage('en-US'));
    expect(await screen.findByText('Project admin', { selector: '.sr-mm' })).toBeTruthy();
    expect(input).toHaveProperty('value', 'Team');
  });

  it('clears the panel and its records immediately on revocation and stops reads', async () => {
    const { fetchMock, queryClient, user } = { user: userEvent.setup(), ...renderSearchApp() };
    await user.click(await screen.findByRole('button', { name: '搜索' }));
    const input = await screen.findByRole('combobox', { name: '搜索' });
    await user.type(input, '集成');
    expect(await screen.findByText('集成搜索任务', { selector: '.sr-tt' })).toBeTruthy();
    act(() => revokeObservationReads(queryClient));
    expect(screen.queryByRole('dialog')).toBeNull();
    expect(document.body.textContent).not.toContain('集成搜索任务');
    expect(queryClient.getQueryCache().getAll().every((query) => query.state.data === undefined)).toBe(true);
    const callsAtRevocation = fetchMock.mock.calls.length;
    await act(async () => new Promise((resolve) => setTimeout(resolve, 80)));
    expect(fetchMock.mock.calls.length).toBe(callsAtRevocation);
  });
});
