import { act, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';
import i18n from './i18n';
import { formatUnixMs } from './lib/format';
import { queryKeys } from './lib/queries';
import {
  DEMO_PROJECTS,
  DEMO_WORK_INBOX,
  DEMO_WORK_ITEMS,
  DEMO_WORK_NEXT,
  demoWorkSnapshot,
  renderApp,
} from './test-utils';

function json(payload: unknown) {
  return new Response(JSON.stringify(payload), {
    status: 200,
    headers: { 'content-type': 'application/json' },
  });
}

function makeItems(count: number) {
  return Array.from({ length: count }, (_, index) => ({
    ...DEMO_WORK_ITEMS[0],
    work_id: `W${index + 1}`,
    workstream_id: `WS${index + 1}`,
    title: `Observed task ${index + 1}`,
    next_query: {
      ...DEMO_WORK_ITEMS[0].next_query,
      work_id: `W${index + 1}`,
      workstream_id: `WS${index + 1}`,
    },
  }));
}

function makeNext(items: unknown[], resume: unknown[] = []) {
  return {
    ...DEMO_WORK_NEXT,
    result: {
      ...DEMO_WORK_NEXT.result,
      data: { ...DEMO_WORK_NEXT.result.data, items, resume },
    },
  };
}

function setupWorkbench({
  path = '/workbench?scope=demo',
  next = DEMO_WORK_NEXT,
  inbox = DEMO_WORK_INBOX,
  snapshot = demoWorkSnapshot,
}: {
  path?: string;
  next?: unknown;
  inbox?: unknown;
  snapshot?: (workId: string) => unknown;
} = {}) {
  const fetchMock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
    const url = String(input);
    if (url.includes('/api/team/projects')) return json(DEMO_PROJECTS);
    if (url.includes('/api/team/read')) {
      const request = JSON.parse(String(init?.body)) as {
        query: { op: string; work_id?: string };
      };
      if (request.query.op === 'work.next') return json(next);
      if (request.query.op === 'work.inbox') return json(inbox);
      if (request.query.op === 'work.snapshot') {
        return json(snapshot(request.query.work_id ?? ''));
      }
    }
    return json({ ok: false, error: { code: 'NoRoute', message: url } });
  });
  vi.stubGlobal('fetch', fetchMock);
  return { fetchMock, ...renderApp(path) };
}

function snapshotCalls(fetchMock: ReturnType<typeof vi.fn>) {
  return fetchMock.mock.calls.filter(([, init]) => {
    const body = String(init?.body ?? '');
    return body.includes('"work.snapshot"');
  });
}

function readOperations(fetchMock: ReturnType<typeof vi.fn>) {
  return fetchMock.mock.calls
    .filter(([input]) => String(input).includes('/api/team/read'))
    .map(([, init]) => (JSON.parse(String(init?.body)) as { query: { op: string } }).query.op);
}

afterEach(async () => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
  await i18n.changeLanguage('zh-CN');
});

