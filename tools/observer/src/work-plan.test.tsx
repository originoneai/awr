import { act, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { axe, toHaveNoViolations } from 'jest-axe';
import { afterEach, describe, expect, it, vi } from 'vitest';
import i18n from './i18n';
import { formatUnixMs } from './lib/format';
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
  it('renders real groups and fields, filters by workstream, and disables board view', async () => {
    const { router } = setupPlan({ path: '/projects/demo?tab=plan' });
    expect(await screen.findByRole('link', { name: 'Build the filter API' })).toBeTruthy();
    expect(screen.getByTestId('workstream-group-WS1').textContent).toContain('Backend workstream');
    expect(screen.getByTestId('workstream-group-WS2').textContent).toContain('Frontend workstream');
    expect(screen.getByTestId('plan-status-W1').textContent).toBe('未测量');
    expect(screen.getByTestId('plan-owner-W1').textContent).toBe('未测量');
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
  it('renders real contract, required specs, runtime, compact guidance, and report time', async () => {
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
    expect(screen.getByTestId('plan-detail').textContent).toContain('docs/support.md');
    expect(screen.getByTestId('plan-detail').textContent).toContain('Keep the support API stable.');
    expect(screen.getByTestId('plan-runtime-state').textContent).toBe('claimed');
    expect(screen.getByTestId('plan-guidance').textContent).toBe('session.checkpoint');
    expect(screen.getByTestId('plan-guidance').textContent).not.toContain('Batch progress');
    expect(screen.getByTestId('last-report-W1').textContent).toBe(formatUnixMs(1761839800000, 'zh-CN'));
    expect(screen.getByTestId('last-report-W1').textContent).not.toBe(formatUnixMs(1761839900000, 'zh-CN'));
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

  it('shows factual source fields and an honest not-measured takeover note', async () => {
    setupPlan();
    const card = await screen.findByTestId('source-state-card');
    expect(card.textContent).toContain('snapshot-1');
    expect(card.textContent).toContain('epoch-1');
    expect(card.textContent).toContain('contract-1');
    expect(card.textContent).toContain('enabled');
    expect(card.textContent).toContain('work');
    expect(card.textContent).toContain('project_revision');
    expect(card.textContent).toContain('authority_version');
    expect(card.textContent).toContain('selection_basis');
    expect(card.textContent).toContain('未测量');
    expect(card.textContent).not.toContain('已同步');
    expect(card.textContent).not.toContain('takeover complete');
  });

  it('marks a missing attribution triple as not measured, never synchronized', async () => {
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
    expect(within(card).getByTestId('source-snapshot-id').textContent).toBe('未测量');
    expect(within(card).getByTestId('source-project-revision').textContent).toBe('未测量');
    expect(within(card).getByTestId('source-coordinator-epoch').textContent).toBe('未测量');
    expect(card.textContent).not.toContain('已同步');
  });

  it('keeps candidate and source-set blocks as later-slice placeholders', async () => {
    setupPlan();
    expect((await screen.findByTestId('plan-candidates-placeholder')).textContent).toContain('后续切片');
    expect((await screen.findByTestId('plan-source-set-placeholder')).textContent).toContain('后续切片');
  });
});

describe('work detail review outcomes', () => {
  it('renders raw review decisions and metadata without narrative', async () => {
    const user = userEvent.setup();
    setupPlan({ path: '/projects/demo?tab=plan&work=W1&review_round_id=round-1' });
    await user.click(await screen.findByRole('tab', { name: '成果' }));
    const review = await screen.findByTestId('plan-review');
    const decision = within(review).getByTestId('review-decision-decision-1');
    expect(within(decision).getByTestId('review-decision-code').textContent).toBe('approve');
    expect(decision.textContent).toContain('simulated_member_independent_review');
    expect(decision.textContent).toContain('simulated_member_independent');
    expect(decision.textContent).toContain('human_approval');
    expect(review.textContent).toContain('author-agent-1');
    expect(review.textContent).toContain('artifact-digest-1');
    expect(review.textContent).toContain('bundle-hash-1');
    expect(review.querySelector('[data-testid="review-narrative"]')).toBeNull();
    expect(review.textContent).toContain('后续切片');
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
      .toContain('复核轮次无已证实来源');
    const reviewCalls = fetchMock.mock.calls.filter(([, init]) => String(init?.body ?? '').includes('"review.inspect"'));
    expect(reviewCalls).toHaveLength(0);
  });
});

describe('work detail history', () => {
  it('filters development history by work_id and renders raw fields and notes', async () => {
    const user = userEvent.setup();
    const { fetchMock } = setupPlan();
    await user.click(await screen.findByRole('tab', { name: '历史' }));
    const history = await screen.findByTestId('plan-history');
    const item = await within(history).findByTestId('activity-item-activity-1');
    expect(within(item).getByTestId('activity-action').textContent).toBe('access.changed');
    expect(within(item).getByTestId('activity-actor').textContent).toBe('actor-raw-17');
    expect(within(item).getByTestId('activity-result').textContent).toBe('granted');
    expect(within(item).getByTestId('activity-time').textContent)
      .toBe(formatUnixMs(1761839700000, 'zh-CN'));
    expect(history.textContent).toContain('audit.development');
    expect(history.textContent).toContain('not_observed');
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
    expect(screen.getByTestId('plan-history').textContent).toContain('ReadTimeout');
    expect(screen.getByTestId('plan-history').textContent).toContain('部分读取失败');
  });

  it('discloses earlier records and caps the newest history at twenty', async () => {
    const user = userEvent.setup();
    const items = Array.from({ length: 22 }, (_, index) => ({
      ...DEMO_ACTIVITY.data.items[0],
      id: `activity-${index + 1}`,
      action: `action-${index + 1}`,
      created_at_unix_ms: 1761839700000 + index * 1000,
    }));
    setupPlan({ activity: makeActivity(items, { next_cursor: 'older-2' }) });
    await user.click(await screen.findByRole('tab', { name: '历史' }));
    const history = await screen.findByTestId('plan-history');
    await waitFor(() => expect(history.querySelectorAll('[data-testid^="activity-item-"]')).toHaveLength(20));
    expect(history.querySelectorAll('[data-testid^="activity-item-"]')[0].getAttribute('data-testid'))
      .toBe('activity-item-activity-22');
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
    expect(screen.getByText('Source state')).toBeTruthy();
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
    expect(partial.textContent).toContain('Forbidden');
  });

  it('a missing client_reported_at reads 未测量, never the current clock', async () => {
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
    await waitFor(() => expect(cell.textContent).toBe('未测量'));
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
    expect(partial.textContent).toContain('ReadTimeout');
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
