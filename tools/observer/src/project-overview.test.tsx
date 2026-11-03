import { act, cleanup, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { axe, toHaveNoViolations } from 'jest-axe';
import { afterEach, describe, expect, it, vi } from 'vitest';
import i18n from './i18n';
import { formatUnixMs } from './lib/format';
import { defaultVisibleText, findForbiddenVisibleText } from './lib/human-language-guard';
import { queryKeys, revokeObservationReads } from './lib/queries';
import type { TeamWork } from './lib/types';
import {
  DEMO_ACTIVITY,
  DEMO_PROJECT_OVERVIEW,
  DEMO_PROJECT_WORKS,
  DEMO_PROJECTS,
  demoWorkSnapshot,
  renderApp,
} from './test-utils';

expect.extend(toHaveNoViolations);

function json(payload: unknown) {
  return new Response(JSON.stringify(payload), {
    status: 200,
    headers: { 'content-type': 'application/json' },
  });
}

function makeProjectWorks(count: number) {
  return Array.from({ length: count }, (_, index) => ({
    ...DEMO_PROJECT_WORKS[0],
    key: `W${index + 1}`,
    title: `Observed project task ${index + 1}`,
    workstream_id: 'WS1',
    contract_hash: `contract-${index + 1}`,
  }));
}

function makeTwoStreamWorks(count: number) {
  return Array.from({ length: count }, (_, index) => ({
    ...DEMO_PROJECT_WORKS[0],
    key: `W${index + 1}`,
    title: `Observed project task ${index + 1}`,
    workstream_id: index % 2 === 0 ? 'WS1' : 'WS2',
    contract_hash: `contract-${index + 1}`,
  }));
}

function makeProjectOverview(works: TeamWork[] = DEMO_PROJECT_WORKS) {
  return {
    ...DEMO_PROJECT_OVERVIEW,
    works,
    coverage: {
      ...DEMO_PROJECT_OVERVIEW.coverage,
      works_enumerated: works.length,
      scope: { ...DEMO_PROJECT_OVERVIEW.coverage.scope, enumerated: works.length },
    },
  };
}

function makeActivity(items: unknown[], overrides: Record<string, unknown> = {}) {
  return {
    ...DEMO_ACTIVITY,
    data: { ...DEMO_ACTIVITY.data, ...overrides, items },
  };
}

function setupOverview({
  path = '/projects/demo?tab=overview',
  overview = DEMO_PROJECT_OVERVIEW,
  activity = DEMO_ACTIVITY,
  snapshot = demoWorkSnapshot,
  snapshotGate,
}: {
  path?: string;
  overview?: unknown;
  activity?: unknown;
  snapshot?: (workId: string) => unknown;
  /** Holds every work snapshot read back until it resolves. */
  snapshotGate?: Promise<void>;
} = {}) {
  const fetchMock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
    const url = String(input);
    if (url.includes('/api/team/projects')) return json(DEMO_PROJECTS);
    if (url.includes('/api/team/overview')) return json(overview);
    if (url.includes('/api/team/activity')) return json(activity);
    if (url.includes('/api/team/read')) {
      const request = JSON.parse(String(init?.body)) as { query: { op: string; work_id?: string } };
      if (request.query.op === 'work.snapshot') {
        await snapshotGate;
        return json(snapshot(request.query.work_id ?? ''));
      }
    }
    return json({ ok: false, error: { code: 'NoRoute', message: url } });
  });
  vi.stubGlobal('fetch', fetchMock);
  return { fetchMock, ...renderApp(path) };
}

function snapshotCalls(fetchMock: ReturnType<typeof vi.fn>) {
  return fetchMock.mock.calls.filter(([, init]) => String(init?.body ?? '').includes('"work.snapshot"'));
}

afterEach(async () => {
  vi.unstubAllGlobals();
  await i18n.changeLanguage('zh-CN');
});