describe('My work sourced rows and navigation', () => {
  it('renders real identity, title, claimed/unclaimed state, navigation, and advisory next action', async () => {
    setupWorkbench();

    expect(await screen.findAllByText('Build the filter API')).toBeTruthy();
    expect(screen.getByRole('heading', { name: '我的工作' })).toBeTruthy();
    expect(screen.getByTestId('workbench-role').textContent).toBe('developer');
    expect(screen.getAllByText('已认领')).not.toHaveLength(0);
    expect(screen.getByText('未认领')).toBeTruthy();
    expect(screen.queryByText('未知')).toBeNull();
    expect(screen.getAllByText('继续工作')).not.toHaveLength(0);
    expect(screen.getByText('准备')).toBeTruthy();
    expect(screen.getByText('查看任务（仅观测）')).toBeTruthy();
    expect(screen.getByText('准备任务（由 Agent 执行）')).toBeTruthy();
    expect(screen.queryByRole('button', { name: /认领|claim/i })).toBeNull();
  });

  it('switches sourced and later-slice tabs through the URL', async () => {
    const user = userEvent.setup();
    const { router } = setupWorkbench();
    await screen.findAllByText('Build the filter API');
    const selectedRow = screen.getByRole('link', { name: 'Build the filter API' }).closest('tr');
    expect(selectedRow).not.toBeNull();
    await user.click(selectedRow!);
    expect(router.state.location.search).toContain('work=W1');

    await user.click(screen.getByRole('tab', { name: '需要我处理' }));
    expect(router.state.location.search).toContain('tab=needs-me');
    expect(await screen.findAllByText('Review the API handoff')).toBeTruthy();

    for (const [name, key] of [['可参与', 'participate'], ['最近交付', 'delivered']] as const) {
      await user.click(screen.getByRole('tab', { name }));
      expect(router.state.location.search).toContain(`tab=${key}`);
      expect(await screen.findByTestId('workbench-tab-placeholder')).toBeTruthy();
      expect(screen.getByTestId('workbench-tab-placeholder').textContent).toContain('后续切片');
    }
  });

  it('encodes same-task links for project and work IDs', async () => {
    const special = [{
      ...DEMO_WORK_ITEMS[0],
      work_id: 'work /?&=中文',
      title: 'Encoded task',
      next_query: { ...DEMO_WORK_ITEMS[0].next_query, work_id: 'work /?&=中文' },
    }];
    setupWorkbench({
      path: '/workbench?scope=demo%20%2F%3F%26%3D',
      next: makeNext(special),
    });

    const link = await screen.findByRole('link', { name: 'Encoded task' });
    expect(link.getAttribute('href')).toBe(
      '/projects/demo%20%2F%3F%26%3D?tab=plan&work=work%20%2F%3F%26%3D%E4%B8%AD%E6%96%87',
    );
  });

  it('renders real resume entries without turning them into claim controls', async () => {
    setupWorkbench({
      next: makeNext([], [{
        work_id: 'W-resume',
        session_id: 'session-1',
        session_version: '3',
        next_query: { op: 'work.observe', protocol_version: 1, work_id: 'W-resume', workstream_id: 'WS1' },
      }]),
    });

    const resume = await screen.findByTestId('workbench-resume');
    expect(within(resume).getByRole('link', { name: 'W-resume' }).getAttribute('href'))
      .toBe('/projects/demo?tab=plan&work=W-resume');
    expect(resume.textContent).toContain('session-1');
    expect(screen.queryByTestId('state-empty')).toBeNull();
  });
});

