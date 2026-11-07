import { act, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { axe, toHaveNoViolations } from 'jest-axe';
import { afterEach, describe, expect, it, vi } from 'vitest';
import i18n from './i18n';
import { formatUnixMs } from './lib/format';
import { defaultVisibleText, findForbiddenVisibleText } from './lib/human-language-guard';
import { isObservationReadRevoked } from './lib/queries';
import type { TeamWork } from './lib/types';
import {
  DEMO_ACTIVITY,
  DEMO_PROJECT_OVERVIEW,
  DEMO_PROJECT_WORKS,
  DEMO_PROJECTS,
  DEMO_REVIEW,
  demoWorkSnapshot,
  renderApp,
} from './test-utils';

expect.extend(toHaveNoViolations);

/** A checkpoint in the shape the service returns, with plain sentences so the default text can be checked. */
const CHECKPOINT = {
  phase: 'ready_for_review',
  summary: 'The status and keyword filters are finished and waiting for review.',
  completed: ['Filters combine status and keyword.', 'Checks cover an empty list.'],
  blockers: [],
  tests: [{ name: 'npm test', outcome: 'passed', reference: 'node --test: 5 passed' }, { name: 'smoke run', outcome: 'failed', reference: 'exit 1' }],
  artifacts: [{ label: 'development branch', reference: 'refs/heads/issue-filter-api@abc123' }],
  next_action: 'Wait for the independent review.',
  provenance: 'caller_declared',
  stale: false,
  contract_matches_current: true,
  checkpoint_id: 'checkpoint-1',
  reported_at_unix_ms: 1761839700000,
  origin: { actor_name: 'Lin Agent' },
};

function json(payload: unknown) {
  return new Response(JSON.stringify(payload), {
    status: 200,
    headers: { 'content-type': 'application/json' },
  });
}

function makeTwoStreamWorks(count: number) {
  return Array.from({ length: count }, (_, index) => ({
    ...DEMO_PROJECT_WORKS[0],
    key: `W${index + 1}`,
    title: `Observed plan task ${index + 1}`,
    workstream_id: index % 2 === 0 ? 'WS1' : 'WS2',
    contract_hash: `contract-${index + 1}`,
  }));
}

function makePlanOverview(works: TeamWork[]) {
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
  return { ...DEMO_ACTIVITY, data: { ...DEMO_ACTIVITY.data, ...overrides, items } };
}

function setupPlan({
  path = '/projects/demo?tab=plan&work=W1',
  overview = DEMO_PROJECT_OVERVIEW,
  activity = DEMO_ACTIVITY,
  review = DEMO_REVIEW,
  snapshot = demoWorkSnapshot,
}: {
  path?: string;
  overview?: unknown;
  activity?: unknown;
  review?: unknown;
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
      if (request.query.op === 'review.inspect') return json(review);
    }
    return json({ ok: false, error: { code: 'NoRoute', message: url } });
  });
  vi.stubGlobal('fetch', fetchMock);
  return { fetchMock, ...renderApp(path) };
}

afterEach(async () => {
  vi.unstubAllGlobals();
  await i18n.changeLanguage('zh-CN');
});