describe('project workstream progress', () => {
  it('renders real groups and fields, honest unknowns, the work\'s own state, a plain missing next step and the report time', async () => {
    setupOverview();
    expect(await screen.findByRole('link', { name: 'Build the filter API' })).toBeTruthy();
    expect(screen.getByTestId('workstream-group-WS1').textContent).toContain('Backend workstream');
    expect(screen.getByTestId('workstream-group-WS2').textContent).toContain('Frontend workstream');
    expect(screen.getByTestId('project-status-W1').textContent).toBe('已认领');
    // The row carries no owner; the work's own read says 'lin' is responsible.
    expect(screen.getByTestId('project-owner-W1').textContent).toBe('lin');
    expect(screen.getByTestId('project-owner-W1').getAttribute('data-owner-source')).toBe('snapshot');
    expect(screen.getByTestId('project-next-W1').textContent).toBe('未报告下一步');
    expect(screen.getByTestId('last-report-W1').textContent).toBe(formatUnixMs(1761839800000, 'zh-CN'));
    expect(screen.getByTestId('last-report-W1').textContent).not.toBe(formatUnixMs(1761839900000, 'zh-CN'));
    expect(screen.getByTestId('last-report-W1').style.whiteSpace).toBe('nowrap');
    const timeColumn = screen.getByRole('columnheader', { name: '最近报告' }).closest('table')?.querySelector('colgroup')?.lastElementChild as HTMLElement | undefined;
    expect(timeColumn?.style.width || timeColumn?.getAttribute('width')).toBe('220px');
  });

  it('uses the 60-second list cadence for every visible work snapshot', async () => {
    const { fetchMock, queryClient } = setupOverview();
    await waitFor(() => expect(snapshotCalls(fetchMock)).toHaveLength(2));
    const snapshots = queryClient.getQueryCache().getAll().filter((query) => (
      query.queryKey[0] === 'team' && query.queryKey[1] === 'work.snapshot'
    ));
    expect(snapshots).toHaveLength(2);
    expect(snapshots.map((query) => (
      'refetchInterval' in query.options ? query.options.refetchInterval : undefined
    ))).toEqual([60000, 60000]);
  });

  it.each([59, 60])('renders every work on page one across two workstreams: %i', async (count) => {
    setupOverview({ overview: makeProjectOverview(makeTwoStreamWorks(count)) });
    await waitFor(() => expect(screen.getAllByRole('link', { name: /Observed project task/ })).toHaveLength(count));
    expect(screen.getByRole('link', { name: `Observed project task ${count}` })).toBeTruthy();
  }, 20_000);

  it('renders the correct sixty works on page two across two workstreams', async () => {
    setupOverview({
      path: '/projects/demo?tab=overview&page=2',
      overview: makeProjectOverview(makeTwoStreamWorks(140)),
    });
    await waitFor(() => expect(screen.getAllByRole('link', { name: /Observed project task/ })).toHaveLength(60));
    expect(screen.getByRole('link', { name: 'Observed project task 121' })).toBeTruthy();
    expect(screen.getByRole('link', { name: 'Observed project task 139' })).toBeTruthy();
    expect(screen.getByRole('link', { name: 'Observed project task 2' })).toBeTruthy();
    expect(screen.getByRole('link', { name: 'Observed project task 100' })).toBeTruthy();
    expect(screen.queryByRole('link', { name: 'Observed project task 120' })).toBeNull();
    expect(screen.queryByRole('link', { name: 'Observed project task 102' })).toBeNull();
  }, 20_000);

  it('never puts the service\'s agent-addressed operation or its long note into the row, not even on hover', async () => {
    const longNote = 'Batch progress with next_action/open_loops. Stop and report workspace execution before repository push or publication. '.repeat(4);
    setupOverview({
      snapshot: (workId) => {
        const response = demoWorkSnapshot(workId);
        response.result.data.guidance.action.note = longNote;
        return response;
      },
    });
    const cell = await screen.findByTestId('project-next-W1');
    await waitFor(() => expect(cell.textContent).toBe('未报告下一步'));
    expect(cell.textContent).not.toContain('session.checkpoint');
    expect(cell.textContent).not.toContain('Batch progress');
    // The row used to carry the note in an always-on tooltip; a person gets no engineering text there now.
    const user = userEvent.setup();
    await user.hover(cell);
    await act(async () => { await new Promise((resolve) => setTimeout(resolve, 400)); });
    expect(document.querySelector('.ant-tooltip')).toBeNull();
    expect(screen.queryByText(/Batch progress with next_action/)).toBeNull();
    expect(findForbiddenVisibleText(defaultVisibleText(screen.getByTestId('project-progress-table')))).toEqual([]);
  });

  it('cuts the long ids of a reporter\'s sentence in the row and keeps the sentence as written in its title', async () => {
    setupOverview({
      snapshot: (workId) => {
        const response = demoWorkSnapshot(workId) as { result: { data: { observation: { progress: Record<string, unknown> | null } } } };
        response.result.data.observation.progress = { next_action: 'Wait for independent review of round 01M6AVQ870N7MBRJ5MPCN79T55.' };
        return response;
      },
    });
    await waitFor(() => expect(screen.getByTestId('project-next-W1').textContent).toBe('Wait for independent review of round 01M6AVQ8….'));
    expect(screen.getByTestId('project-next-W1').getAttribute('title')).toBe('Wait for independent review of round 01M6AVQ870N7MBRJ5MPCN79T55.');
    expect(findForbiddenVisibleText(defaultVisibleText(screen.getByTestId('project-progress-table')))).toEqual([]);
  });

  it('shows the next action the work\'s reporter wrote in the checkpoint, the overview row as the fallback, and never the service\'s own hint', async () => {
    const works = [
      { ...DEMO_PROJECT_WORKS[0], next_step: 'Row text for the first work' },
      { ...DEMO_PROJECT_WORKS[1], next_step: 'Row text for the second work' },
      { ...DEMO_PROJECT_WORKS[1], key: 'W3', title: 'Third work', next_step: null },
    ];
    setupOverview({
      overview: makeProjectOverview(works),
      snapshot: (workId) => {
        const response = demoWorkSnapshot(workId) as {
          result: { data: { next_step: string | null; observation: { progress: Record<string, unknown> | null } } };
        };
        response.result.data.observation.progress = workId === 'W1' ? { next_action: 'Wait for the independent review.' } : null;
        response.result.data.next_step = 'Call work.prepare to restore the context.';
        return response;
      },
    });
    await waitFor(() => {
      expect(screen.getByTestId('project-next-W1').textContent).toBe('Wait for the independent review.');
      expect(screen.getByTestId('project-next-W2').textContent).toBe('Row text for the second work');
      expect(screen.getByTestId('project-next-W3').textContent).toBe('未报告下一步');
    });
    expect(screen.getByTestId('project-progress-table').textContent).not.toContain('work.prepare');
    // A long sentence is clamped in the row; the whole of it stays in the title and the work's own page.
    expect(screen.getByTestId('project-next-W1').getAttribute('title')).toBe('Wait for the independent review.');
    expect(screen.getByTestId('project-next-W3').hasAttribute('title')).toBe(false);
  });

  it('says loading, not not-read-yet, while a work\'s own read is still pending', async () => {
    let release: () => void = () => undefined;
    setupOverview({ snapshotGate: new Promise<void>((resolve) => { release = resolve; }) });
    await waitFor(() => {
      expect(screen.getByTestId('project-next-W1').textContent).toBe('加载中…');
      expect(screen.getByTestId('last-report-W1').textContent).toBe('加载中…');
      expect(screen.getByTestId('project-status-W1').textContent).toBe('加载中…');
    });
    await act(async () => { release(); });
    await waitFor(() => expect(screen.getByTestId('last-report-W1').textContent).toBe(formatUnixMs(1761839800000, 'zh-CN')));
    expect(screen.getByTestId('project-next-W1').textContent).toBe('未报告下一步');
  });

  it('puts no stale note on a finished work, whatever the clock says, and keeps the note on an open one', async () => {
    setupOverview({
      snapshot: (workId) => {
        const response = demoWorkSnapshot(workId) as {
          result: { data: { runtime: { state: string }; observation: { reporting: Record<string, unknown> } } };
        };
        response.result.data.observation.reporting.client_stale = true;
        if (workId === 'W1') response.result.data.runtime.state = 'completed';
        return response;
      },
    });
    await waitFor(() => {
      expect(screen.getByTestId('last-report-W1').textContent).toBe(formatUnixMs(1761839800000, 'zh-CN'));
      expect(screen.getByTestId('last-report-W2').textContent).toBe(`${formatUnixMs(1761839800000, 'zh-CN')}可能已过期`);
    });
  });

  it('says how far each work has come: the reporter\'s phase, else the runtime state, else the row\'s status, else unknown', async () => {
    const works = [
      DEMO_PROJECT_WORKS[0],
      DEMO_PROJECT_WORKS[1],
      { ...DEMO_PROJECT_WORKS[1], key: 'W3', title: 'Third work', status: 'open' },
      { ...DEMO_PROJECT_WORKS[1], key: 'W4', title: 'Fourth work', status: null },
    ];
    setupOverview({
      overview: makeProjectOverview(works),
      snapshot: (workId) => {
        if (workId === 'W3' || workId === 'W4') return { ok: false, error: { code: 'ReadTimeout', message: 'slow' } };
        const response = demoWorkSnapshot(workId) as {
          result: { data: { observation: { progress: Record<string, unknown> | null } } };
        };
        response.result.data.observation.progress = workId === 'W1' ? { phase: 'ready_for_review' } : null;
        return response;
      },
    });
    await waitFor(() => {
      expect(screen.getByTestId('project-status-W1').textContent).toBe('待评审');
      expect(screen.getByTestId('project-status-W2').textContent).toBe('已认领');
      expect(screen.getByTestId('project-status-W3').textContent).toBe('进行中');
      expect(screen.getByTestId('project-status-W4').textContent).toBe('未知');
    });
    // the tone is one of the semantic colours: waiting for review is a warning, an unknown state is calm
    expect(screen.getByTestId('project-status-W1').querySelector('.ant-badge-status-dot')?.className).toContain('ant-badge-status-warning');
    expect(screen.getByTestId('project-status-W4').querySelector('.ant-badge-status-dot')?.className).toContain('ant-badge-status-default');
    expect(findForbiddenVisibleText(defaultVisibleText(screen.getByTestId('project-progress-table')))).toEqual([]);
  });

  it('says "reported as delivered" for a reporter\'s delivered that the system has not confirmed, and delivered once it holds the completion', async () => {
    setupOverview({
      snapshot: (workId) => {
        const response = demoWorkSnapshot(workId) as {
          result: { data: { runtime: { state: string; selected_completion_id?: string | null }; observation: { progress: Record<string, unknown> | null } } };
        };
        response.result.data.observation.progress = { phase: 'delivered' };
        if (workId === 'W2') response.result.data.runtime = { state: 'completed', selected_completion_id: 'completion-1' };
        return response;
      },
    });
    await waitFor(() => {
      expect(screen.getByTestId('project-status-W1').textContent).toBe('汇报称已交付');
      expect(screen.getByTestId('project-status-W2').textContent).toBe('已交付');
    });
    expect(screen.getByTestId('project-status-W1').querySelector('.ant-badge-status-dot')?.className).toContain('ant-badge-status-warning');
    expect(screen.getByTestId('project-status-W2').querySelector('.ant-badge-status-dot')?.className).toContain('ant-badge-status-success');
  });

  it('marks a report the service itself declared stale, and only that one', async () => {
    setupOverview({
      snapshot: (workId) => {
        const response = demoWorkSnapshot(workId) as {
          result: { data: { observation: { reporting: Record<string, unknown> } } };
        };
        response.result.data.observation.reporting.client_stale = workId === 'W1';
        return response;
      },
    });
    await waitFor(() => {
      expect(screen.getByTestId('last-report-W1').textContent).toBe(`${formatUnixMs(1761839800000, 'zh-CN')}可能已过期`);
      expect(screen.getByTestId('last-report-W2').textContent).toBe(formatUnixMs(1761839800000, 'zh-CN'));
    });
    expect(findForbiddenVisibleText(defaultVisibleText(screen.getByTestId('project-progress-table')))).toEqual([]);
  });

  it('uses not measured for missing or failed report times, never the current clock', async () => {
    const currentTime = formatUnixMs(Date.now(), 'zh-CN');
    setupOverview({
      snapshot: (workId) => {
        if (workId === 'W2') return { ok: false, error: { code: 'ReadTimeout', message: 'slow' } };
        const response = demoWorkSnapshot(workId) as {
          result: { data: { observation: { reporting: { client_reported_at_unix_ms?: number } } } };
        };
        delete response.result.data.observation.reporting.client_reported_at_unix_ms;
        return response;
      },
    });
    // Wait for hydration to finish instead of a fixed sleep: on a slow
    // runner the cells can still read 加载中 at the 1200ms mark.
    await waitFor(() => {
      expect(screen.getByTestId('last-report-W1').textContent).toBe('尚未读取');
      expect(screen.getByTestId('last-report-W2').textContent).toBe('尚未读取');
    }, { timeout: 4000 });
    expect(screen.getByTestId('last-report-W1').textContent).not.toBe(currentTime);
    expect(screen.getByTestId('last-report-W2').textContent).not.toBe(currentTime);
  });

  it('falls back to the overview row\'s next step, or says none was reported, when snapshots fail', async () => {
    const works = [
      { ...DEMO_PROJECT_WORKS[0], next_step: 'Review the contract' },
      { ...DEMO_PROJECT_WORKS[1], next_step: null },
    ];
    setupOverview({
      overview: makeProjectOverview(works),
      snapshot: () => ({ ok: false, error: { code: 'ReadTimeout', message: 'slow' } }),
    });
    await waitFor(() => {
      expect(screen.getByTestId('project-next-W1').textContent).toBe('Review the contract');
      expect(screen.getByTestId('project-next-W2').textContent).toBe('未报告下一步');
    });
  });

  it('groups shared workstreams once and uses an unlisted workstream id as its header', async () => {
    const works = [
      { ...DEMO_PROJECT_WORKS[0], key: 'SHARED1', title: 'Shared work one' },
      { ...DEMO_PROJECT_WORKS[0], key: 'SHARED2', title: 'Shared work two' },
      { ...DEMO_PROJECT_WORKS[0], key: 'OTHER', title: 'Other stream work', workstream_id: 'WS9' },
    ];
    setupOverview({ overview: makeProjectOverview(works) });
    await screen.findByRole('link', { name: 'Shared work two' });
    expect(screen.getAllByTestId('workstream-group-WS1')).toHaveLength(1);
    expect(screen.getByTestId('workstream-group-WS9').textContent).toBe('WS9');
    expect(document.querySelectorAll('[data-testid^="workstream-group-"]')).toHaveLength(2);
  });

  it('encodes same-task links for project and work IDs', async () => {
    const special = [{
      ...DEMO_PROJECT_WORKS[0],
      key: 'work /?&=中文',
      title: 'Encoded project task',
    }];
    setupOverview({
      path: '/projects/demo%20%2F%3F%26%3D?tab=overview',
      overview: makeProjectOverview(special),
    });
    const link = await screen.findByRole('link', { name: 'Encoded project task' });
    expect(link.getAttribute('href')).toBe(
      '/projects/demo%20%2F%3F%26%3D?tab=plan&work=work%20%2F%3F%26%3D%E4%B8%AD%E6%96%87',
    );
  });

  it('clamps an out-of-range page and keeps pagination visible', async () => {
    setupOverview({ path: '/projects/demo?tab=overview&page=5' });
    expect(await screen.findByTestId('project-progress-table')).toBeTruthy();
    expect(screen.getByRole('link', { name: 'Build the filter API' })).toBeTruthy();
    expect(screen.getByRole('link', { name: 'Prepare the support page' })).toBeTruthy();
    expect(document.querySelector('.ant-pagination')).not.toBeNull();
    expect(document.querySelector('.ant-pagination-item-active')?.textContent).toBe('1');
  });

  it('clamps again when the overview shrinks below the current page', async () => {
    const { queryClient } = setupOverview({
      path: '/projects/demo?tab=overview&page=2',
      overview: makeProjectOverview(makeProjectWorks(61)),
    });
    await screen.findByRole('link', { name: 'Observed project task 61' });

    act(() => {
      queryClient.setQueryData(queryKeys.overview('demo'), makeProjectOverview(makeProjectWorks(60)));
    });

    await screen.findByRole('link', { name: 'Observed project task 1' });
    expect(screen.queryByRole('link', { name: 'Observed project task 61' })).toBeNull();
    expect(document.querySelector('.ant-pagination-item-active')?.textContent).toBe('1');
  });

  it.each(['0', '-1', '1.5', '2e1'])('falls back to page one for page=%s', async (page) => {
    setupOverview({
      path: `/projects/demo?tab=overview&page=${page}`,
      overview: makeProjectOverview(makeProjectWorks(61)),
    });
    await screen.findByRole('link', { name: 'Observed project task 1' });
    expect(screen.queryByRole('link', { name: 'Observed project task 61' })).toBeNull();
  });

  it('writes pages to the URL, clears selection, and hydrates only visible rows', async () => {
    const user = userEvent.setup();
    const { fetchMock, router } = setupOverview({
      path: '/projects/demo?tab=overview&work=W1',
      overview: makeProjectOverview(makeProjectWorks(61)),
    });
    await screen.findByRole('link', { name: 'Observed project task 1' });
    await waitFor(() => expect(snapshotCalls(fetchMock)).toHaveLength(60));

    await user.click(screen.getByRole('listitem', { name: '下一页' }));
    await screen.findByRole('link', { name: 'Observed project task 61' });
    let params = new URLSearchParams(router.state.location.search);
    expect(params.get('page')).toBe('2');
    expect(params.get('work')).toBeNull();
    await waitFor(() => expect(snapshotCalls(fetchMock)).toHaveLength(61));

    await user.click(screen.getByRole('listitem', { name: '上一页' }));
    await screen.findByRole('link', { name: 'Observed project task 1' });
    params = new URLSearchParams(router.state.location.search);
    expect(params.get('page')).toBeNull();
  }, 20_000);

  it('clears page and work when changing project tabs', async () => {
    const user = userEvent.setup();
    const { router } = setupOverview({
      path: '/projects/demo?tab=overview&page=2&work=W61',
      overview: makeProjectOverview(makeProjectWorks(61)),
    });
    await screen.findByRole('link', { name: 'Observed project task 61' });
    await user.click(screen.getByRole('tab', { name: '工作计划' }));
    const params = new URLSearchParams(router.state.location.search);
    expect(params.get('page')).toBeNull();
    expect(params.get('work')).toBeNull();
  });
});

