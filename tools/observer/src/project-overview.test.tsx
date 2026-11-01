import { act, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { axe, toHaveNoViolations } from 'jest-axe';
import { afterEach, describe, expect, it, vi } from 'vitest';
import i18n from './i18n';
import { formatUnixMs } from './lib/format';
import { queryKeys } from './lib/queries';
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
}: {
  path?: string;
  overview?: unknown;
  activity?: unknown;
  snapshot?: (workId: string) => unknown;
} = {}) {
  const fetchMock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
    const url = String(input);
    if (url.includes('/api/team/projects')) return json(DEMO_PROJECTS);
    if (url.includes('/api/team/overview')) return json(overview);
    if (url.includes('/api/team/activity')) return json(activity);
    if (url.includes('/api/team/read')) {
      const request = JSON.parse(String(init?.body)) as { query: { op: string; work_id?: string } };
      if (request.query.op === 'work.snapshot') return json(snapshot(request.query.work_id ?? ''));
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
  it('renders real groups and fields, honest unknowns, and snapshot next step/report time', async () => {
    setupOverview();
    expect(await screen.findByRole('link', { name: 'Build the filter API' })).toBeTruthy();
    expect(screen.getByTestId('workstream-group-WS1').textContent).toContain('Backend workstream');
    expect(screen.getByTestId('workstream-group-WS2').textContent).toContain('Frontend workstream');
    expect(screen.getByTestId('project-status-W1').textContent).toBe('未知');
    expect(screen.getByTestId('project-owner-W1').textContent).toBe('未提供');
    expect(screen.getByTestId('project-next-W1').textContent).toBe('session.checkpoint');
    expect(screen.getByTestId('last-report-W1').textContent).toBe(formatUnixMs(1761839800000, 'zh-CN'));
    expect(screen.getByTestId('last-report-W1').textContent).not.toBe(formatUnixMs(1761839900000, 'zh-CN'));
    expect(screen.getByTestId('last-report-W1').style.whiteSpace).toBe('nowrap');
    const timeColumn = screen.getByRole('columnheader', { name: '最近报告' }).closest('table')?.querySelector('colgroup')?.lastElementChild;
    expect(timeColumn?.style.width || timeColumn?.getAttribute('width')).toBe('220px');
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

  it('shows a compact guidance op and keeps the long note out of the cell text', async () => {
    const longNote = 'Batch progress with next_action/open_loops. Stop and report workspace execution before repository push or publication. '.repeat(4);
    setupOverview({
      snapshot: (workId) => {
        const response = demoWorkSnapshot(workId);
        response.result.data.guidance.action.note = longNote;
        return response;
      },
    });
    const cell = await screen.findByTestId('project-next-W1');
    await waitFor(() => expect(cell.textContent).toBe('session.checkpoint'));
    expect(cell.textContent.length).toBeLessThan(80);
    expect(cell.textContent).not.toContain('Batch progress');
    // The full agent-facing note stays available behind an always-on
    // tooltip (ellipsis.tooltip would never fire on the short op).
    const user = userEvent.setup();
    await user.hover(cell);
    expect(await screen.findByText(/Batch progress with next_action/)).toBeTruthy();
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
      expect(screen.getByTestId('last-report-W1').textContent).toBe('未测量');
      expect(screen.getByTestId('last-report-W2').textContent).toBe('未测量');
    }, { timeout: 4000 });
    expect(screen.getByTestId('last-report-W1').textContent).not.toBe(currentTime);
    expect(screen.getByTestId('last-report-W2').textContent).not.toBe(currentTime);
  });

  it('falls back to the overview next step or not measured when snapshots fail', async () => {
    const works = [
      { ...DEMO_PROJECT_WORKS[0], next_step: 'Review the contract' },
      { ...DEMO_PROJECT_WORKS[1], next_step: null },
    ];
    setupOverview({
      overview: makeProjectOverview(works),
      snapshot: () => ({ ok: false, error: { code: 'ReadTimeout', message: 'slow' } }),
    });
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 1200));
    });
    expect(screen.getByTestId('project-next-W1').textContent).toBe('Review the contract');
    expect(screen.getByTestId('project-next-W2').textContent).toBe('未提供');
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