describe('effective plan table', () => {
  it('says how far each work has come from its own read, in the list and in the detail header, with the same fallbacks as the overview', async () => {
    const works = [
      DEMO_PROJECT_WORKS[0],
      DEMO_PROJECT_WORKS[1],
      { ...DEMO_PROJECT_WORKS[1], key: 'W3', title: 'Third task', status: 'open' },
      { ...DEMO_PROJECT_WORKS[1], key: 'W4', title: 'Fourth task', status: null },
    ];
    setupPlan({
      path: '/projects/demo?tab=plan&work=W1',
      overview: makePlanOverview(works),
      snapshot: (workId) => {
        if (workId === 'W3' || workId === 'W4') return { ok: false, error: { code: 'ReadTimeout', message: 'slow' } };
        const response = demoWorkSnapshot(workId) as { result: { data: { observation: { progress: Record<string, unknown> | null } } } };
        response.result.data.observation.progress = workId === 'W1' ? { phase: 'ready_for_review' } : null;
        return response;
      },
    });
    await waitFor(() => {
      expect(screen.getByTestId('plan-status-W1').textContent).toBe('待评审');
      expect(screen.getByTestId('plan-status-W2').textContent).toBe('已认领');
      expect(screen.getByTestId('plan-status-W3').textContent).toBe('进行中');
      expect(screen.getByTestId('plan-status-W4').textContent).toBe('未知');
    });
    expect(screen.getByTestId('plan-detail-status').textContent).toBe('待评审');
    expect(screen.getByTestId('plan-status-W1').querySelector('.ant-badge-status-dot')?.className).toContain('ant-badge-status-warning');
    expect(screen.getByTestId('plan-status-W4').querySelector('.ant-badge-status-dot')?.className).toContain('ant-badge-status-default');
  });

  it('says loading in the list while a work\'s own read is pending', async () => {
    let release: () => void = () => undefined;
    const gate = new Promise<void>((resolve) => { release = resolve; });
    const original = demoWorkSnapshot;
    const { fetchMock } = setupPlan({ path: '/projects/demo?tab=plan', snapshot: (workId) => original(workId) });
    const impl = fetchMock.getMockImplementation()!;
    fetchMock.mockImplementation(async (input, init) => {
      if (String(init?.body ?? '').includes('"work.snapshot"')) await gate;
      return impl(input, init);
    });
    await waitFor(() => expect(screen.getByTestId('plan-status-W1').textContent).toBe('加载中…'));
    expect(screen.getByTestId('plan-owner-W1').textContent).toBe('加载中…');
    await act(async () => { release(); });
    await waitFor(() => expect(screen.getByTestId('plan-status-W1').textContent).toBe('已认领'));
    expect(screen.getByTestId('plan-owner-W1').textContent).toBe('lin');
  });

  it('refreshes the progress words of the list slowly, ten minutes, and the selected work\'s own detail fast', async () => {
    const { queryClient } = setupPlan({ path: '/projects/demo?tab=plan' });
    await waitFor(() => expect(screen.getByTestId('plan-status-W2').textContent).toBe('已认领'));
    // every observer of every listed work's read, by work: the cadence each observer asks for
    const cadences = () => Object.fromEntries(queryClient.getQueryCache().getAll()
      .filter((query) => query.queryKey[0] === 'team' && query.queryKey[1] === 'work.snapshot' && query.queryKey[2] === 'demo')
      .map((query) => [String(query.queryKey[3]), query.observers.map((observer) => observer.options.refetchInterval).sort()]));
    expect(cadences()).toEqual({ W1: [600000], W2: [600000] });
    // selecting a work adds the detail's own fast observer for that work only
    await userEvent.setup().click(screen.getByRole('link', { name: 'Build the filter API' }));
    await screen.findByTestId('plan-detail');
    await waitFor(() => expect(cadences()).toEqual({ W1: [5000, 600000], W2: [600000] }));
  });

  it('reads snapshots only for the works on the visible page', async () => {
    const { fetchMock } = setupPlan({ path: '/projects/demo?tab=plan', overview: makePlanOverview(makeTwoStreamWorks(61)) });
    await waitFor(() => expect(screen.getAllByRole('link', { name: /Observed plan task/ })).toHaveLength(60));
    const reads = () => fetchMock.mock.calls
      .filter(([, init]) => String(init?.body ?? '').includes('"work.snapshot"'))
      .map(([, init]) => (JSON.parse(String(init?.body)) as { query: { work_id: string } }).query.work_id);
    await waitFor(() => expect(new Set(reads()).size).toBe(60));
    expect(reads()).toHaveLength(60);
    expect(reads()).not.toContain('W60');
  }, 20_000);

  it('reads each listed work once, shared with the selected work\'s detail', async () => {
    const { fetchMock } = setupPlan();
    await waitFor(() => expect(screen.getByTestId('plan-status-W2').textContent).toBe('已认领'));
    const snapshotReads = fetchMock.mock.calls.filter(([, init]) => String(init?.body ?? '').includes('"work.snapshot"'));
    expect(snapshotReads.map(([, init]) => (JSON.parse(String(init?.body)) as { query: { work_id: string } }).query.work_id).sort()).toEqual(['W1', 'W2']);
  });

  it('renders real groups and fields, filters by workstream, and disables board view', async () => {
    const { router } = setupPlan({ path: '/projects/demo?tab=plan' });
    expect(await screen.findByRole('link', { name: 'Build the filter API' })).toBeTruthy();
    expect(screen.getByTestId('workstream-group-WS1').textContent).toContain('Backend workstream');
    expect(screen.getByTestId('workstream-group-WS2').textContent).toContain('Frontend workstream');
    await waitFor(() => expect(screen.getByTestId('plan-status-W1').textContent).toBe('已认领'));
    expect(screen.getByTestId('plan-owner-W1').textContent).toBe('lin');
    expect(screen.getByRole('radio', { name: '看板' })).toHaveProperty('disabled', true);
    expect(screen.queryByRole('button', { name: '提出需求' })).toBeNull();

    expect(screen.getByRole('combobox', { name: '工作线筛选' })).toBeTruthy();
    await act(() => router.navigate('/projects/demo?tab=plan&stream=WS1'));
    expect(screen.getByRole('link', { name: 'Build the filter API' })).toBeTruthy();
    await waitFor(() => expect(screen.queryByRole('link', { name: 'Prepare the support page' })).toBeNull());
  });

  it.each([59, 60])('renders every work on page one without consuming group rows: %i', async (count) => {
    setupPlan({ path: '/projects/demo?tab=plan', overview: makePlanOverview(makeTwoStreamWorks(count)) });
    await waitFor(() => expect(screen.getAllByRole('link', { name: /Observed plan task/ })).toHaveLength(count));
    expect(screen.getByRole('link', { name: `Observed plan task ${count}` })).toBeTruthy();
    expect(document.querySelector('[data-testid="plan-progress-table"] .ant-pagination')).toBeNull();
    expect(screen.getByTestId('plan-pagination')).toBeTruthy();
  }, 20_000);

  it('renders the correct sixty works on page two across two workstreams', async () => {
    setupPlan({
      path: '/projects/demo?tab=plan&page=2',
      overview: makePlanOverview(makeTwoStreamWorks(140)),
    });
    await waitFor(() => expect(screen.getAllByRole('link', { name: /Observed plan task/ })).toHaveLength(60));
    expect(screen.getByRole('link', { name: 'Observed plan task 121' })).toBeTruthy();
    expect(screen.getByRole('link', { name: 'Observed plan task 139' })).toBeTruthy();
    expect(screen.getByRole('link', { name: 'Observed plan task 2' })).toBeTruthy();
    expect(screen.getByRole('link', { name: 'Observed plan task 100' })).toBeTruthy();
    expect(screen.queryByRole('link', { name: 'Observed plan task 120' })).toBeNull();
    expect(screen.queryByRole('link', { name: 'Observed plan task 102' })).toBeNull();
  }, 20_000);

  it('clamps an out-of-range page and keeps standalone pagination visible', async () => {
    setupPlan({ path: '/projects/demo?tab=plan&page=5' });
    expect(await screen.findByTestId('plan-progress-table')).toBeTruthy();
    expect(screen.getByRole('link', { name: 'Build the filter API' })).toBeTruthy();
    expect(screen.getByRole('link', { name: 'Prepare the support page' })).toBeTruthy();
    expect(screen.getByTestId('plan-pagination')).toBeTruthy();
    expect(document.querySelector('.ant-pagination-item-active')?.textContent).toBe('1');
  });

  it('selects work through the URL and restores the detail after navigation', async () => {
    const user = userEvent.setup();
    const { router } = setupPlan({ path: '/projects/demo?tab=plan&work=W1' });
    await screen.findByTestId('plan-detail');
    await waitFor(() => expect(screen.getByTestId('plan-detail').textContent).toContain('Build the filter API'));
    await user.click(screen.getByRole('link', { name: 'Prepare the support page' }));
    expect(router.state.location.search).toContain('tab=plan');
    expect(router.state.location.search).toContain('work=W2');
    expect((await screen.findByTestId('plan-detail')).textContent).toContain('Prepare the support page');
  });
});