async function openTechnicalDetails(block: HTMLElement) {
  const user = userEvent.setup();
  for (const toggle of within(block).getAllByRole('button', { name: /技术细节/ })) await user.click(toggle);
}

describe('project coverage and freshness', () => {
  it('says in words how much was read, with an unknown total rather than zero, and keeps the raw rows behind the technical details', async () => {
    setupOverview();
    const coverage = await screen.findByTestId('project-coverage');
    expect(within(coverage).getByTestId('coverage-summary').textContent).toBe('已读取 2 个工作项，总数暂时未知');
    expect(within(coverage).getByTestId('freshness-badge')).toBeTruthy();
    expect(findForbiddenVisibleText(defaultVisibleText(coverage))).toEqual([]);
    expect(within(coverage).queryByTestId('coverage-works-total')).toBeNull();
    await openTechnicalDetails(coverage);
    expect(within(coverage).getByTestId('coverage-works-total').textContent).toContain('未提供');
    expect(within(coverage).getByTestId('technical-detail-coverage.works_enumerated').textContent).toContain('2');
    expect(within(coverage).getByTestId('technical-detail-coverage.scope.next_cursor').textContent).toContain('未提供');
  });

  it('says all of it was read when the total is known, and counts a partial batch against the total', async () => {
    setupOverview({ overview: { ...DEMO_PROJECT_OVERVIEW, works: DEMO_PROJECT_WORKS, coverage: { ...DEMO_PROJECT_OVERVIEW.coverage, works_enumerated: 2, works_total: 2, scope: { ...DEMO_PROJECT_OVERVIEW.coverage.scope, enumerated: 7 } } } });
    expect((await screen.findByTestId('coverage-summary')).textContent).toBe('已读取 2 / 2 个工作项');
    expect(screen.queryByTestId('coverage-failed-summary')).toBeNull();
  });

  it.each(['failed', 'succeeded'])('does not call zero enumerated empty with failed streams: %s scope', async (status) => {
    setupOverview({
      overview: {
        ...DEMO_PROJECT_OVERVIEW,
        works: [],
        coverage: {
          ...DEMO_PROJECT_OVERVIEW.coverage,
          failed: 1,
          works_enumerated: 0,
          scope: { ...DEMO_PROJECT_OVERVIEW.coverage.scope, status, enumerated: 0 },
          streams: [{
            workstream_id: 'WS1',
            status: 'failed',
            error_code: 'ReadTimeout',
            read_at_unix_ms: 1761831234000,
          }],
        },
      },
    });
    expect(await screen.findByTestId('state-partial')).toBeTruthy();
    expect(screen.queryByTestId('state-empty')).toBeNull();
    expect(screen.getByTestId('coverage-summary').textContent).toBe('已读取 0 个工作项，总数暂时未知');
  });

  it('retains a failed stream with its read time and confirmed rows', async () => {
    const failedAt = 1761831234000;
    setupOverview({
      overview: {
        ...DEMO_PROJECT_OVERVIEW,
        coverage: {
          ...DEMO_PROJECT_OVERVIEW.coverage,
          failed: 1,
          streams: [{
            workstream_id: 'WS1',
            status: 'failed',
            error_code: 'ReadTimeout',
            works_enumerated: 1,
            works_total: null,
            truncated: false,
            next_cursor: null,
            read_at_unix_ms: failedAt,
          }],
        },
      },
    });
    expect(await screen.findByTestId('state-partial')).toBeTruthy();
    const failed = screen.getByTestId('coverage-stream-WS1');
    expect(failed.textContent).toContain('读取失败');
    expect(failed.textContent).toContain(formatUnixMs(failedAt, 'zh-CN'));
    expect(failed.textContent).not.toContain('ReadTimeout');
    expect(failed.textContent).not.toContain('WS1');
    expect(screen.getByTestId('coverage-failed-summary').textContent).toBe('有 1 条工作线读取失败，列表可能不完整');
    const coverage = screen.getByTestId('project-coverage');
    expect(findForbiddenVisibleText(defaultVisibleText(coverage))).toEqual([]);
    await openTechnicalDetails(coverage);
    expect(within(coverage).getByTestId('technical-detail-coverage.streams.WS1').textContent).toContain('ReadTimeout');
    expect(screen.getByRole('link', { name: 'Build the filter API' })).toBeTruthy();
  });

  it('discloses truncation and an unread cursor instead of presenting a complete batch', async () => {
    setupOverview({
      overview: {
        ...DEMO_PROJECT_OVERVIEW,
        coverage: {
          ...DEMO_PROJECT_OVERVIEW.coverage,
          truncated: true,
          scope: { ...DEMO_PROJECT_OVERVIEW.coverage.scope, next_cursor: 'cursor-next-2' },
        },
      },
    });
    const unread = await screen.findByTestId('project-coverage-unread');
    expect(unread.textContent).toContain('还有未读部分');
    expect(unread.textContent).not.toContain('cursor-next-2');
    const coverage = screen.getByTestId('project-coverage');
    await openTechnicalDetails(coverage);
    expect(within(coverage).getByTestId('technical-detail-coverage.scope.next_cursor').textContent).toContain('cursor-next-2');
  });
});

