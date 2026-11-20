import { QueryClientProvider } from '@tanstack/react-query';
import { render, screen, waitFor } from '@testing-library/react';
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
import { myWorkInput } from '../components/workbench/testing';
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

  it('uses a member only when the existing identity read establishes that member', async () => {
    const input = myWorkInput({
      options: { nowUnixMs: myWorkInput().options.nowUnixMs, stalledAfterDays: 1 },
    });
    stubModel(input);
    renderWorkbench();
    await waitFor(() => expect(screen.getByTestId('workbench-loading')).toBeTruthy());
    expect(document.body.textContent).not.toContain('Owner One');
  });
});