describe('work detail requirements and source state', () => {
  it('renders real contract, required specs, runtime, a plain next step with the agent guidance kept in the technical details, and the report time', async () => {
    const longNote = 'Batch progress with next_action/open_loops. Stop and report workspace execution before publication. '.repeat(4);
    setupPlan({
      snapshot: (workId) => {
        const response = demoWorkSnapshot(workId);
        response.result.data.guidance.action.note = longNote;
        return response;
      },
    });
    await screen.findByTestId('plan-detail');
    await waitFor(() => expect(screen.getByTestId('plan-detail').textContent).toContain('Keep support filters useful'));
    expect(screen.getByTestId('plan-detail').textContent).toContain('Do not mutate input records');
    expect(screen.getByTestId('required-spec-name').textContent).toBe('support');
    expect(defaultVisibleText(screen.getByTestId('plan-detail'))).not.toContain('docs/support.md');
    expect(screen.getByTestId('plan-detail').textContent).toContain('Keep the support API stable.');
    expect(screen.getByTestId('plan-runtime-state').textContent).toBe('已认领');
    expect(screen.getByTestId('plan-guidance').textContent).toBe('未报告下一步');
    expect(screen.getByTestId('plan-detail').textContent).not.toContain('session.checkpoint');
    expect(screen.getByTestId('plan-detail').textContent).not.toContain('Batch progress');
    expect(screen.getByTestId('last-report-W1').textContent).toBe(formatUnixMs(1761839800000, 'zh-CN'));
    expect(screen.getByTestId('last-report-W1').textContent).not.toBe(formatUnixMs(1761839900000, 'zh-CN'));
    // The service's own guidance is addressed to the agent: verbatim, but only behind the technical details.
    const user = userEvent.setup();
    await user.click(within(screen.getByTestId('plan-detail')).getAllByText('技术细节')[0]);
    expect((await screen.findByTestId('technical-guidance-op')).textContent).toContain('session.checkpoint');
    expect(screen.getByTestId('technical-guidance-note').textContent).toContain(longNote);
    expect(screen.getByTestId('technical-next-step').textContent).toContain('未提供');
    // the path of the required source, with its digest and size, is behind its own technical details
    await user.click(within(screen.getByTestId('plan-detail')).getAllByText('技术细节')[1]);
    expect((await screen.findByTestId('technical-detail-required_specs[0].path')).textContent).toContain('docs/support.md');
    expect(screen.getByTestId('technical-detail-required_specs[0].sha256')).toBeTruthy();
    expect(screen.getByTestId('technical-detail-required_specs[0].byte_length')).toBeTruthy();
  });

  it('shows the reporter\'s own next action in the detail, keeps the raw value in the technical details, and marks a report the service declared stale', async () => {
    setupPlan({
      snapshot: (workId) => {
        const response = demoWorkSnapshot(workId) as {
          result: { data: { observation: { progress: Record<string, unknown> | null; reporting: Record<string, unknown> } } };
        };
        response.result.data.observation.progress = { next_action: 'Review the contract first' };
        response.result.data.observation.reporting.client_stale = true;
        return response;
      },
    });
    await waitFor(() => expect(screen.getByTestId('plan-guidance').textContent).toBe('Review the contract first'));
    expect(screen.getByTestId('last-report-W1').textContent).toBe(`${formatUnixMs(1761839800000, 'zh-CN')}可能已过期`);
    await userEvent.setup().click(within(screen.getByTestId('plan-detail')).getAllByText('技术细节')[0]);
    expect((await screen.findByTestId('technical-next-action')).textContent).toContain('Review the contract first');
    // why the note is (not) there stays checkable: the service's flag and the work's own state, verbatim
    expect(screen.getByTestId('technical-client-stale').textContent).toContain('true');
    expect(screen.getByTestId('technical-runtime-state').textContent).toContain('claimed');
    expect(screen.getByTestId('technical-completion-id').textContent).toContain('未提供');
    expect(screen.getByTestId('technical-observed-at').textContent).toContain('1761839999000');
  });

  it('shows the checkpoint in plain words instead of a JSON dump and keeps every raw field in the technical details', async () => {
    setupPlan({
      snapshot: (workId) => {
        const response = demoWorkSnapshot(workId) as { result: { data: { observation: { progress: Record<string, unknown> | null } } } };
        response.result.data.observation.progress = { ...CHECKPOINT };
        return response;
      },
    });
    const block = await screen.findByTestId('progress-report');
    expect(within(block).getByTestId('progress-report-summary').textContent).toBe(CHECKPOINT.summary);
    expect(within(block).getByTestId('progress-report-completed').textContent).toContain('Checks cover an empty list.');
    expect(within(block).getByTestId('progress-report-blockers').textContent).toBe('未报告阻塞');
    expect(within(block).getByTestId('progress-report-tests').textContent).toContain('npm test');
    expect(within(block).getByTestId('progress-report-tests').textContent).toContain('已通过');
    expect(within(block).getByTestId('progress-report-tests').textContent).toContain('失败');
    expect(within(block).getByTestId('progress-report-artifacts').textContent).toContain('开发分支');
    expect(within(block).getByTestId('progress-report-declaration').textContent).toContain('Lin Agent');
    expect(within(block).getByTestId('progress-report-declaration').textContent).toContain('系统未核验');
    expect(block.textContent).not.toContain('{');
    expect(block.textContent).not.toContain('refs/heads');
    expect(findForbiddenVisibleText(defaultVisibleText(block))).toEqual([]);
    expect(within(block).queryByTestId('progress-report-contract')).toBeNull();
    const user = userEvent.setup();
    await user.click(within(block).getByText('技术细节'));
    expect((await within(block).findByTestId('technical-detail-progress.checkpoint_id')).textContent).toContain('checkpoint-1');
    expect(within(block).getByTestId('technical-detail-progress.artifacts').textContent).toContain('refs/heads/issue-filter-api@abc123');
  });

  it('wires the work being finished into the checkpoint: a stale checkpoint is marked on an open work and not on a finished one', async () => {
    const { unmount } = setupPlan({
      snapshot: (workId) => {
        const response = demoWorkSnapshot(workId) as { result: { data: { observation: { progress: Record<string, unknown> | null } } } };
        response.result.data.observation.progress = { ...CHECKPOINT, stale: true };
        return response;
      },
    });
    expect((await screen.findByTestId('progress-report-declaration')).textContent).toContain('可能已过期');
    unmount();
    setupPlan({
      snapshot: (workId) => {
        const response = demoWorkSnapshot(workId) as { result: { data: { runtime: { state: string }; observation: { progress: Record<string, unknown> | null } } } };
        response.result.data.runtime.state = 'completed';
        response.result.data.observation.progress = { ...CHECKPOINT, stale: true };
        return response;
      },
    });
    expect((await screen.findByTestId('progress-report-declaration')).textContent).not.toContain('可能已过期');
  });

  it('cuts the long ids of the reporter\'s sentences in the checkpoint block and keeps them as written in the technical details', async () => {
    const hash = '5f7eb2fe9c563e4d355aa64a527e0aef754116c1';
    setupPlan({
      snapshot: (workId) => {
        const response = demoWorkSnapshot(workId) as { result: { data: { observation: { progress: Record<string, unknown> | null } } } };
        response.result.data.observation.progress = { ...CHECKPOINT, summary: `Merged ${hash} into main.`, completed: [`Published ${hash}.`] };
        return response;
      },
    });
    const block = await screen.findByTestId('progress-report');
    expect(within(block).getByTestId('progress-report-summary').textContent).toBe('Merged 5f7eb2fe… into main.');
    expect(within(block).getByTestId('progress-report-completed').textContent).toContain('Published 5f7eb2fe….');
    expect(block.textContent).not.toContain(hash);
    await userEvent.setup().click(within(block).getByText('技术细节'));
    expect((await within(block).findByTestId('technical-detail-progress.summary')).textContent).toContain(hash);
  });

  it('warns on the checkpoint when the task changed after it was written', async () => {
    setupPlan({
      snapshot: (workId) => {
        const response = demoWorkSnapshot(workId) as { result: { data: { observation: { progress: Record<string, unknown> | null } } } };
        response.result.data.observation.progress = { ...CHECKPOINT, contract_matches_current: false };
        return response;
      },
    });
    const note = await screen.findByTestId('progress-report-contract');
    expect(note.textContent).toBe('这份汇报写成之后，任务要求有变化，内容可能已不适用');
  });

  it('constrains long required-spec text with ellipsis and a zero-min-width flex child', async () => {
    const longSpec = 'Required source text with a very long immutable locator and validation payload. '.repeat(8);
    setupPlan({
      snapshot: (workId) => {
        const response = demoWorkSnapshot(workId);
        response.result.data.required_specs[0].text = longSpec;
        return response;
      },
    });
    const text = await screen.findByTestId('required-spec-text');
    expect(text.className).toContain('ant-typography-ellipsis');
    expect(text.parentElement?.style.minWidth).toBe('0px');
  });

  it('keeps the source facts and an honest not-measured takeover note behind the technical details only', async () => {
    setupPlan();
    const card = await screen.findByTestId('source-state-card');
    // collapsed by default: a person sees one words-only entry, no ids, no versions
    expect(card.textContent).toBe('技术细节');
    await userEvent.setup().click(within(card).getByText('技术细节'));
    expect((await within(card).findByTestId('source-snapshot-id')).textContent).toContain('snapshot-1');
    expect(card.textContent).toContain('epoch-1');
    expect(card.textContent).toContain('contract-1');
    expect(card.textContent).toContain('enabled');
    expect(card.textContent).toContain('project_revision');
    expect(card.textContent).toContain('authority_version');
    expect(card.textContent).toContain('selection_basis');
    expect(card.textContent).toContain('尚未读取');
    expect(card.textContent).not.toContain('已同步');
    expect(card.textContent).not.toContain('takeover complete');
  });

  it('marks a missing attribution triple as not provided, never synchronized', async () => {
    setupPlan({
      snapshot: (workId) => {
        const response = demoWorkSnapshot(workId) as {
          result: {
            source_snapshot_id?: string;
            coordinator_epoch?: string;
            project_revision?: string;
          };
        };
        delete response.result.source_snapshot_id;
        delete response.result.coordinator_epoch;
        delete response.result.project_revision;
        return response;
      },
    });
    const card = await screen.findByTestId('source-state-card');
    await userEvent.setup().click(within(card).getByText('技术细节'));
    expect((await within(card).findByTestId('source-snapshot-id')).textContent).toContain('未提供');
    expect(within(card).getByTestId('source-project-revision').textContent).toContain('未提供');
    expect(within(card).getByTestId('source-coordinator-epoch').textContent).toContain('未提供');
    expect(card.textContent).not.toContain('已同步');
  });

  it('keeps candidate and source-set blocks as later-slice placeholders', async () => {
    setupPlan();
    expect((await screen.findByTestId('plan-candidates-placeholder')).textContent).toContain('尚未开放');
    expect((await screen.findByTestId('plan-source-set-placeholder')).textContent).toContain('尚未开放');
  });
});