describe('project recent development activity', () => {
  it('shows recent changes in plain words, keeps the raw records in the technical details, and reads development activity only', async () => {
    const { fetchMock } = setupOverview();
    const block = await screen.findByTestId('project-activity');
    const step = await within(block).findByTestId('recent-step-0');
    expect(within(step).getByTestId('recent-step-text').textContent).toBe('变更授权');
    expect(within(step).getByTestId('recent-step-time').textContent).toBe(formatUnixMs(1761839700000, 'zh-CN'));
    for (const hidden of ['access.changed', 'actor-raw-17', 'activity-1', 'audit.development', 'not_observed']) {
      expect(defaultVisibleText(block)).not.toContain(hidden);
    }
    expect(findForbiddenVisibleText(defaultVisibleText(block))).toEqual([]);
    expect(step.querySelector('[data-testid="activity-narrative"]')).toBeNull();
    const user = userEvent.setup();
    for (const toggle of within(block).getAllByRole('button', { name: /技术细节/ })) await user.click(toggle);
    expect((await within(block).findByTestId('recent-step-technical-0')).textContent).toContain('access.changed');
    expect(within(block).getByTestId('recent-step-technical-0').textContent).toContain('activity-1');
    expect(within(block).getByTestId('technical-detail-development_source').textContent).toContain('audit.development');
    expect(within(block).getByTestId('technical-detail-unknown_means').textContent).toContain('not_observed');
    expect(within(block).getByTestId('recent-changes-all').getAttribute('href')).toBe('/activity?project=demo');
    const activityUrls = fetchMock.mock.calls.map(([input]) => String(input))
      .filter((url) => url.includes('/api/team/activity'));
    expect(activityUrls).toHaveLength(1);
    expect(activityUrls[0]).toContain('kind=development');
    expect(activityUrls[0]).not.toContain('kind=requests');
  });

  it('uses the shared empty state for a successful empty activity read', async () => {
    setupOverview({ activity: makeActivity([]) });
    const block = await screen.findByTestId('project-activity');
    expect(await within(block).findByTestId('state-empty')).toBeTruthy();
  });

  it('presents Unsupported as unavailable in this mode, not a failure', async () => {
    setupOverview({ activity: { ok: false, error: { code: 'Unsupported', message: 'demo' } } });
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 1200));
    });
    expect(screen.getByTestId('activity-unavailable')).toBeTruthy();
    expect(screen.getByTestId('activity-unavailable').textContent).toContain('此模式不可用');
    expect(screen.queryByTestId('state-partial')).toBeNull();
  });

  it('presents upstream activity failure as partial failure', async () => {
    setupOverview({ activity: { ok: false, error: { code: 'ReadTimeout', message: 'slow' } } });
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 1200));
    });
    const block = screen.getByTestId('project-activity');
    const partial = within(block).getByTestId('state-partial');
    expect(partial.textContent).not.toContain('ReadTimeout');
    await userEvent.setup().click(within(partial).getByText('技术细节'));
    expect(await within(partial).findByText('ReadTimeout')).toBeTruthy();
  });

  it('discloses earlier activity records without continuation', async () => {
    setupOverview({ activity: makeActivity(DEMO_ACTIVITY.data.items, { next_cursor: 'older-2' }) });
    const block = await screen.findByTestId('project-activity');
    expect(await within(block).findByText('还有更早的记录')).toBeTruthy();
  });

  it('shows at most eight steps, newest first, and says that earlier changes exist', async () => {
    const items = Array.from({ length: 22 }, (_, index) => ({
      ...DEMO_ACTIVITY.data.items[0],
      id: `activity-${index + 1}`,
      action: index % 2 === 0 ? 'work.snapshot' : 'execution.start',
      created_at_unix_ms: 1761839700000 + index * 1000,
    }));
    setupOverview({ activity: makeActivity(items) });
    const block = await screen.findByTestId('project-activity');
    await waitFor(() => expect(block.querySelectorAll('[data-testid^="recent-step-"][data-testid$="0"], [data-testid^="recent-step-"]').length).toBeGreaterThan(0));
    const steps = Array.from(block.querySelectorAll('li[data-testid^="recent-step-"]'));
    expect(steps).toHaveLength(8);
    expect(within(steps[0] as HTMLElement).getByTestId('recent-step-text').textContent).toBe('开始执行');
    expect(within(steps[1] as HTMLElement).getByTestId('recent-step-text').textContent).toBe('查看工作现状');
    expect(within(block).getByTestId('recent-changes-more').textContent).toBe('更早的变化请在“活动与审计”里查看');
    const user = userEvent.setup();
    for (const toggle of within(block).getAllByRole('button', { name: /技术细节/ })) await user.click(toggle);
    expect(within(block).getByTestId('recent-step-technical-0').textContent).toContain('activity-22');
    expect(block.textContent).not.toContain('activity-1 ');
  });

  it('folds a run of the same kind into one counted step and marks an unknown result', async () => {
    const base = DEMO_ACTIVITY.data.items[0];
    const items = [
      ...Array.from({ length: 5 }, (_, index) => ({ ...base, id: `read-${index}`, action: 'work.snapshot', result: 'committed', created_at_unix_ms: 1761839700000 + index * 1000 })),
      { ...base, id: 'unknown-1', action: 'execution.report', result: 'unknown', created_at_unix_ms: 1761839700000 + 6000 },
    ];
    setupOverview({ activity: makeActivity(items) });
    const block = await screen.findByTestId('project-activity');
    const steps = await within(block).findAllByTestId(/^recent-step-\d+$/);
    expect(steps.map((step) => within(step).getByTestId('recent-step-text').textContent)).toEqual(['报告执行结果', '5 次读取']);
    expect(within(steps[0]).getByTestId('recent-step-unknown').textContent).toBe('结果未知');
    expect(within(steps[1]).queryByTestId('recent-step-unknown')).toBeNull();
    // A counted step shows when its newest record happened.
    expect(within(steps[1]).getByTestId('recent-step-time').textContent).toBe(formatUnixMs(1761839700000 + 4000, 'zh-CN'));
  });

  it('counts a repeated audit id once and says so in plain words', async () => {
    const base = DEMO_ACTIVITY.data.items[0];
    const items = [
      { ...base, id: 'dup-1', action: 'work.snapshot' },
      { ...base, id: 'dup-1', action: 'work.snapshot' },
      { ...base, id: 'other-1', action: 'work.snapshot', created_at_unix_ms: base.created_at_unix_ms - 1000 },
    ];
    setupOverview({ activity: makeActivity(items) });
    const block = await screen.findByTestId('project-activity');
    const steps = await within(block).findAllByTestId(/^recent-step-\d+$/);
    expect(steps).toHaveLength(1);
    expect(within(steps[0]).getByTestId('recent-step-text').textContent).toBe('2 次读取');
    expect(within(block).getByTestId('recent-note-duplicate-id').textContent).toBe('发现重复记录，原始事件已全部保留');
  });

});