describe('My work URL, source selection, and keyboard behavior', () => {
  it('falls back to in progress for an unknown tab', async () => {
    setupWorkbench({ path: '/workbench?scope=demo&tab=bogus' });
    await screen.findAllByText('Build the filter API');
    expect(screen.getByRole('tab', { name: '进行中' }).getAttribute('aria-selected')).toBe('true');
    expect(screen.queryByTestId('workbench-tab-placeholder')).toBeNull();
  });

  it('requests exactly the sources used by each sourced tab', async () => {
    const user = userEvent.setup();
    const { fetchMock } = setupWorkbench();
    await screen.findAllByText('Build the filter API');
    await waitFor(() => expect(snapshotCalls(fetchMock)).toHaveLength(2));
    expect([...new Set(readOperations(fetchMock))].sort()).toEqual(['work.next', 'work.snapshot']);
    expect(readOperations(fetchMock)).not.toContain('work.inbox');

    await user.click(screen.getByRole('tab', { name: '需要我处理' }));
    await screen.findAllByText('Review the API handoff');
    await waitFor(() => expect(snapshotCalls(fetchMock)).toHaveLength(3));
    expect([...new Set(readOperations(fetchMock))].sort()).toEqual([
      'work.inbox',
      'work.next',
      'work.snapshot',
    ]);
  });

  it('selects rows with Enter and Space', async () => {
    const user = userEvent.setup();
    const { router } = setupWorkbench();
    await screen.findAllByText('Build the filter API');

    const firstRow = screen.getByRole('link', { name: 'Build the filter API' }).closest('tr');
    expect(firstRow).not.toBeNull();
    firstRow!.focus();
    expect(document.activeElement).toBe(firstRow);
    await user.keyboard('{Enter}');
    expect(router.state.location.search).toContain('work=W1');

    const secondRow = screen.getByRole('link', { name: 'Prepare the support page' }).closest('tr');
    expect(secondRow).not.toBeNull();
    secondRow!.focus();
    await user.keyboard(' ');
    expect(router.state.location.search).toContain('work=W2');
  });

  it('shows unclaimed in the selected detail pane', async () => {
    setupWorkbench({ path: '/workbench?scope=demo&work=W2' });
    const detail = await screen.findByTestId('workbench-detail');
    await waitFor(() => expect(detail.textContent).toContain('Keep support filters useful'));
    expect(within(detail).getByText('未认领')).toBeTruthy();
  });

  it('restores page two from the URL', async () => {
    setupWorkbench({
      path: '/workbench?scope=demo&page=2',
      next: makeNext(makeItems(61)),
    });
    await screen.findAllByText('Observed task 61');
    expect(screen.queryByText('Observed task 1')).toBeNull();
  });

  it('clamps an out-of-range URL page to the last available page', async () => {
    setupWorkbench({ path: '/workbench?scope=demo&page=5' });
    expect(await screen.findByTestId('workbench-table')).toBeTruthy();
    expect(screen.getAllByText('Build the filter API')).not.toHaveLength(0);
    expect(screen.getByText('Prepare the support page')).toBeTruthy();
    expect(document.querySelector('.ant-pagination')).not.toBeNull();
    expect(document.querySelector('.ant-pagination-item-active')?.textContent).toBe('1');
  });

  it('clamps again when the row count shrinks below the current page', async () => {
    const { queryClient } = setupWorkbench({
      path: '/workbench?scope=demo&page=2',
      next: makeNext(makeItems(61)),
    });
    await screen.findAllByText('Observed task 61');

    act(() => {
      queryClient.setQueryData(queryKeys.workNext('demo'), makeNext(makeItems(60)));
    });

    await screen.findAllByText('Observed task 1');
    expect(screen.queryByText('Observed task 61')).toBeNull();
    expect(document.querySelector('.ant-pagination')).not.toBeNull();
    expect(document.querySelector('.ant-pagination-item-active')?.textContent).toBe('1');
  });

  it.each(['bogus', '0', '-1', '1.5', '2e1'])('falls back to page one for page=%s', async (page) => {
    setupWorkbench({
      path: `/workbench?scope=demo&page=${page}`,
      next: makeNext(makeItems(61)),
    });
    await screen.findAllByText('Observed task 1');
    expect(screen.queryByText('Observed task 61')).toBeNull();
  });

  it('drops both page and selected work when switching tabs', async () => {
    const user = userEvent.setup();
    const { router } = setupWorkbench({
      path: '/workbench?scope=demo&tab=in-progress&page=2&work=W61',
      next: makeNext(makeItems(61)),
    });
    await screen.findAllByText('Observed task 61');
    await user.click(screen.getByRole('tab', { name: '需要我处理' }));
    const params = new URLSearchParams(router.state.location.search);
    expect(params.get('page')).toBeNull();
    expect(params.get('work')).toBeNull();
  });

  it('drops selected work when changing page and omits page one from the URL', async () => {
    const user = userEvent.setup();
    const { router } = setupWorkbench({
      path: '/workbench?scope=demo&work=W1',
      next: makeNext(makeItems(61)),
    });
    await screen.findAllByText('Observed task 1');
    await user.click(screen.getByRole('listitem', { name: '下一页' }));
    await screen.findAllByText('Observed task 61');
    let params = new URLSearchParams(router.state.location.search);
    expect(params.get('work')).toBeNull();
    expect(params.get('page')).toBe('2');

    await user.click(screen.getByRole('listitem', { name: '上一页' }));
    await screen.findAllByText('Observed task 1');
    params = new URLSearchParams(router.state.location.search);
    expect(params.get('page')).toBeNull();
  });
});

