import { QueryClientProvider } from '@tanstack/react-query';
import { render, screen } from '@testing-library/react';
import { MemoryRouter } from 'react-router';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { OVERVIEW_PARTS, type OverviewInput } from '../core/overview';
import i18n from '../i18n';
import { CraftThemeProvider } from '../components/CraftThemeProvider';
import { PerspectiveProvider } from '../lib/perspective/PerspectiveProvider';
import { unknownPerspectiveFacts } from '../lib/perspective/types';
import { createAppQueryClient, revokeObservationReads } from '../lib/queries';
import { RevocationProvider } from '../lib/revocation';
import { ScopeProvider } from '../lib/scope';
import { stubFetch } from '../test-utils';
import { myWorkInput } from '../test-fixtures/workbench';
import { WorkbenchPage } from './WorkbenchPage';

const PROJECT = { key: 'demo', title: 'Synthetic Project', role: 'developer' };

function responseFor(input: OverviewInput) {
  return {
    ok: true,
    project: 'demo',
    read_at_unix_ms: input.options.nowUnixMs,
    input,
    coverage: Object.fromEntries(OVERVIEW_PARTS.map((part) => [part, {
      status: input[part].coverage,
      ...(input[part].coverage === 'complete' ? {} : { reason: input[part].reason }),
      missing: [],
    }])),
    freshness: {
      report_received: { status: 'not_measured' },
      authoritative: { status: 'measured', at_unix_ms: input.options.nowUnixMs },
      page_read: { status: 'measured', at_unix_ms: input.options.nowUnixMs },
    },
  };
}

function emptyInput() {
  return myWorkInput({
    tasks: { coverage: 'complete', items: [] },
    dependencies: { coverage: 'complete', items: [] },
    decisions: { coverage: 'complete', items: [] },
    reviewRounds: { coverage: 'complete', items: [] },
    handoffs: { coverage: 'complete', items: [] },
    approvals: { coverage: 'complete', items: [] },
    activity: { coverage: 'complete', items: [] },
  });
}

function renderWorkbench(queryClient = createAppQueryClient({ retry: false })) {
  const utils = render(
    <QueryClientProvider client={queryClient}>
      <CraftThemeProvider>
        <RevocationProvider>
          <MemoryRouter initialEntries={['/workbench']}>
            <ScopeProvider>
              <PerspectiveProvider facts={unknownPerspectiveFacts()}>
                <WorkbenchPage />
              </PerspectiveProvider>
            </ScopeProvider>
          </MemoryRouter>
        </RevocationProvider>
      </CraftThemeProvider>
    </QueryClientProvider>,
  );
  return { queryClient, ...utils };
}

function stubModel(input: OverviewInput, extra: Record<string, unknown> = {}) {
  return stubFetch({
    '/api/team/overview-model': { ...responseFor(input), ...extra },
    '/api/team/projects': { ok: true, projects: [PROJECT] },
  });
}

beforeEach(async () => {
  await i18n.changeLanguage('zh-CN');
});