describe('project overview placeholders, localization, and accessibility', () => {
  it('keeps the dependencies block as a later-slice placeholder', async () => {
    setupOverview();
    // Dependencies have no read yet; goals and delivery are filled from the reads the page makes.
    expect((await screen.findByTestId('project-deps-placeholder')).textContent).toContain('尚未开放');
    expect(screen.queryByTestId('project-goals-placeholder')).toBeNull();
    expect(screen.queryByTestId('project-delivery-placeholder')).toBeNull();
  });

  it('renders new overview strings in English', async () => {
    setupOverview();
    await screen.findByRole('link', { name: 'Build the filter API' });
    await act(() => i18n.changeLanguage('en-US'));
    expect(await screen.findByText('Workstream progress')).toBeTruthy();
    expect(screen.getByText('Recent changes')).toBeTruthy();
    expect(screen.getByText('How much was read')).toBeTruthy();
    expect(screen.getByTestId('coverage-summary').textContent).toBe('Read 2 work items; the total is not known yet');
  });

  it('has no axe violations', async () => {
    const { container } = setupOverview();
    await screen.findByRole('link', { name: 'Build the filter API' });
    const results = await axe(container, {
      rules: { 'color-contrast': { enabled: false } },
    });
    expect(results).toHaveNoViolations();
  });
});