describe('work detail review outcomes', () => {
  it('renders localized review decisions with exact metadata and no narrative', async () => {
    const user = userEvent.setup();
    setupPlan({ path: '/projects/demo?tab=plan&work=W1&review_round_id=round-1' });
    await user.click(await screen.findByRole('tab', { name: '成果' }));
    const review = await screen.findByTestId('plan-review');
    const decision = within(review).getByTestId('review-decision-decision-1');
    expect(within(decision).getByTestId('review-decision-code').textContent).toBe('批准');
    expect(decision.textContent).toContain('成员独立复核（模拟）');
    expect(decision.textContent).toContain('人工批准');
    expect(review.textContent).toContain('author-agent-1');
    expect(review.textContent).toContain('artifact-digest-1');
    expect(review.textContent).toContain('bundle-hash-1');
    expect(review.querySelector('[data-testid="review-narrative"]')).toBeNull();
    expect(review.textContent).toContain('尚未开放');
  });

  it('presents a missing review as absent rather than an error', async () => {
    const user = userEvent.setup();
    setupPlan({
      path: '/projects/demo?tab=plan&work=W1&review_round_id=round-1',
      review: {
        ...DEMO_REVIEW,
        result: { ...DEMO_REVIEW.result, data: { review: null } },
      },
    });
    await user.click(await screen.findByRole('tab', { name: '成果' }));
    expect(await screen.findByTestId('review-absent')).toBeTruthy();
    expect(screen.queryByTestId('state-partial')).toBeNull();
  });

  it('does not request review.inspect without a verified review round id', async () => {
    const user = userEvent.setup();
    const { fetchMock } = setupPlan();
    await user.click(await screen.findByRole('tab', { name: '成果' }));
    expect((await screen.findByTestId('review-round-unavailable')).textContent)
      .toContain('暂时无法读取这次检查');
    const reviewCalls = fetchMock.mock.calls.filter(([, init]) => String(init?.body ?? '').includes('"review.inspect"'));
    expect(reviewCalls).toHaveLength(0);
  });
});