describe('project coverage and freshness', () => {
  it('shows real coverage with an unknown total rather than zero', async () => {
    setupOverview();
    const coverage = await screen.findByTestId('project-coverage');
    expect(within(coverage).getByTestId('coverage-works-total').textContent).toBe('未知');
    expect(within(coverage).getByTestId('coverage-works-total').textContent).not.toBe('0');
    expect(within(coverage).getByTestId('coverage-enumerated').textContent).toBe('2');
    expect(within(coverage).getByTestId('freshness-badge')).toBeTruthy();
  });

  it.each(['failed', 'succeeded'])('does not call zero enumerated empty with failed streams: %s scope', async (status) => {
    setupOverview({
      overview: {
        ...DEMO_PROJECT_OVERVIEW,
        works: [],
        coverage: {
          ...DEMO_PROJECT_OVERVIEW.coverage,
          failed: 1,
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
    expect(failed.textContent).toContain('ReadTimeout');
    expect(failed.textContent).toContain(formatUnixMs(failedAt, 'zh-CN'));
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
    expect(unread.textContent).toContain('cursor-next-2');
  });
});

describe('project recent development activity', () => {
  it('renders raw fields, source notes, and development-only reads without narrative', async () => {
    const { fetchMock } = setupOverview();
    const block = await screen.findByTestId('project-activity');
    const item = await within(block).findByTestId('activity-item-activity-1');
    expect(within(item).getByTestId('activity-action').textContent).toBe('access.changed');
    expect(within(item).getByTestId('activity-actor').textContent).toBe('actor-raw-17');
    expect(within(item).getByTestId('activity-result').textContent).toBe('granted');
    expect(within(item).getByTestId('activity-time').textContent)
      .toBe(formatUnixMs(1761839700000, 'zh-CN'));
    expect(block.textContent).toContain('audit.development');
    expect(block.textContent).toContain('not_observed');
    expect(item.querySelector('[data-testid="activity-narrative"]')).toBeNull();
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
    expect(within(block).getByTestId('state-partial').textContent).toContain('ReadTimeout');
  });

  it('discloses earlier activity records without continuation', async () => {
    setupOverview({ activity: makeActivity(DEMO_ACTIVITY.data.items, { next_cursor: 'older-2' }) });
    const block = await screen.findByTestId('project-activity');
    expect(await within(block).findByText('还有更早的记录')).toBeTruthy();
  });

  it('shows at most twenty items, newest first', async () => {
    const items = Array.from({ length: 22 }, (_, index) => ({
      ...DEMO_ACTIVITY.data.items[0],
      id: `activity-${index + 1}`,
      action: `action-${index + 1}`,
      actor_id: `actor-${index + 1}`,
      created_at_unix_ms: 1761839700000 + index * 1000,
    }));
    setupOverview({ activity: makeActivity(items) });
    const block = await screen.findByTestId('project-activity');
    await waitFor(() => expect(block.querySelectorAll('[data-testid^="activity-item-"]')).toHaveLength(20));
    const rendered = block.querySelectorAll('[data-testid^="activity-item-"]');
    expect(rendered[0].getAttribute('data-testid')).toBe('activity-item-activity-22');
    expect(rendered[19].getAttribute('data-testid')).toBe('activity-item-activity-3');
    expect(block.querySelector('[data-testid="activity-item-activity-1"]')).toBeNull();
    expect(block.querySelector('[data-testid="activity-item-activity-2"]')).toBeNull();
  });
});

describe('project overview placeholders, localization, and accessibility', () => {
  it('keeps unsourced blocks as later-slice placeholders', async () => {
    setupOverview();
    for (const id of ['project-goals-placeholder', 'project-delivery-placeholder', 'project-deps-placeholder']) {
      expect((await screen.findByTestId(id)).textContent).toContain('后续切片');
    }
  });

  it('renders new overview strings in English', async () => {
    setupOverview();
    await screen.findByRole('link', { name: 'Build the filter API' });
    await act(() => i18n.changeLanguage('en-US'));
    expect(await screen.findByText('Workstream progress')).toBeTruthy();
    expect(screen.getByText('Recent changes')).toBeTruthy();
    expect(screen.getByText('Coverage & freshness')).toBeTruthy();
    expect(screen.getByTestId('coverage-works-total').textContent).toBe('unknown');
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
