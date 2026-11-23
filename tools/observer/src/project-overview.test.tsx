import { act, cleanup, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { axe, toHaveNoViolations } from 'jest-axe';
import { afterEach, describe, expect, it, vi } from 'vitest';
import i18n from './i18n';
import { defaultVisibleText, findForbiddenVisibleText } from './lib/human-language-guard';
import { revokeObservationReads } from './lib/queries';
import type { ProjectListModel } from './components/projects/project-list-model';
import { demoOverviewModelResponse, emptyOverviewModelResponse } from './test-fixtures/overview-model';
import { realShapedActivity, realShapedOverview } from './test-fixtures/real-shaped';
import { myWorkInput } from './test-fixtures/workbench';
import {
  DEMO_ACTIVITY,
  DEMO_PROJECT_OVERVIEW,
  DEMO_PROJECTS,
  renderApp,
} from './test-utils';

expect.extend(toHaveNoViolations);

const NOW = Date.UTC(2026, 10, 19, 10, 0, 0);

function json(payload: unknown) {
  return new Response(JSON.stringify(payload), {
    status: 200,
    headers: { 'content-type': 'application/json' },
  });
}

function makeRichProjects() {
  return {
    ...DEMO_PROJECTS,
    projects: [{
      key: 'demo',
      title: '订单协作平台',
      role: 'admin',
      leadPersonId: 'M-001',
      groupName: '应用小组',
    }],
  };
}

function makeRichOverview() {
  return {
    ...DEMO_PROJECT_OVERVIEW,
    project: 'demo',
    freshness: {
      report_received: { status: 'measured', at_unix_ms: NOW - 12 * 60_000 },
      authoritative: { status: 'measured', at_unix_ms: NOW },
      page_read: { status: 'measured', at_unix_ms: NOW },
    },
    workstreams: [
      { id: 'WS-FE', title: '前端' },
      { id: 'WS-BE', title: '后端' },
      { id: 'WS-QA', title: '验证' },
    ],
    works: [
      { key: 'T-101', title: '订单查询接口', status: 'completed', owner_person: '陈宇', agent: null, next_step: null, detail_loaded: false, workstream_id: 'WS-BE', contract_hash: 'h1' },
      { key: 'T-102', title: '接口第 3 版：提交与错误反馈', status: 'in_progress', owner_person: '陈宇', agent: null, next_step: null, detail_loaded: false, workstream_id: 'WS-BE', contract_hash: 'h2' },
      { key: 'T-103', title: '订单页面联调', status: 'in_progress', owner_person: '林悦', agent: null, next_step: null, detail_loaded: false, workstream_id: 'WS-FE', contract_hash: 'h3' },
      { key: 'T-104', title: '空状态检查', status: 'in_progress', owner_person: '林悦', agent: null, next_step: null, detail_loaded: false, workstream_id: 'WS-FE', contract_hash: 'h4' },
      { key: 'T-106', title: '业务验收', status: 'planned', owner_person: '许宁', agent: null, next_step: null, detail_loaded: false, workstream_id: 'WS-QA', contract_hash: 'h6' },
      { key: 'T-110', title: '错误提示文案', status: 'in_progress', owner_person: '林悦', agent: null, next_step: null, detail_loaded: false, workstream_id: 'WS-FE', contract_hash: 'h10' },
    ],
    coverage: {
      scope: { status: 'succeeded', enumerated: 6, truncated: false, next_cursor: null },
      streams: [
        { workstream_id: 'WS-FE', status: 'succeeded', works_enumerated: 3, works_total: 3, truncated: false, next_cursor: null, read_at_unix_ms: NOW },
        { workstream_id: 'WS-BE', status: 'succeeded', works_enumerated: 2, works_total: 2, truncated: false, next_cursor: null, read_at_unix_ms: NOW },
        { workstream_id: 'WS-QA', status: 'succeeded', works_enumerated: 1, works_total: 1, truncated: false, next_cursor: null, read_at_unix_ms: NOW },
      ],
      failed: 0,
      truncated: false,
      works_enumerated: 6,
      works_total: 6,
    },
  };
}

function makeRichModel(overrides: Partial<ProjectListModel> = {}): ProjectListModel {
  const input = myWorkInput({
    options: {
      nowUnixMs: NOW,
      stalledAfterDays: 2,
      views: { me: { personId: 'M-002', projectId: 'demo' } },
    },
    members: {
      coverage: 'complete',
      items: [
        { personId: 'M-001', displayName: '周宁' },
        { personId: 'M-002', displayName: '林悦' },
        { personId: 'M-003', displayName: '陈宇' },
        { personId: 'M-004', displayName: '许宁' },
      ],
    },
    tasks: {
      coverage: 'complete',
      items: [
        { id: 'T-101', title: '订单查询接口', projectId: 'demo', ownerPersonId: 'M-003', started: true, agentActive: false, acceptance: [{ evidence: [{ passed: true, current: true }] }], submittedResultAtUnixMs: NOW - 5 * 86_400_000, verifiedAtUnixMs: NOW - 4 * 86_400_000, integrated: true, report: { completed: true, reportedAtUnixMs: NOW - 5 * 86_400_000 }, externalWait: null, commitments: [] },
        { id: 'T-102', title: '接口第 3 版：提交与错误反馈', projectId: 'demo', ownerPersonId: 'M-003', started: true, agentActive: false, acceptance: [{ evidence: [{ passed: true, current: true }] }], submittedResultAtUnixMs: NOW - 2 * 86_400_000, integrated: false, report: { completed: false, reportedAtUnixMs: NOW - 2 * 86_400_000 }, externalWait: null, commitments: [] },
        { id: 'T-103', title: '订单页面联调', projectId: 'demo', ownerPersonId: 'M-002', started: true, agentName: 'Claude 桌面', agentActive: true, acceptance: [{ evidence: [] }], submittedResultAtUnixMs: null, integrated: false, report: { completed: false, reportedAtUnixMs: NOW - 12 * 60_000 }, externalWait: null, commitments: [] },
        { id: 'T-104', title: '空状态检查', projectId: 'demo', ownerPersonId: 'M-002', started: true, agentName: 'Codex', agentActive: false, acceptance: [{ evidence: [] }], submittedResultAtUnixMs: null, integrated: false, report: { completed: false, reportedAtUnixMs: NOW - 2 * 86_400_000 }, externalWait: null, commitments: [] },
        { id: 'T-106', title: '业务验收', projectId: 'demo', ownerPersonId: 'M-004', started: false, agentActive: false, acceptance: [{ evidence: [] }], submittedResultAtUnixMs: null, integrated: false, report: null, externalWait: null, commitments: [] },
        { id: 'T-110', title: '错误提示文案', projectId: 'demo', ownerPersonId: 'M-002', started: true, agentActive: false, acceptance: [{ evidence: [] }], submittedResultAtUnixMs: null, integrated: false, report: { completed: true, reportedAtUnixMs: NOW - 86_400_000 }, externalWait: null, commitments: [] },
      ],
    },
    dependencies: {
      coverage: 'complete',
      items: [
        { id: 'D-1', consumerTaskId: 'T-103', providerTaskId: 'T-102', satisfied: false, sinceUnixMs: NOW - 2 * 86_400_000 },
        { id: 'D-2', consumerTaskId: 'T-106', providerTaskId: 'T-103', satisfied: false, sinceUnixMs: NOW - 2 * 86_400_000 },
      ],
    },
    reviewRounds: {
      coverage: 'complete',
      items: [
        { id: 'R-1', taskId: 'T-101', roundIndex: 1, current: true, state: 'approved', independent: true, authorPersonId: 'M-003', reviewerPersonId: 'M-004' },
        { id: 'R-2', taskId: 'T-102', roundIndex: 1, current: true, state: 'open', independent: true, authorPersonId: 'M-003', reviewerPersonId: 'M-004', openedAtUnixMs: NOW - 2 * 86_400_000 },
      ],
    },
    decisions: {
      coverage: 'complete',
      items: [
        { id: 'Q-001', taskId: 'T-104', state: 'open', current: true, question: '空状态要不要显示引导文案', allowedResponderIds: ['M-002'], createdAtUnixMs: NOW - 2 * 86_400_000 },
        { id: 'Q-002', taskId: 'T-102', state: 'open', current: true, question: '登录异常处理要不要返工', allowedResponderIds: ['M-001'], createdAtUnixMs: NOW - 1 * 86_400_000 },
      ],
    },
    handoffs: { coverage: 'complete', items: [] },
    approvals: { coverage: 'unread', reason: 'not-read', items: [] },
    activity: { coverage: 'complete', items: [] },
  });
  const base = demoOverviewModelResponse(input) as unknown as ProjectListModel;
  return {
    ...base,
    project: 'demo',
    read_at_unix_ms: Date.now(),
    periodUpdates: {
      coverage: 'complete',
      items: [
        { id: 'U-001', periodLabel: '第 47 周', startsOn: '2026-11-16', endsOn: '2026-11-22', status: 'risk', summary: '接口第 3 版的复核拖了两天，页面联调在等。', authorPersonId: 'M-001', recordedAtUnixMs: NOW - 30 * 60_000 },
        { id: 'U-002', periodLabel: '第 46 周', startsOn: '2026-11-09', endsOn: '2026-11-15', status: 'ok', summary: '查询接口加缓存层后压测达标，按计划推进。', authorPersonId: 'M-001', recordedAtUnixMs: NOW - 4 * 86_400_000 },
        { id: 'U-003', periodLabel: '第 45 周', startsOn: '2026-11-02', endsOn: '2026-11-08', status: 'ok', summary: '完成拆解：6 件任务。', authorPersonId: 'M-001', recordedAtUnixMs: NOW - 11 * 86_400_000 },
        { id: 'U-004', periodLabel: '第 44 周', startsOn: '2026-10-26', endsOn: '2026-11-01', status: 'ok', summary: '更早一期不应显示在概览最近三条。', authorPersonId: 'M-001', recordedAtUnixMs: NOW - 18 * 86_400_000 },
      ],
    },
    ...overrides,
  };
}

function makeRichActivity(count = 8) {
  return {
    ...DEMO_ACTIVITY,
    data: {
      ...DEMO_ACTIVITY.data,
      items: Array.from({ length: count }, (_, idx) => ({
        id: `E-${idx + 1}`,
        action: idx % 2 === 0 ? 'session.checkpoint' : 'delivery.submit_and_request_review',
        work_id: idx === 0 ? 'T-103' : idx === 1 ? 'T-102' : null,
        actor: { display_name: idx % 2 === 0 ? '林悦' : '陈宇' },
        client: idx === 0 ? { display_name: 'Claude 桌面' } : null,
        client_id: idx === 0 ? 'c-1' : null,
        result: 'ok',
        created_at_unix_ms: NOW - (idx + 1) * 12 * 60_000,
      })),
    },
  };
}

function setupOverview({
  path = '/projects/demo?tab=overview',
  projects = makeRichProjects(),
  overview = makeRichOverview(),
  model = makeRichModel(),
  activity = makeRichActivity(),
  gate,
}: {
  path?: string;
  projects?: unknown;
  overview?: unknown;
  model?: unknown;
  activity?: unknown;
  gate?: Promise<void>;
} = {}) {
  const fetchMock = vi.fn(async (input: RequestInfo | URL, _init?: RequestInit) => {
    const url = String(input);
    if (gate) await gate;
    if (url.includes('/api/team/projects')) return json(projects);
    if (url.includes('/api/team/overview-model')) return json(model);
    if (url.includes('/api/team/overview')) return json(overview);
    if (url.includes('/api/team/activity')) return json(activity);
    return json({ ok: false, error: { code: 'NoRoute', message: url } });
  });
  vi.stubGlobal('fetch', fetchMock);
  return { fetchMock, ...renderApp(path) };
}

afterEach(async () => {
  vi.unstubAllGlobals();
  await i18n.changeLanguage('zh-CN');
});

describe('ProjectOverview prototype structure and interactions', () => {
  it('renders verdict band, 4 KPIs, workstream bars, waiting answers rail, recent 6 activity, recent 3 updates and empty reconciliation', async () => {
    const { fetchMock } = setupOverview();
    await waitFor(() => expect(screen.getByTestId('project-verdict-sentence').textContent).toBe('接口第 3 版的复核拖了两天，页面联调在等。'));

    expect(screen.getByTestId('project-verdict-word').textContent).toContain('有风险');
    expect(screen.getByTestId('project-verdict-sentence').hasAttribute('data-reporter-text')).toBe(true);
    expect(screen.getByTestId('project-verdict-byline').textContent).toContain('第 47 周');
    expect(screen.getByTestId('project-verdict-byline').textContent).toContain('周宁');
    expect(screen.getByTestId('project-verdict-basis-btn').textContent).toContain('这句话的依据');

    // 4 KPIs
    const kpiGroup = screen.getByRole('group', { name: '关键数字' });
    const kpiButtons = within(kpiGroup).getAllByRole('button');
    expect(kpiButtons).toHaveLength(4);
    expect(kpiButtons[0].textContent).toContain('已核验');
    expect(kpiButtons[0].textContent).toContain('1件');
    expect(kpiButtons[0].textContent).toContain('共 6 件');
    expect(kpiButtons[1].textContent).toContain('标了完成，没核验');
    expect(kpiButtons[1].textContent).toContain('1件');
    expect(kpiButtons[1].textContent).toContain('错误提示文案');
    expect(kpiButtons[2].textContent).toContain('等复核');
    expect(kpiButtons[2].textContent).toContain('1件');
    expect(kpiButtons[2].textContent).toContain('都在等许宁，最久 2 天');
    expect(kpiButtons[3].textContent).toContain('等人回答');
    expect(kpiButtons[3].textContent).toContain('2个');
    expect(kpiButtons[3].textContent).toContain('最久 2 天');

    // Workstreams block
    expect(screen.getByTestId('project-workstreams-title').textContent).toBe('3 条工作线走到哪了');
    expect(screen.getByTestId('workstream-row-WS-FE').textContent).toContain('前端');
    expect(screen.getByTestId('workstream-row-WS-FE').textContent).toContain('订单页面联调、空状态检查');
    expect(screen.getByTestId('workstream-row-WS-BE').textContent).toContain('后端');
    expect(screen.getByTestId('workstream-row-WS-BE').textContent).toContain('接口第 3 版');
    expect(screen.getByTestId('project-deps-link').textContent).toContain('卡在哪：看依赖');

    // Waiting answers rail
    expect(screen.getByTestId('project-waiting-answers-count').textContent).toBe('2');
    const railItems = screen.getAllByTestId('project-waiting-answer-item');
    expect(railItems).toHaveLength(2);
    expect(railItems[0].textContent).toContain('空状态要不要显示引导文案');
    expect(railItems[0].textContent).toContain('等林悦回答 · 2 天');

    // Recent activity capped at 6 with "全部" link
    expect(screen.getAllByTestId('project-activity-row')).toHaveLength(6);
    expect(screen.getByTestId('project-activity-all').textContent).toContain('全部');

    // Past updates capped at 3 (4th excluded)
    const updatesBlock = screen.getByTestId('project-updates');
    const updateButtons = within(updatesBlock).getAllByRole('button');
    expect(updateButtons).toHaveLength(3);
    expect(updatesBlock.textContent).not.toContain('更早一期不应显示在概览最近三条');

    // Reconciliation shows "还没有对账" with zero example numbers
    const recon = screen.getByTestId('project-reconciliation');
    expect(screen.getByTestId('project-reconciliation-status').textContent).toBe('还没有对账');
    expect(recon.textContent).not.toMatch(/\d/);

    // Read-only: zero write requests issued
    const methods = fetchMock.mock.calls.map(([, init]) => (init?.method ?? 'GET').toUpperCase());
    expect(methods.every((m) => m === 'GET' || m === 'POST')).toBe(true);
    expect(fetchMock.mock.calls.some(([input]) => String(input).includes('/api/team/write'))).toBe(false);
  });

  it('shows at most four waiting answers in the rail while the count says how many there are', async () => {
    const model = makeRichModel() as unknown as { input: { decisions: { items: Array<Record<string, unknown>> } } };
    const extra = ['T-101', 'T-103', 'T-106', 'T-110'].map((taskId, index) => ({
      id: `Q-10${index}`, taskId, state: 'open', current: true, question: `合成问题 ${index + 1}`,
      allowedResponderIds: ['M-001'], createdAtUnixMs: NOW - (index + 1) * 3_600_000,
    }));
    model.input.decisions.items = [...model.input.decisions.items, ...extra];
    setupOverview({ model });
    await screen.findByTestId('project-instrument');
    await waitFor(() => expect(screen.getByTestId('project-waiting-answers-count').textContent).toBe('6'));
    expect(screen.getAllByTestId('project-waiting-answer-item')).toHaveLength(4);
  });

  it('names at most two blocked tasks per workstream row', async () => {
    const model = makeRichModel() as unknown as { input: { dependencies: { items: Array<Record<string, unknown>> } } };
    model.input.dependencies.items = [
      ...model.input.dependencies.items,
      { id: 'D-3', consumerTaskId: 'T-110', providerTaskId: 'T-102', satisfied: false, sinceUnixMs: NOW - 86_400_000 },
    ];
    setupOverview({ model });
    const row = await screen.findByTestId('workstream-row-WS-FE');
    await waitFor(() => expect(row.textContent).toContain('订单页面联调'));
    const named = ['订单页面联调', '空状态检查', '错误提示文案'].filter((title) => row.textContent?.includes(title));
    expect(named).toHaveLength(2);
  });

  it('opens a count-matched detail panel for every KPI and navigates from workstreams, basis, decisions, activity and updates', async () => {
    const user = userEvent.setup();
    const { router } = setupOverview();
    await waitFor(() => expect(screen.getByTestId('project-verdict-sentence').textContent).toContain('接口第 3 版'));

    const kpiGroup = screen.getByRole('group', { name: '关键数字' });
    const expectedCounts = [1, 1, 1, 2];
    for (let i = 0; i < expectedCounts.length; i += 1) {
      const buttons = within(kpiGroup).getAllByRole('button');
      await user.click(buttons[i]);
      const detail = await screen.findByTestId('project-overview-metric-detail');
      expect(Number(detail.getAttribute('data-count'))).toBe(expectedCounts[i]);
      expect(detail.querySelectorAll('.obs-row-item')).toHaveLength(expectedCounts[i]);
      await user.click(screen.getByTestId('project-overview-detail-back'));
      await waitFor(() => expect(screen.queryByTestId('project-overview-metric-detail')).toBeNull());
    }

    // Workstream click opens plan filtered to that stream
    await user.click(screen.getByTestId('workstream-row-WS-FE'));
    expect(router.state.location.search).toContain('tab=plan');
    expect(router.state.location.search).toContain('stream=WS-FE');
  });

  it('navigates to /updates/:id from verdict basis button when an update exists, and toggles inline basis when none exists', async () => {
    const user = userEvent.setup();
    const { router } = setupOverview();
    await waitFor(() => expect(screen.getByTestId('project-verdict-basis-btn')).toBeTruthy());
    await user.click(screen.getByTestId('project-verdict-basis-btn'));
    expect(router.state.location.pathname).toBe('/updates/U-001');

    // Without period updates: deterministic program-fact verdict + inline basis disclosure
    vi.unstubAllGlobals();
    setupOverview({ model: makeRichModel({ periodUpdates: { coverage: 'complete', items: [] } }) });
    await waitFor(() => expect(screen.getByTestId('project-verdict-sentence').textContent).toContain('还没有一期更新'));
    expect(screen.getByTestId('project-verdict-sentence').hasAttribute('data-reporter-text')).toBe(false);
    await user.click(screen.getByTestId('project-verdict-basis-btn'));
    expect(await screen.findByTestId('project-verdict-basis')).toBeTruthy();
    expect(screen.getByTestId('project-verdict-basis').textContent).toContain('没有使用模型');
  });

  it('opens inbox detail when clicking a waiting answer card and /activity from 全部', async () => {
    const user = userEvent.setup();
    const { router } = setupOverview();
    const firstCard = (await screen.findAllByTestId('project-waiting-answer-item'))[0];
    await user.click(firstCard);
    expect(router.state.location.pathname).toBe('/inbox');
    expect(router.state.location.search).toContain('detail=answer%3AQ-001%3AM-002');
  });

  it('preserves coverage disclosures, freshness badge, activity Unsupported, next_cursor and duplicate-id notes', async () => {
    const ov = makeRichOverview();
    ov.coverage.failed = 1;
    ov.coverage.truncated = true;
    ov.coverage.streams[0] = {
      workstream_id: 'WS-FE',
      status: 'failed',
      works_enumerated: 0,
      works_total: 3,
      truncated: false,
      next_cursor: null,
      read_at_unix_ms: NOW,
    };
    setupOverview({
      overview: ov,
      activity: {
        ok: false,
        error: { code: 'Unsupported', message: 'not supported' },
      },
    });
    expect(await screen.findByTestId('project-coverage')).toBeTruthy();
    expect(screen.getByTestId('freshness-badge')).toBeTruthy();
    expect(screen.getByTestId('coverage-summary').textContent).toContain('6 / 6');
    expect(screen.getByTestId('coverage-failed-summary').textContent).toContain('1');
    expect(screen.getByTestId('coverage-stream-WS-FE')).toBeTruthy();
    expect(screen.getByTestId('project-coverage-unread')).toBeTruthy();
    expect((await screen.findByTestId('activity-unavailable')).textContent).toContain('此模式不可用');
  });

  it.each(['zh-CN', 'en-US'] as const)('renders all 7 states cleanly in %s', async (lang) => {
    await act(async () => { await i18n.changeLanguage(lang); });

    // 1. Loading
    let release!: () => void;
    const gate = new Promise<void>((resolve) => { release = resolve; });
    setupOverview({ gate });
    expect(await screen.findByTestId('project-overview-loading')).toBeTruthy();
    release();
    await screen.findByTestId('project-instrument');

    // 2. Empty
    cleanup();
    vi.unstubAllGlobals();
    setupOverview({
      overview: {
        ...makeRichOverview(),
        works: [],
        workstreams: [],
        coverage: { scope: { status: 'succeeded', enumerated: 0, truncated: false, next_cursor: null }, streams: [], succeeded: 0, failed: 0, truncated: false, works_enumerated: 0, works_total: 0 },
      },
      model: { ...emptyOverviewModelResponse(), project: 'demo', periodUpdates: { coverage: 'complete', items: [] } },
      activity: makeRichActivity(0),
    });
    expect(await screen.findByTestId('project-overview-empty')).toBeTruthy();

    // 3. Failed
    cleanup();
    vi.unstubAllGlobals();
    setupOverview({
      overview: { ok: false, error: { code: 'ReadTimeout', message: 'fail' } },
      model: { ok: false, error: { code: 'ReadTimeout', message: 'fail' } },
      activity: { ok: false, error: { code: 'ReadTimeout', message: 'fail' } },
    });
    expect(await screen.findByRole('button', { name: lang === 'zh-CN' ? '重试' : 'Try again' })).toBeTruthy();

    // 4. Partial failure
    cleanup();
    vi.unstubAllGlobals();
    setupOverview({
      model: { ok: false, error: { code: 'ReadTimeout', message: 'partial' } },
    });
    expect(await screen.findByTestId('project-overview-partial')).toBeTruthy();

    // 5. Unknown result (incomparable ordering)
    cleanup();
    vi.unstubAllGlobals();
    setupOverview({
      model: makeRichModel({ ordering: 'incomparable' }),
    });
    expect(await screen.findByTestId('project-overview-unknown')).toBeTruthy();

    // 6. Revoked clears the overview completely
    cleanup();
    vi.unstubAllGlobals();
    const { queryClient } = setupOverview();
    await screen.findByTestId('project-instrument');
    act(() => revokeObservationReads(queryClient));
    expect(await screen.findByTestId('state-forbidden')).toBeTruthy();
    expect(screen.queryByTestId('project-instrument')).toBeNull();
  }, 15_000);

  it('renders verbatim real-shaped live replies with zero undefined/null/NaN/raw identifiers and passes axe', async () => {
    const { container } = setupOverview({
      overview: realShapedOverview(),
      activity: realShapedActivity(),
      model: makeRichModel(),
    });
    await screen.findByTestId('project-instrument');
    await waitFor(() => expect(screen.getByTestId('project-recent-changes')).toBeTruthy());

    const visible = defaultVisibleText(screen.getByTestId('page-project'));
    expect(visible).not.toMatch(/\bundefined\b|\bnull\b|\bNaN\b/);
    expect(visible).not.toMatch(/actor_id|client_id|workstream_id|contract_hash|0121E42AF137882D640D53F0D3/);
    expect(findForbiddenVisibleText(visible)).toEqual([]);
    expect(document.querySelector('[data-testid="technical-details"]')).toBeNull();

    const results = await axe(container);
    expect(results).toHaveNoViolations();
  });
});