describe('visible-row snapshot hydration', () => {
  it('hydrates at most the current 60-row page and then the next visible page', async () => {
    const user = userEvent.setup();
    const { fetchMock, router } = setupWorkbench({ next: makeNext(makeItems(61)) });
    await screen.findAllByText('Observed task 1');

    await waitFor(() => expect(snapshotCalls(fetchMock)).toHaveLength(60));
    expect(snapshotCalls(fetchMock).some(([, init]) => String(init?.body).includes('W61'))).toBe(false);

    await user.click(screen.getByRole('listitem', { name: '下一页' }));
    expect(router.state.location.search).toContain('page=2');
    await screen.findAllByText('Observed task 61');
    await waitFor(() => expect(snapshotCalls(fetchMock)).toHaveLength(61));
    expect(snapshotCalls(fetchMock).filter(([, init]) => String(init?.body).includes('W61'))).toHaveLength(1);
  }, 20_000);

  it('keeps at most four snapshot requests in flight', async () => {
    let active = 0;
    let maximumActive = 0;
    const releases: Array<() => void> = [];
    const fetchMock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = String(input);
      if (url.includes('/api/team/projects')) return json(DEMO_PROJECTS);
      const request = JSON.parse(String(init?.body)) as { query: { op: string; work_id?: string } };
      if (request.query.op === 'work.next') return json(makeNext(makeItems(8)));
      if (request.query.op === 'work.snapshot') {
        active += 1;
        maximumActive = Math.max(maximumActive, active);
        return new Promise<Response>((resolve) => {
          releases.push(() => {
            active -= 1;
            resolve(json(demoWorkSnapshot(request.query.work_id)));
          });
        });
      }
      return json({ ok: false, error: { code: 'NoRoute' } });
    });
    vi.stubGlobal('fetch', fetchMock);
    renderApp('/workbench?scope=demo');
    await screen.findAllByText('Observed task 1');

    await waitFor(() => expect(snapshotCalls(fetchMock)).toHaveLength(4));
    expect(active).toBe(4);
    expect(maximumActive).toBe(4);

    releases.splice(0).forEach((release) => release());
    await waitFor(() => expect(snapshotCalls(fetchMock)).toHaveLength(8));
    releases.splice(0).forEach((release) => release());
    await waitFor(() => expect(snapshotCalls(fetchMock).length).toBe(8));
    expect(maximumActive).toBeLessThanOrEqual(4);
  });

  it('uses not measured for a missing client report or failed snapshot, never last activity', async () => {
    const listRead = formatUnixMs(1761840000000, 'zh-CN');
    const observedAt = formatUnixMs(1761839999000, 'zh-CN');
    const lastActivity = formatUnixMs(1761839900000, 'zh-CN');
    const { fetchMock } = setupWorkbench({
      snapshot: (workId) => {
        if (workId === 'W2') return { ok: false, error: { code: 'ReadTimeout', message: 'slow' } };
        const response = demoWorkSnapshot(workId) as {
          result: { data: { observation: { reporting: { client_reported_at_unix_ms?: number } } } };
        };
        delete response.result.data.observation.reporting.client_reported_at_unix_ms;
        return response;
      },
    });
    await screen.findAllByText('Build the filter API');

    await waitFor(() => expect(snapshotCalls(fetchMock)).toHaveLength(2));
    const first = await screen.findByTestId('last-report-W1');
    const second = await screen.findByTestId('last-report-W2');
    await waitFor(() => {
      expect(first.textContent).toBe('未测量');
      expect(second.textContent).toBe('未测量');
    });
    expect(first.textContent).not.toContain(listRead);
    expect(first.textContent).not.toContain(observedAt);
    expect(first.textContent).not.toContain(lastActivity);
    expect(screen.getByText('Prepare the support page')).toBeTruthy();
    expect(screen.queryByTestId('state-partial')).toBeNull();
  });

  it('formats the client report time, not last activity or either other clock', async () => {
    setupWorkbench();
    const cell = await screen.findByTestId('last-report-W1');
    await waitFor(() => expect(cell.textContent).toBe(formatUnixMs(1761839800000, 'zh-CN')));
    expect(cell.textContent).not.toBe(formatUnixMs(1761839900000, 'zh-CN'));
    expect(cell.textContent).not.toBe(formatUnixMs(1761840000000, 'zh-CN'));
    expect(cell.textContent).not.toBe(formatUnixMs(1761839999000, 'zh-CN'));
  });

  it('shows partial failure when every visible snapshot read fails', async () => {
    setupWorkbench({
      snapshot: () => ({ ok: false, error: { code: 'ReadTimeout', message: 'slow' } }),
    });
    await screen.findAllByText('Build the filter API');
    expect(await screen.findByTestId('state-partial')).toBeTruthy();
    expect(screen.getByTestId('state-partial').textContent).toContain('ReadTimeout');
    expect(screen.getByText('Prepare the support page')).toBeTruthy();
  });
});