describe('overview goals and delivery blocks (read from the page\'s own snapshot reads)', () => {
  function snapshotWithContract(workId: string, contract: Record<string, unknown> | null) {
    const base = demoWorkSnapshot(workId) as { result: { data: Record<string, unknown> } };
    if (contract === null) delete base.result.data.visible_contract;
    else base.result.data.visible_contract = contract;
    return base;
  }

  function opsOf(fetchMock: ReturnType<typeof vi.fn>) {
    const ops = new Map<string, number>();
    for (const [, init] of fetchMock.mock.calls as Array<[string, RequestInit | undefined]>) {
      const body = String(init?.body ?? '');
      if (!body.includes('"op"')) continue;
      const op = (JSON.parse(body).query as { op: string }).op;
      ops.set(op, (ops.get(op) ?? 0) + 1);
    }
    return ops;
  }

  it('sends no read the page did not already make: the same snapshot ops, nothing else new', async () => {
    const { fetchMock } = setupOverview();
    await screen.findAllByTestId('project-status-W2');
    const ops = opsOf(fetchMock);
    // The blocks read only the snapshot queries the table already drives, once per visible work.
    expect(ops.get('work.snapshot')).toBe(2);
    expect([...ops.keys()].filter((op) => op !== 'work.snapshot')).toEqual([]);
  });

  it('merges works that agreed on their goals into one group named by both', async () => {
    const snapshot = (id: string) => snapshotWithContract(id, {
      goals: ['Keep support filters useful'],
      acceptance: ['Do not mutate input records'],
    });
    setupOverview({ snapshot });
    const groups = await screen.findAllByTestId('project-goals-group');
    expect(groups).toHaveLength(1);
    const joined = groups[0].textContent ?? '';
    expect(joined).toContain('Build the filter API');
    expect(joined).toContain('Prepare the support page');
    expect(joined).toContain('Keep support filters useful');
  });

  it('keeps different agreements as separate groups', async () => {
    const snapshot = (id: string) => snapshotWithContract(id, id === 'W2'
      ? { goals: ['A different goal'], acceptance: ['A different acceptance'] }
      : { goals: ['Keep support filters useful'], acceptance: ['Do not mutate input records'] });
    setupOverview({ snapshot });
    await screen.findAllByTestId('project-goals-group');
    expect(screen.getAllByTestId('project-goals-group')).toHaveLength(2);
  });

  it('shows the first five groups and says how many more there are', async () => {
    const works = Array.from({ length: 7 }, (_, index) => ({
      key: `W${index + 1}`, title: `Task ${index + 1}`, status: 'unknown', owner_person: null, agent: null,
      next_step: null, detail_loaded: false, workstream_id: 'WS1', contract_hash: `c${index}`,
    }));
    let m = 0;
    setupOverview({
      overview: { ...DEMO_PROJECT_OVERVIEW, works, coverage: { ...DEMO_PROJECT_OVERVIEW.coverage, works_enumerated: 7 } },
      snapshot: (id: string) => snapshotWithContract(id, { goals: [`Goal ${++m}`], acceptance: [] }),
    });
    expect(await screen.findAllByTestId('project-goals-group')).toHaveLength(5);
    expect(screen.getByTestId('project-goals').textContent).toContain('还有');
  });

  it('draws no checkbox: nothing in this data says whether an acceptance item is met', async () => {
    setupOverview();
    await screen.findAllByTestId('project-goals-group');
    expect(document.querySelector('[data-testid="project-goals"] input[type="checkbox"]')).toBeNull();
    expect(screen.getByTestId('project-goals').textContent).toContain('Do not mutate input records');
  });

  it('counts progress words only from reads that happened, and gives no project verdict', async () => {
    setupOverview();
    await screen.findAllByTestId('project-delivery-counts');
    expect(screen.getByTestId('project-delivery-counts').textContent).toContain('已认领');
    expect(screen.getByTestId('project-delivery-counts').textContent).toContain('× 2');
    // Loading and unread works are named separately, never folded into a word.
    expect(screen.getByTestId('project-delivery-counts').textContent).not.toContain('加载中');
    expect(screen.getByTestId('project-delivery-counts').textContent).not.toContain('未知');
    // The demo coverage knows no total, so the scope sentence states no total either.
    expect(screen.getByTestId('project-delivery-scope').textContent).toBe('本页 2 项工作，已读到 2 项');
    // No whole-project verdict: the prototype's judgement sentence is not ours to make.
    const visible = document.querySelector('[data-testid="project-overview"]')?.textContent ?? '';
    expect(visible).not.toContain('最终验收');
    expect(visible).not.toContain('不具备');
  });

  it('names a failed read, counts it nowhere, and drops it from the word counts', async () => {
    // A failed read is the bridge's error envelope (throwing inside the mock leaves the
    // query pending forever under p-limit, which is a loading read, not a failed one).
    const snapshot = (id: string) => (id === 'W2'
      ? { ok: false, error: { code: 'ReadFailed', message: 'synthetic read failure' } }
      : demoWorkSnapshot(id));
    setupOverview({ snapshot });
    // Wait for the read to actually fail (the table's word stops being the loading one);
    // a query that never settles is a loading read, and this test is about a failed one.
    await waitFor(() => expect(screen.getByTestId('project-status-W2').textContent).not.toBe('加载中…'));
    expect(screen.getByTestId('project-status-W2').textContent).toBe('未知');
    expect(screen.getByTestId('project-delivery-unread')).toBeTruthy();
    expect(screen.getByTestId('project-delivery-unread').textContent).toContain('1');
    // One word bucket only — the failed work appears as no word at all, whatever the row says.
    expect(screen.getByTestId('project-delivery-counts').querySelectorAll('.ant-typography-secondary')).toHaveLength(1);
    expect(screen.getByTestId('project-delivery-counts').textContent).toContain('× 1');
    expect(screen.getByTestId('project-delivery-counts').textContent).not.toContain('× 2');
    expect(screen.getByTestId('project-delivery-counts').textContent).not.toContain('未知');
    expect(screen.getByTestId('project-delivery-counts').textContent).not.toContain('加载中');
    expect(screen.getByTestId('project-delivery-counts').textContent).not.toContain('未提供');
  });

  it('names the works that need attention with their word, and caps the list', async () => {
    setupOverview();
    await screen.findAllByTestId('project-delivery-attention');
    const attention = screen.getByTestId('project-delivery-attention').textContent ?? '';
    expect(attention).toContain('Build the filter API');
    expect(attention).toContain('已认领');
    expect(screen.getByTestId('project-delivery-attention').querySelectorAll('a').length).toBe(2);
  });

  it('says it is reading while the reads are running, in both blocks', async () => {
    let release: () => void = () => {};
    const gate = new Promise<void>((resolve) => { release = resolve; });
    setupOverview({ snapshotGate: gate });
    expect(await screen.findByTestId('project-goals-loading')).toBeTruthy();
    expect(screen.getByTestId('project-goals-loading').textContent).toContain('加载中');
    expect(await screen.findByTestId('project-delivery-loading')).toBeTruthy();
    const goalsText = document.querySelector('[data-testid="project-goals"]')?.textContent ?? '';
    const deliveryText = document.querySelector('[data-testid="project-delivery"]')?.textContent ?? '';
    expect(goalsText).not.toContain('没有登记');
    expect(deliveryText).not.toContain('读取不到');
    release();
  });

  it('with one read of two failed, the page count and the read count each say their own thing', async () => {
    const snapshot = (id: string) => {
      if (id === 'W2') throw new Error('synthetic read failure');
      return demoWorkSnapshot(id);
    };
    setupOverview({ snapshot });
    expect(await screen.findByTestId('project-delivery-scope')).toBeTruthy();
    // The second number is the reads that happened (1), not the page count (2): swapping
    // them changes the sentence, and the English line carries both numbers too.
    expect(screen.getByTestId('project-delivery-scope').textContent).toBe('本页 2 项工作，已读到 1 项');
    await act(() => i18n.changeLanguage('en-US'));
    expect(screen.getByTestId('project-delivery-scope').textContent).toBe('2 works on this page, 1 read');
    await act(() => i18n.changeLanguage('zh-CN'));
    expect(screen.getByTestId('project-delivery-unread').textContent).toContain('1');
  });

  it('takes the project total from the coverage card, not from the length of the list', async () => {
    const works = Array.from({ length: 2 }, (_, index) => ({
      key: `W${index + 1}`, title: `Task ${index + 1}`, status: 'unknown', owner_person: null, agent: null,
      next_step: null, detail_loaded: false, workstream_id: 'WS1', contract_hash: `c${index}`,
    }));
    // One read of two fails, so the sentence's three numbers are all different: page 2,
    // read 1, total 100 — no two of them can stand in for each other.
    const snapshot = (id: string) => (id === 'W2'
      ? { ok: false, error: { code: 'ReadFailed', message: 'synthetic read failure' } }
      : demoWorkSnapshot(id));
    setupOverview({
      overview: {
        ...DEMO_PROJECT_OVERVIEW,
        works,
        coverage: { ...DEMO_PROJECT_OVERVIEW.coverage, works_total: 100, works_enumerated: 2 },
      },
      snapshot,
    });
    expect(await screen.findByTestId('project-delivery-scope')).toBeTruthy();
    expect(screen.getByTestId('project-delivery-scope').textContent).toBe('本页 2 项工作，已读到 1 项，项目共 100 项');
    // One total on the page: the coverage card's own sentence matches.
    expect(screen.getByTestId('coverage-summary').textContent).toContain('100');
  });

  it('keeps goals and acceptance apart under their own small headings', async () => {
    setupOverview();
    const group = await screen.findByTestId('project-goals-group');
    expect(group.textContent).toContain('目标');
    expect(group.textContent).toContain('验收');
    expect(group.textContent?.indexOf('目标')).toBeLessThan(group.textContent?.indexOf('验收') ?? 0);
  });

  it('says how many more groups of agreements there are, in the block\'s own words', async () => {
    const works = Array.from({ length: 7 }, (_, index) => ({
      key: `W${index + 1}`, title: `Task ${index + 1}`, status: 'unknown', owner_person: null, agent: null,
      next_step: null, detail_loaded: false, workstream_id: 'WS1', contract_hash: `c${index}`,
    }));
    let m = 0;
    setupOverview({
      overview: { ...DEMO_PROJECT_OVERVIEW, works, coverage: { ...DEMO_PROJECT_OVERVIEW.coverage, works_enumerated: 7 } },
      snapshot: (id: string) => {
        const base = demoWorkSnapshot(id) as { result: { data: Record<string, unknown> } };
        base.result.data.visible_contract = { goals: [`Goal ${++m}`], acceptance: [] };
        return base;
      },
    });
    expect(await screen.findByTestId('project-goals-more')).toBeTruthy();
    expect(screen.getByTestId('project-goals-more').textContent).toContain('还有 2 组约定');
  });

  it('caps the attention list at five with the more line, and each link goes to its own work', async () => {
    const works = Array.from({ length: 8 }, (_, index) => ({
      key: `W${index + 1}`, title: `Task ${index + 1}`, status: 'unknown', owner_person: null, agent: null,
      next_step: null, detail_loaded: false, workstream_id: 'WS1', contract_hash: `c${index}`,
    }));
    // W1 finished (positive) and W3 unrecognized (neutral) stay out of the attention list;
    // W2 is blocked (negative) and must be named; six named works hit the cap of five.
    const snapshot = (id: string) => {
      const base = demoWorkSnapshot(id) as { result: { data: Record<string, unknown> } };
      const data = base.result.data as { runtime: Record<string, unknown>; observation: Record<string, unknown> };
      if (id === 'W1') {
        data.runtime = { ...data.runtime, state: 'completed', selected_completion_id: '01M6COMPLETION000000000000000' };
        data.observation = { ...data.observation, progress: { phase: 'delivered' } };
      }
      if (id === 'W2') {
        data.runtime = { ...data.runtime, state: 'blocked' };
        data.observation = { ...data.observation, progress: { phase: 'blocked' } };
      }
      if (id === 'W3') {
        data.runtime = { ...data.runtime, state: 'made-up-state' };
      }
      return base;
    };
    setupOverview({
      overview: { ...DEMO_PROJECT_OVERVIEW, works, coverage: { ...DEMO_PROJECT_OVERVIEW.coverage, works_enumerated: 7 } },
      snapshot,
    });
    await screen.findByTestId('project-delivery-attention');
    const attention = screen.getByTestId('project-delivery-attention');
    const links = attention.querySelectorAll('a');
    expect(links.length).toBe(5);
    expect(attention.textContent).toContain('还有');
    expect(links[0].getAttribute('href')).toContain('work=W2');
    expect(links[0].textContent).toContain('受阻');
    expect(links[4].getAttribute('href')).toContain('work=W7');
    // Waiting and blocked words own the list; the finished and unrecognized ones never appear.
    expect(attention.textContent).not.toContain('Task 1');
    expect(screen.getByTestId('project-delivery-counts').textContent).toContain('已交付× 1');
    expect(screen.getByTestId('project-delivery-counts').textContent).toContain('受阻× 1');
    // A neutral word is counted as a word but never named in the list above.
    expect(screen.getByTestId('project-delivery-counts').textContent).toContain('未知× 1');
    expect(attention.textContent).not.toContain('Task 3');
  });

  it('says the agreements cannot be read when every read failed, in its own words', async () => {
    const fetchMock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = String(input);
      if (url.includes('/api/team/projects')) return json(DEMO_PROJECTS);
      if (url.includes('/api/team/overview')) return json(DEMO_PROJECT_OVERVIEW);
      if (url.includes('/api/team/activity')) return json(DEMO_ACTIVITY);
      if (url.includes('/api/team/read')) {
        const request = JSON.parse(String(init?.body)) as { query: { op: string } };
        if (request.query.op === 'work.snapshot') {
          return json({ ok: false, error: { code: 'ReadFailed', message: 'synthetic read failure' } });
        }
      }
      return json({ ok: false, error: { code: 'NoRoute', message: url } });
    });
    vi.stubGlobal('fetch', fetchMock);
    renderApp('/projects/demo?tab=overview');
    const failed = await screen.findByTestId('project-goals-failed');
    expect(failed.textContent).toContain('约定暂时读取不到');
    expect(failed.textContent).not.toContain('进展');
  });

  it('keeps both blocks off the page after revocation', async () => {
    const { queryClient } = setupOverview();
    await screen.findAllByTestId('project-status-W2');
    act(() => revokeObservationReads(queryClient));
    expect(await screen.findByTestId('state-forbidden')).toBeTruthy();
    expect(screen.queryByTestId('project-goals')).toBeNull();
    expect(screen.queryByTestId('project-delivery')).toBeNull();
  });

  it('keeps both blocks off the page after revocation', async () => {
    const { queryClient } = setupOverview();
    await screen.findAllByTestId('project-status-W2');
    act(() => revokeObservationReads(queryClient));
    expect(await screen.findByTestId('state-forbidden')).toBeTruthy();
    expect(screen.queryByTestId('project-goals')).toBeNull();
    expect(screen.queryByTestId('project-delivery')).toBeNull();
  });

  it('says loading and no unread count while every read is pending; the count names the mixed state', async () => {
    let release: () => void = () => {};
    const gate = new Promise<void>((resolve) => { release = resolve; });
    setupOverview({ snapshotGate: gate });
    expect(await screen.findByTestId('project-goals-loading')).toBeTruthy();
    // The loading sentence already says it; an unread count would repeat it in other words.
    expect(screen.queryByTestId('project-goals-unread')).toBeNull();
    release();
  });

  it('keeps the unread sentence on its own line in the mixed state', async () => {
    const snapshot = (id: string) => (id === 'W2'
      ? { ok: false, error: { code: 'ReadFailed', message: 'synthetic read failure' } }
      : demoWorkSnapshot(id));
    setupOverview({ snapshot });
    const unread = await screen.findByTestId('project-goals-unread');
    // Its own line: a block container, not an inline run that can touch the sentence above it.
    expect(unread.tagName).toBe('DIV');
    expect(unread.style.display).toBe('block');
  });

  it('shows exactly one sentence when every read failed, on its own line, not inline with another', async () => {
    const fetchMock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = String(input);
      if (url.includes('/api/team/projects')) return json(DEMO_PROJECTS);
      if (url.includes('/api/team/overview')) return json(DEMO_PROJECT_OVERVIEW);
      if (url.includes('/api/team/activity')) return json(DEMO_ACTIVITY);
      if (url.includes('/api/team/read')) {
        const request = JSON.parse(String(init?.body)) as { query: { op: string } };
        if (request.query.op === 'work.snapshot') {
          return json({ ok: false, error: { code: 'ReadFailed', message: 'synthetic read failure' } });
        }
      }
      return json({ ok: false, error: { code: 'NoRoute', message: url } });
    });
    vi.stubGlobal('fetch', fetchMock);
    renderApp('/projects/demo?tab=overview');
    const failed = await screen.findByTestId('project-goals-failed');
    // Only the failure sentence: no unread sentence repeating it, nothing inline beside it.
    expect(screen.queryByTestId('project-goals-unread')).toBeNull();
    const block = failed.closest('section');
    expect(block?.textContent).toContain('约定暂时读取不到');
    const lineTexts = [...(block?.querySelectorAll('[data-testid], div, span') ?? [])]
      .filter((el) => el.children.length === 0 && el.textContent && el.textContent.trim().length > 0)
      .map((el) => el.textContent);
    expect(lineTexts.some((text) => text.includes('尚未读取'))).toBe(false);
    // The English sentence is pinned as a whole, not by a shared word.
    cleanup();
    const fetchEn = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = String(input);
      if (url.includes('/api/team/projects')) return json(DEMO_PROJECTS);
      if (url.includes('/api/team/overview')) return json(DEMO_PROJECT_OVERVIEW);
      if (url.includes('/api/team/activity')) return json(DEMO_ACTIVITY);
      if (url.includes('/api/team/read')) {
        const request = JSON.parse(String(init?.body)) as { query: { op: string } };
        if (request.query.op === 'work.snapshot') {
          return json({ ok: false, error: { code: 'ReadFailed', message: 'synthetic read failure' } });
        }
      }
      return json({ ok: false, error: { code: 'NoRoute', message: url } });
    });
    vi.stubGlobal('fetch', fetchEn);
    renderApp('/projects/demo?tab=overview');
    await act(() => i18n.changeLanguage('en-US'));
    const failedEn = await screen.findByTestId('project-goals-failed');
    expect(failedEn.textContent).toBe('The agreements of these works cannot be read right now');
    expect(failedEn.textContent).not.toContain('How far these works have come');
  });
});