describe('work detail history', () => {
  it('filters development history by work_id and shows it in plain words with the raw records in the technical details', async () => {
    const user = userEvent.setup();
    const { fetchMock } = setupPlan();
    await user.click(await screen.findByRole('tab', { name: '历史' }));
    const history = await screen.findByTestId('plan-history');
    const step = await within(history).findByTestId('recent-step-0');
    expect(within(step).getByTestId('recent-step-text').textContent).toBe('变更授权');
    expect(within(step).getByTestId('recent-step-time').textContent).toBe(formatUnixMs(1761839700000, 'zh-CN'));
    for (const hidden of ['access.changed', 'actor-raw-17', 'audit.development', 'not_observed']) {
      expect(defaultVisibleText(history)).not.toContain(hidden);
    }
    expect(within(history).getByTestId('recent-changes-all').getAttribute('href')).toBe('/activity?project=demo&work=W1');
    for (const toggle of within(history).getAllByRole('button', { name: /技术细节/ })) await user.click(toggle);
    expect((await within(history).findByTestId('recent-step-technical-0')).textContent).toContain('access.changed');
    expect(within(history).getByTestId('technical-detail-development_source').textContent).toContain('audit.development');
    expect(within(history).getByTestId('technical-detail-unknown_means').textContent).toContain('not_observed');
    const urls = fetchMock.mock.calls.map(([input]) => String(input))
      .filter((url) => url.includes('/api/team/activity'));
    expect(urls).toHaveLength(1);
    expect(urls[0]).toContain('kind=development');
    expect(urls[0]).toContain('work_id=W1');
    expect(urls[0]).not.toContain('kind=requests');
  });

  it('uses empty for a successful empty history read', async () => {
    const user = userEvent.setup();
    setupPlan({ activity: makeActivity([]) });
    await user.click(await screen.findByRole('tab', { name: '历史' }));
    const history = await screen.findByTestId('plan-history');
    expect(await within(history).findByTestId('state-empty')).toBeTruthy();
  });

  it('presents Unsupported as unavailable in this mode', async () => {
    const user = userEvent.setup();
    setupPlan({ activity: { ok: false, error: { code: 'Unsupported', message: 'demo' } } });
    await user.click(await screen.findByRole('tab', { name: '历史' }));
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 1200));
    });
    expect(screen.getByTestId('activity-unavailable').textContent).toContain('此模式不可用');
  });

  it('presents history failure as partial failure', async () => {
    const user = userEvent.setup();
    setupPlan({ activity: { ok: false, error: { code: 'ReadTimeout', message: 'slow' } } });
    await user.click(await screen.findByRole('tab', { name: '历史' }));
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 1200));
    });
    const history = screen.getByTestId('plan-history');
    expect(history.textContent).toContain('部分读取失败');
    expect(history.textContent).not.toContain('ReadTimeout');
    await user.click(within(history).getByText('技术细节'));
    expect(await within(history).findByText('ReadTimeout')).toBeTruthy();
  });

  it('discloses earlier records and caps the newest history at ten steps', async () => {
    const user = userEvent.setup();
    const items = Array.from({ length: 22 }, (_, index) => ({
      ...DEMO_ACTIVITY.data.items[0],
      id: `activity-${index + 1}`,
      action: index % 2 === 0 ? 'work.snapshot' : 'execution.start',
      created_at_unix_ms: 1761839700000 + index * 1000,
    }));
    setupPlan({ activity: makeActivity(items, { next_cursor: 'older-2' }) });
    await user.click(await screen.findByRole('tab', { name: '历史' }));
    const history = await screen.findByTestId('plan-history');
    await waitFor(() => expect(history.querySelectorAll('li[data-testid^="recent-step-"]')).toHaveLength(10));
    expect(within(history.querySelector('li[data-testid="recent-step-0"]') as HTMLElement).getByTestId('recent-step-text').textContent).toBe('开始执行');
    expect(within(history).getByTestId('recent-changes-more')).toBeTruthy();
    expect(history.textContent).toContain('还有更早的记录');
  });

});