describe('My work states, detail, and localization', () => {
  it('uses empty only after a successful work.next with no items and no resume entries', async () => {
    setupWorkbench({ next: makeNext([]) });
    expect(await screen.findByTestId('state-empty')).toBeTruthy();
  });

  it('does not turn a failed list read into an empty result', async () => {
    setupWorkbench({ next: { ok: false, error: { code: 'ReadTimeout', message: 'slow' } } });
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 1200));
    });
    expect(screen.getByTestId('state-partial')).toBeTruthy();
    expect(screen.queryByTestId('state-empty')).toBeNull();
  });

  it('uses empty for a successful empty work.inbox read', async () => {
    const emptyInbox = {
      ...DEMO_WORK_INBOX,
      result: {
        ...DEMO_WORK_INBOX.result,
        data: { ...DEMO_WORK_INBOX.result.data, items: [] },
      },
    };
    setupWorkbench({ path: '/workbench?scope=demo&tab=needs-me', inbox: emptyInbox });
    expect(await screen.findByTestId('state-empty')).toBeTruthy();
  });

  it('shows real contract, dependency and guidance fields plus honest absent progress', async () => {
    setupWorkbench({ path: '/workbench?scope=demo&work=W1' });
    const detail = await screen.findByTestId('workbench-detail');
    await waitFor(() => expect(detail.textContent).toContain('Keep support filters useful'));
    expect(detail.textContent).toContain('Do not mutate input records');
    expect(detail.textContent).toContain('upstream-contract-v1');
    expect(detail.textContent).toContain('Record the verified outcome.');
    expect(within(detail).getByTestId('state-not-reported')).toBeTruthy();
    expect(within(detail).getByRole('link', { name: '打开完整任务 →' }).getAttribute('href'))
      .toBe('/projects/demo?tab=plan&work=W1');
  });

  it('does not invent detail sections when snapshot fields are absent', async () => {
    setupWorkbench({
      path: '/workbench?scope=demo&work=W1',
      snapshot: (workId) => ({
        ...demoWorkSnapshot(workId),
        result: {
          ...demoWorkSnapshot(workId).result,
          data: {
            work_id: workId,
            observation: { last_activity_at_unix_ms: null, progress: null },
          },
        },
      }),
    });
    const detail = await screen.findByTestId('workbench-detail');
    await waitFor(() => expect(detail.querySelectorAll('[data-testid="detail-absent"]').length).toBeGreaterThanOrEqual(3));
    expect(detail.textContent).not.toContain('Keep support filters useful');
    expect(detail.textContent).not.toContain('Record the verified outcome.');
  });

  it('renders the new workbench strings in English after a language switch', async () => {
    setupWorkbench();
    await screen.findAllByText('Build the filter API');
    await act(() => i18n.changeLanguage('en-US'));

    expect(await screen.findByRole('heading', { name: 'My work' })).toBeTruthy();
    expect(screen.getByRole('tab', { name: 'In progress' })).toBeTruthy();
    expect(screen.getByRole('tab', { name: 'Needs me' })).toBeTruthy();
    expect(screen.getByText('Unclaimed')).toBeTruthy();
    expect(screen.getByText('Open full task →')).toBeTruthy();
  });
});