describe('WorkbenchPage seven read states in Chinese and English', () => {
  it.each(['zh-CN', 'en-US'] as const)('renders data, empty, partial and unknown in %s', async (language) => {
    await i18n.changeLanguage(language);
    stubModel(myWorkInput());
    const populated = renderWorkbench();
    expect(await screen.findByTestId('workbench-frame')).toBeTruthy();
    expect(screen.getByTestId('workbench-frame').getAttribute('data-state')).toBe('data');
    expect(screen.getByTestId('freshness-badge')).toBeTruthy();
    populated.unmount();

    stubModel(emptyInput());
    const empty = renderWorkbench();
    expect((await screen.findByTestId('workbench-frame')).getAttribute('data-state')).toBe('empty');
    expect(screen.getAllByTestId('obs-empty-state').length).toBeGreaterThan(0);
    empty.unmount();

    stubModel(myWorkInput({ dependencies: { coverage: 'partial', reason: 'read-failed', items: [] } }));
    const partial = renderWorkbench();
    expect((await screen.findByTestId('workbench-frame')).getAttribute('data-state')).toBe('partial');
    expect(screen.getByTestId('workbench-partial')).toBeTruthy();
    expect(document.body.textContent).not.toMatch(/\b(?:read-failed|coverage)\b/);
    partial.unmount();

    stubModel(myWorkInput(), { ordering: 'incomparable' });
    const unknown = renderWorkbench();
    expect((await screen.findByTestId('workbench-frame')).getAttribute('data-state')).toBe('unknown');
    expect(screen.getByTestId('workbench-ordering-unknown')).toBeTruthy();
    unknown.unmount();
  });

  it.each([
    [20_000, '刚刚'],
    [5 * 60_000, '5 分钟前'],
  ])('reads the generated source age against the clock at render, %i ms after the read', async (afterReadMs, expected) => {
    const input = myWorkInput();
    // The reply carries page_read.at_unix_ms equal to read_at_unix_ms, so a reference taken from the reply would
    // make every page read 刚刚 however old the read is; the reference is the clock at render, like the activity page.
    stubModel(input);
    vi.useFakeTimers({ toFake: ['Date'] });
    try {
      vi.setSystemTime(input.options.nowUnixMs + afterReadMs);
      renderWorkbench();
      expect(await screen.findByTestId('workbench-frame')).toBeTruthy();
      expect(screen.getByText(`按你名下的任务和 Agent 的报告算出 · ${expected}`)).toBeTruthy();
    } finally {
      vi.useRealTimers();
    }
  });

  it.each(['zh-CN', 'en-US'] as const)('renders loading and failure in %s', async (language) => {
    await i18n.changeLanguage(language);
    vi.stubGlobal('fetch', vi.fn(async (input: RequestInfo | URL) => {
      const url = String(input);
      if (url.includes('/api/team/projects')) {
        return new Response(JSON.stringify({ ok: true, projects: [PROJECT] }), { status: 200, headers: { 'content-type': 'application/json' } });
      }
      return new Promise<Response>(() => {});
    }));
    const loading = renderWorkbench();
    expect(await screen.findByTestId('workbench-loading')).toBeTruthy();
    expect(screen.getByTestId('freshness-badge')).toBeTruthy();
    loading.unmount();

    stubFetch({
      '/api/team/overview-model': { ok: false, error: { code: 'Unavailable', message: 'synthetic failure' } },
      '/api/team/projects': { ok: true, projects: [PROJECT] },
    });
    renderWorkbench();
    expect(await screen.findByTestId('workbench-failed')).toBeTruthy();
    expect(document.body.textContent).not.toContain('Unavailable');
    expect(document.body.textContent).not.toContain('synthetic failure');
  });

  it.each(['zh-CN', 'en-US'] as const)('clears the page and stops the model read immediately after revocation in %s', async (language) => {
    await i18n.changeLanguage(language);
    stubModel(myWorkInput());
    const { queryClient } = renderWorkbench();
    expect(await screen.findByTestId('workbench-frame')).toBeTruthy();
    const callsBefore = vi.mocked(fetch).mock.calls.length;
    revokeObservationReads(queryClient);
    expect(await screen.findByTestId('state-forbidden')).toBeTruthy();
    expect(screen.queryByTestId('workbench-frame')).toBeNull();
    expect(document.body.textContent).not.toContain('Synthetic mine-running');
    expect(vi.mocked(fetch).mock.calls.length).toBe(callsBefore);
  });

  it.each(['zh-CN', 'en-US'] as const)('renders forbidden access without leaking the old model in %s', async (language) => {
    await i18n.changeLanguage(language);
    stubFetch({
      '/api/team/overview-model': { ok: false, error: { code: 'Forbidden', message: 'restricted synthetic scope' } },
      '/api/team/projects': { ok: true, projects: [PROJECT] },
    });
    renderWorkbench();
    expect(await screen.findByTestId('state-forbidden')).toBeTruthy();
    expect(screen.queryByTestId('workbench-frame')).toBeNull();
    expect(document.body.textContent).not.toContain('restricted synthetic scope');
  });
});

describe('project scope selection', () => {
  it('uses the only authorized project when all scope is selected', async () => {
    stubModel(myWorkInput());
    renderWorkbench();
    await screen.findByTestId('workbench-frame');
    const modelCall = vi.mocked(fetch).mock.calls.find(([input]) => String(input).includes('/api/team/overview-model'));
    expect(String(modelCall?.[0])).toContain('project=demo');
  });

  it('asks for one project instead of fanning out across several projects', async () => {
    stubFetch({
      '/api/team/projects': { ok: true, projects: [PROJECT, { key: 'second', title: 'Second Synthetic Project' }] },
    });
    renderWorkbench();
    expect(await screen.findByTestId('workbench-select-project')).toBeTruthy();
    expect(screen.getByRole('link', { name: '去选择项目' })).toBeTruthy();
    expect(vi.mocked(fetch).mock.calls.some(([input]) => String(input).includes('/api/team/overview-model'))).toBe(false);
  });

});

describe('member identity from the composition only', () => {
  function expectNoWorkNextRead() {
    const workNextCalls = vi.mocked(fetch).mock.calls.filter(([input]) => String(input).includes('/api/team/read'));
    expect(workNextCalls).toEqual([]);
  }

  it('uses a supplied id that matches a member and never issues a second identity read', async () => {
    stubModel(myWorkInput());
    const { container } = renderWorkbench();
    expect(await screen.findByTestId('workbench-frame')).toBeTruthy();
    expect(screen.queryByText('当前成员还不能确定')).toBeNull();
    expect(container.querySelectorAll('.obs-chain-node-root').length).toBeGreaterThan(0);
    expectNoWorkNextRead();
  });

  it('keeps a supplied id that matches no member unknown and draws no chains', async () => {
    const base = myWorkInput();
    const input = {
      ...base,
      options: {
        ...base.options,
        views: { ...base.options.views, me: { personId: 'person-missing', projectId: 'project-one' } },
      },
    };
    stubModel(input);
    const { container } = renderWorkbench();
    expect(await screen.findByTestId('workbench-frame')).toBeTruthy();
    expect(screen.getByText('当前成员还不能确定')).toBeTruthy();
    expect(container.querySelectorAll('.obs-chain-node').length).toBe(0);
    expectNoWorkNextRead();
  });

  it('keeps an absent id unknown and draws no chains or identity fallback', async () => {
    const base = myWorkInput();
    const input = {
      ...base,
      options: {
        ...base.options,
        views: { ...base.options.views, me: { personId: null, projectId: 'project-one' } },
      },
    };
    stubModel(input);
    const { container } = renderWorkbench();
    expect(await screen.findByTestId('workbench-frame')).toBeTruthy();
    expect(screen.getByText('当前成员还不能确定')).toBeTruthy();
    expect(container.querySelectorAll('.obs-chain-node').length).toBe(0);
    expectNoWorkNextRead();
  });
});