describe('work plan localization and accessibility', () => {
  it('renders new plan strings in English', async () => {
    setupPlan();
    await screen.findByTestId('plan-detail');
    await act(() => i18n.changeLanguage('en-US'));
    expect(await screen.findByText('Effective plan')).toBeTruthy();
    expect(screen.getByRole('tab', { name: 'Requirements & progress' })).toBeTruthy();
    expect(screen.getByRole('tab', { name: 'Outcomes' })).toBeTruthy();
    expect(screen.getByRole('tab', { name: 'History' })).toBeTruthy();
    expect(within(screen.getByTestId('source-state-card')).getByText('Technical details')).toBeTruthy();
    expect(screen.queryByText('Source state')).toBeNull();
  });

  it('has no axe violations', async () => {
    const { container } = setupPlan();
    await screen.findByTestId('plan-detail');
    const results = await axe(container, {
      rules: { 'color-contrast': { enabled: false } },
    });
    expect(results).toHaveNoViolations();
  });
});

describe('work-plan review round 2: sub-read failures and behavior pins', () => {
  it('a Forbidden review read degrades only its own tab, not the whole panel', async () => {
    setupPlan({
      path: '/projects/demo?tab=plan&work=W1&review_round_id=round-1',
      review: { ok: false, error: { code: 'Forbidden', message: 'access denied' } },
    });
    // The work itself is readable: requirements stay visible with real content.
    expect(await screen.findByTestId('plan-runtime-state')).toBeTruthy();
    expect(screen.queryByTestId('state-forbidden')).toBeNull();
    const user = userEvent.setup();
    await user.click(screen.getByRole('tab', { name: '成果' }));
    const partial = await screen.findByTestId('state-partial');
    expect(partial.textContent).not.toContain('Forbidden');
    await user.click(within(partial).getByText('技术细节'));
    expect(await within(partial).findByText('Forbidden')).toBeTruthy();
  });

  it('a missing client_reported_at reads 尚未读取, never the current clock', async () => {
    setupPlan({
      snapshot: (workId) => {
        const response = demoWorkSnapshot(workId) as {
          result: { data: { observation: { reporting: { client_reported_at_unix_ms?: number } } } };
        };
        delete response.result.data.observation.reporting.client_reported_at_unix_ms;
        return response;
      },
    });
    const cell = await screen.findByTestId('last-report-W1');
    await waitFor(() => expect(cell.textContent).toBe('尚未读取'));
    expect(cell.textContent).not.toBe(formatUnixMs(Date.now(), 'zh-CN'));
  });

  it('a work in a workstream the overview does not list shows under its own id', async () => {
    const works = [
      DEMO_PROJECT_WORKS[0],
      { ...DEMO_PROJECT_WORKS[1], key: 'W-orphan', title: 'Orphan task', workstream_id: 'WS-unlisted' },
    ];
    setupPlan({ overview: makePlanOverview(works) });
    expect(await screen.findByTestId('workstream-group-WS-unlisted')).toBeTruthy();
    expect(screen.getByRole('link', { name: 'Orphan task' })).toBeTruthy();
  });

  it('a failed scope with zero enumerated is not the empty state', async () => {
    const overview = {
      ...DEMO_PROJECT_OVERVIEW,
      works: [],
      coverage: {
        ...DEMO_PROJECT_OVERVIEW.coverage,
        scope: { ...DEMO_PROJECT_OVERVIEW.coverage.scope, status: 'failed', enumerated: 0, error_code: 'DatabaseUnavailable' },
        failed: 1,
      },
    };
    setupPlan({ overview });
    await waitFor(() => expect(screen.getByTestId('state-partial')).toBeTruthy());
    expect(screen.queryByTestId('state-empty')).toBeNull();
  });

  it('a failed work.snapshot renders a partial failure, not a blank pane', async () => {
    setupPlan({ snapshot: () => ({ ok: false, error: { code: 'ReadTimeout', message: 'slow' } }) });
    const partial = await screen.findByTestId('state-partial', undefined, { timeout: 5000 });
    expect(partial.textContent).not.toContain('ReadTimeout');
    await userEvent.setup().click(within(partial).getByText('技术细节'));
    expect(await within(partial).findByText('ReadTimeout')).toBeTruthy();
  });

  it('changing the page clears the selected work', async () => {
    const works = makeTwoStreamWorks(61);
    const { router } = setupPlan({ overview: makePlanOverview(works), path: '/projects/demo?tab=plan&work=W1' });
    await screen.findByRole('link', { name: 'Observed plan task 1' });
    const user = userEvent.setup();
    await user.click(screen.getByRole('listitem', { name: '2' }));
    await waitFor(() => expect(router.state.location.search).not.toContain('work='));
    expect(router.state.location.search).toContain('page=2');
  });

  it('Enter and Space select a row', async () => {
    const { router } = setupPlan({ path: '/projects/demo?tab=plan' });
    const rowLink = await screen.findByRole('link', { name: 'Build the filter API' });
    const user = userEvent.setup();
    const row = rowLink.closest('tr')!;
    await user.click(row);
    await waitFor(() => expect(router.state.location.search).toContain('work=W1'));
    row.focus();
    await user.keyboard('[Enter]');
    await waitFor(() => expect(router.state.location.search).toContain('work=W1'));
    await user.keyboard('[Space]');
    await waitFor(() => expect(router.state.location.search).toContain('work=W1'));
  });

  it('the owner cell prefers owner_person over agent when both are set', async () => {
    const works = [{ ...DEMO_PROJECT_WORKS[0], owner_person: 'lin', agent: { id: 'agent-x', state: 'running' } }];
    setupPlan({ overview: makePlanOverview(works) });
    const cell = await screen.findByTestId('plan-owner-W1');
    expect(cell.textContent).toBe('lin');
    expect(cell.textContent).not.toContain('agent-x');
  });

  it('shows each responsible member from the current work snapshot without extra reads', async () => {
    const { fetchMock } = setupPlan({
      path: '/projects/demo?tab=plan',
      snapshot: (workId) => {
        const response = demoWorkSnapshot(workId);
        return {
          ...response,
          result: {
            ...response.result,
            data: {
              ...response.result.data,
              observation: {
                ...response.result.data.observation,
                progress: CHECKPOINT,
                responsibility: {
                  relation: 'owned_by_other',
                  owner: `member_${workId.toLowerCase()}`,
                  owner_name: workId === 'W1' ? 'Lin Wei' : 'Mei Chen',
                  version: '1',
                },
              },
            },
          },
        };
      },
    });
    await waitFor(() => {
      expect(screen.getByTestId('plan-owner-W1').textContent).toBe('Lin Wei');
      expect(screen.getByTestId('plan-owner-W2').textContent).toBe('Mei Chen');
    });
    const reads = fetchMock.mock.calls
      .filter(([, init]) => String(init?.body ?? '').includes('"work.snapshot"'))
      .map(([, init]) => (JSON.parse(String(init?.body)) as { query: { work_id: string } }).query.work_id);
    expect(reads.sort()).toEqual(['W1', 'W2']);
    expect(screen.getByTestId('plan-owner-W1').textContent).not.toContain('Lin Agent');
  });

  it('does not present a failed owner read or a bare member identifier as a person', async () => {
    setupPlan({
      path: '/projects/demo?tab=plan',
      snapshot: (workId) => {
        if (workId === 'W1') return { ok: false, error: { code: 'ReadTimeout', message: 'slow' } };
        const response = demoWorkSnapshot(workId);
        response.result.data.observation.responsibility.owner = 'member_internal_002';
        return response;
      },
    });
    await waitFor(() => {
      expect(screen.getByTestId('plan-status-W1').textContent).toBe('未知');
      expect(screen.getByTestId('plan-status-W2').textContent).toBe('已认领');
    });
    expect(screen.getByTestId('plan-owner-W1').textContent).toBe('未提供');
    expect(screen.getByTestId('plan-owner-W2').textContent).toBe('未提供');
    expect(screen.getByTestId('plan-owner-W2').textContent).not.toContain('member_internal_002');
  });

  it('review reads are keyed per (work, round) with the right ids', async () => {
    const { fetchMock, router } = setupPlan({ path: '/projects/demo?tab=plan&work=W1&review_round_id=round-1' });
    const reviewCalls = () => fetchMock.mock.calls
      .filter(([, init]) => String(init?.body ?? '').includes('review.inspect'))
      .map(([, init]) => JSON.parse(String(init?.body)).query);
    await waitFor(() => expect(reviewCalls().length).toBe(1));
    expect(reviewCalls()[0]).toMatchObject({ work_id: 'W1', review_round_id: 'round-1' });
    // Select another work: a new (work, round) pair must be read, not cached.
    await act(async () => {
      await router.navigate('/projects/demo?tab=plan&work=W2&review_round_id=round-1');
    });
    await waitFor(() => expect(reviewCalls().length).toBe(2));
    expect(reviewCalls()[1]).toMatchObject({ work_id: 'W2', review_round_id: 'round-1' });
    // Another round of the same work: a third distinct read.
    await act(async () => {
      await router.navigate('/projects/demo?tab=plan&work=W2&review_round_id=round-2');
    });
    await waitFor(() => expect(reviewCalls().length).toBe(3));
    expect(reviewCalls()[2]).toMatchObject({ work_id: 'W2', review_round_id: 'round-2' });
    // Back to an already-read pair: any further read must again name that
    // exact pair (a background refetch of the same object is fine; an answer
    // for the wrong object is not). Exactly three distinct pairs were read.
    await act(async () => {
      await router.navigate('/projects/demo?tab=plan&work=W1&review_round_id=round-1');
    });
    await new Promise((resolve) => setTimeout(resolve, 50));
    const pairs = new Set(reviewCalls().map((query) => `${query.work_id}::${query.review_round_id}`));
    expect(pairs).toEqual(new Set(['W1::round-1', 'W2::round-1', 'W2::round-2']));
    for (const query of reviewCalls().slice(3)) {
      expect(query).toMatchObject({ work_id: 'W1', review_round_id: 'round-1' });
    }
  });
});

describe('work-plan review round 3: three remaining pins', () => {
  it('a Forbidden activity read degrades only the history tab, not the panel', async () => {
    setupPlan({
      path: '/projects/demo?tab=plan&work=W1',
      activity: { ok: false, error: { code: 'Forbidden', message: 'access denied' } },
    });
    // Requirements and the source-state card stay visible with real content.
    expect(await screen.findByTestId('plan-runtime-state')).toBeTruthy();
    expect(screen.getByTestId('source-state-card')).toBeTruthy();
    expect(screen.queryByTestId('state-forbidden')).toBeNull();
    const user = userEvent.setup();
    await user.click(screen.getByRole('tab', { name: '历史' }));
    const partial = await screen.findByTestId('state-partial');
    expect(partial.textContent).not.toContain('Forbidden');
    await user.click(within(partial).getByText('技术细节'));
    expect(await within(partial).findByText('Forbidden')).toBeTruthy();
  });

  it('Space on a focused row changes the selected work', async () => {
    const { router } = setupPlan({ path: '/projects/demo?tab=plan' });
    const rowLink = await screen.findByRole('link', { name: 'Build the filter API' });
    const row = rowLink.closest('tr')!;
    row.focus();
    expect(document.activeElement).toBe(row);
    const user = userEvent.setup();
    await user.keyboard('[Space]');
    await waitFor(() => expect(router.state.location.search).toContain('work=W1'));
  });

  it.each(['Forbidden', 'SessionExpired'])(
    'a work-level auth error (%s) revokes the whole observation',
    async (code) => {
      const calls: string[] = [];
      const fetchMock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
        const url = String(input);
        const body = String(init?.body ?? '');
        if (url.includes('/api/team/projects')) { calls.push('projects'); return json(DEMO_PROJECTS); }
        if (url.includes('/api/team/overview')) { calls.push('overview'); return json(DEMO_PROJECT_OVERVIEW); }
        if (url.includes('/api/team/activity')) { calls.push('activity'); return json(DEMO_ACTIVITY); }
        if (url.includes('/api/team/read')) {
          const op = JSON.parse(body).query.op as string;
          calls.push(op);
          if (op === 'work.snapshot') return json({ ok: false, error: { code, message: 'revoked' } });
          if (op === 'review.inspect') return json(DEMO_REVIEW);
        }
        return json({ ok: false, error: { code: 'NoRoute', message: url } });
      });
      vi.stubGlobal('fetch', fetchMock);
      const { queryClient } = renderApp('/projects/demo?tab=plan&work=W1');
      // The work-level auth error revokes GLOBALLY (the panel's own guard
      // would show the forbidden state either way, so assert the global
      // revocation state itself): no further read goes out afterwards.
      await screen.findByTestId('state-forbidden');
      await waitFor(() => expect(isObservationReadRevoked(queryClient)).toBe(true));
      const settled = calls.length;
      await new Promise((resolve) => setTimeout(resolve, 1200));
      expect(calls.length).toBe(settled);
      expect(calls).toContain('work.snapshot');
    },
  );
});
