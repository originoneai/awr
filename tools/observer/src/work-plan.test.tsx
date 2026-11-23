import { act, cleanup, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { axe, toHaveNoViolations } from 'jest-axe';
import { afterEach, describe, expect, it, vi } from 'vitest';
import i18n from './i18n';
import { defaultVisibleText, findForbiddenVisibleText } from './lib/human-language-guard';
import { isObservationReadRevoked } from './lib/queries';
import type { ProjectListModel } from './components/projects/project-list-model';
import { WORK_PLAN_FILTERS } from './components/projects/work-plan-model';
import { demoOverviewModelResponse, emptyOverviewModelResponse } from './test-fixtures/overview-model';
import { myWorkInput } from './test-fixtures/workbench';
import type { PlanningCandidatesData, RequiredSourcesData, TeamOverviewResponse } from './lib/types';
import { DEMO_PROJECTS, renderApp } from './test-utils';

expect.extend(toHaveNoViolations);

const NOW = Date.UTC(2026, 10, 19, 12, 0, 0);
const DAY = 86_400_000;
const DIGEST = 'a'.repeat(64);

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

function makeRichOverview(overrides: Partial<TeamOverviewResponse> = {}): TeamOverviewResponse {
  return {
    ok: true,
    project: 'demo',
    freshness: {
      report_received: { status: 'measured', at_unix_ms: NOW - 12 * 60_000 },
      authoritative: { status: 'measured', at_unix_ms: NOW },
      page_read: { status: 'measured', at_unix_ms: NOW },
    },
    workstreams: [
      { id: 'WS-001', external_key: 'WS-001', title: '前端' },
      { id: 'WS-002', external_key: 'WS-002', title: '后端' },
      { id: 'WS-003', external_key: 'WS-003', title: '验证' },
    ],
    works: [
      { key: 'T-103', title: '订单页面联调', status: 'claimed', owner_person: '林悦', agent: null, next_step: '接口第 3 版通过复核后，改用第 3 版重新联调提交流程', detail_loaded: false, workstream_id: 'WS-001', contract_hash: 'h1' },
      { key: 'T-107', title: '订单列表分页', status: 'completed', owner_person: '林悦', agent: null, next_step: '没有下一步', detail_loaded: false, workstream_id: 'WS-001', contract_hash: 'h2' },
      { key: 'T-114', title: '错误提示文案', status: 'claimed', owner_person: '林悦', agent: null, next_step: '提交复核', detail_loaded: false, workstream_id: 'WS-001', contract_hash: 'h3' },
      { key: 'T-101', title: '订单查询接口', status: 'completed', owner_person: '陈宇', agent: null, next_step: '没有下一步', detail_loaded: false, workstream_id: 'WS-002', contract_hash: 'h4' },
      { key: 'T-102', title: '接口第 3 版：提交与错误反馈', status: 'claimed', owner_person: '陈宇', agent: null, next_step: '等许宁复核；通过后林悦的页面联调改用第 3 版', detail_loaded: false, workstream_id: 'WS-002', contract_hash: 'h5' },
      { key: 'T-105', title: '登录异常处理', status: 'claimed', owner_person: '陈宇', agent: null, next_step: '周宁决定返工还是接受现状', detail_loaded: false, workstream_id: 'WS-002', contract_hash: 'h6' },
      { key: 'T-106', title: '业务验收', status: 'planned', owner_person: '许宁', agent: null, next_step: '等订单页面联调', detail_loaded: false, workstream_id: 'WS-003', contract_hash: 'h7' },
    ],
    coverage: {
      scope: { status: 'succeeded', enumerated: 7, truncated: false, next_cursor: null },
      streams: [
        { workstream_id: 'WS-001', status: 'succeeded', works_enumerated: 3, works_total: 3, truncated: false, next_cursor: null, read_at_unix_ms: NOW },
        { workstream_id: 'WS-002', status: 'succeeded', works_enumerated: 3, works_total: 3, truncated: false, next_cursor: null, read_at_unix_ms: NOW },
        { workstream_id: 'WS-003', status: 'succeeded', works_enumerated: 1, works_total: 1, truncated: false, next_cursor: null, read_at_unix_ms: NOW },
      ],
      failed: 0,
      truncated: false,
      works_enumerated: 7,
      works_total: 7,
      succeeded: 3,
    },
    ...overrides,
  };
}

function makeRichModel(overrides: Partial<ProjectListModel> = {}): ProjectListModel {
  const input = myWorkInput({
    options: {
      nowUnixMs: NOW,
      stalledAfterDays: 1,
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
        { id: 'T-103', title: '订单页面联调', projectId: 'demo', ownerPersonId: 'M-002', agentName: 'Claude 桌面', agentActive: true, started: true, acceptance: [{ evidence: [] }], report: { completed: false, reportedAtUnixMs: NOW - 12 * 60_000 }, submittedResultAtUnixMs: null, integrated: false, externalWait: null, commitments: [] },
        { id: 'T-107', title: '订单列表分页', projectId: 'demo', ownerPersonId: 'M-002', started: true, agentActive: false, acceptance: [{ evidence: [{ passed: true, current: true }] }], report: { completed: true, reportedAtUnixMs: NOW - 3 * DAY }, submittedResultAtUnixMs: NOW - 3 * DAY, verifiedAtUnixMs: NOW - 3 * DAY, integrated: true, externalWait: null, commitments: [] },
        { id: 'T-114', title: '错误提示文案', projectId: 'demo', ownerPersonId: 'M-002', started: true, agentActive: false, acceptance: [{ evidence: [] }], report: { completed: true, reportedAtUnixMs: NOW - DAY }, submittedResultAtUnixMs: null, integrated: false, externalWait: null, commitments: [] },
        { id: 'T-101', title: '订单查询接口', projectId: 'demo', ownerPersonId: 'M-003', started: true, agentActive: false, acceptance: [{ evidence: [{ passed: true, current: true }] }], report: { completed: true, reportedAtUnixMs: NOW - 5 * DAY }, submittedResultAtUnixMs: NOW - 5 * DAY, verifiedAtUnixMs: NOW - 5 * DAY, integrated: true, externalWait: null, commitments: [] },
        { id: 'T-102', title: '接口第 3 版：提交与错误反馈', projectId: 'demo', ownerPersonId: 'M-003', started: true, agentActive: false, acceptance: [{ evidence: [{ passed: true, current: true }] }], report: { completed: false, reportedAtUnixMs: NOW - 2 * DAY }, submittedResultAtUnixMs: NOW - 2 * DAY, integrated: false, externalWait: null, commitments: [] },
        { id: 'T-105', title: '登录异常处理', projectId: 'demo', ownerPersonId: 'M-003', started: true, agentActive: false, acceptance: [{ evidence: [{ passed: false, current: true }] }], report: { completed: false, reportedAtUnixMs: NOW - 2 * DAY }, submittedResultAtUnixMs: NOW - 2 * DAY, integrated: false, externalWait: null, commitments: [] },
        { id: 'T-106', title: '业务验收', projectId: 'demo', ownerPersonId: 'M-004', started: false, agentActive: false, acceptance: [{ evidence: [] }], report: null, submittedResultAtUnixMs: null, integrated: false, externalWait: null, commitments: [] },
      ],
    },
    dependencies: {
      coverage: 'complete',
      items: [
        { id: 'D-1', consumerTaskId: 'T-103', providerTaskId: 'T-102', satisfied: false, sinceUnixMs: NOW - 2 * DAY },
        { id: 'D-2', consumerTaskId: 'T-106', providerTaskId: 'T-103', satisfied: false, sinceUnixMs: NOW - 2 * DAY },
      ],
    },
    reviewRounds: {
      coverage: 'complete',
      items: [
        { id: 'R-107', taskId: 'T-107', roundIndex: 1, current: true, state: 'approved', independent: true, authorPersonId: 'M-002', reviewerPersonId: 'M-004' },
        { id: 'R-101', taskId: 'T-101', roundIndex: 2, current: true, state: 'approved', independent: true, authorPersonId: 'M-003', reviewerPersonId: 'M-004' },
        { id: 'R-102', taskId: 'T-102', roundIndex: 1, current: true, state: 'open', independent: true, authorPersonId: 'M-003', reviewerPersonId: 'M-004', openedAtUnixMs: NOW - 2 * DAY },
        { id: 'R-105', taskId: 'T-105', roundIndex: 1, current: true, state: 'rejected', independent: true, authorPersonId: 'M-003', reviewerPersonId: 'M-004', openedAtUnixMs: NOW - 2 * DAY },
      ],
    },
    decisions: { coverage: 'complete', items: [] },
    handoffs: { coverage: 'complete', items: [] },
    approvals: {
      coverage: 'complete',
      items: [
        {
          id: 'A-004',
          kind: 'plan-approval',
          recipientPersonId: 'M-001',
          projectId: 'demo',
          title: '“完善异常恢复”拆成了 4 件事',
          sinceUnixMs: NOW - 3 * 3_600_000,
        },
      ],
    },
    activity: { coverage: 'complete', items: [] },
  });
  const base = demoOverviewModelResponse(input) as unknown as ProjectListModel;
  return {
    ...base,
    project: 'demo',
    read_at_unix_ms: Date.now(),
    ...overrides,
  };
}

function makePlanningCandidates(items?: PlanningCandidatesData['items']): PlanningCandidatesData {
  return {
    items: items ?? [
      {
        candidate_id: 'PC-001',
        state: 'drafting',
        draft_revision: 1,
        author: { display_name: '陈宇的 Agent' },
        updated_at_unix_ms: Date.UTC(2026, 10, 19, 9, 10, 0),
        baseline: { matches_current: true },
        current_diff: {
          counts: { added: 4, removed: 0, changed: 0, unchanged: 0 },
          items: [
            { work_id: 'PW-001', title: '网络中断后恢复未提交的表单', kind: 'added' },
            { work_id: 'PW-002', title: '提交超时后的自动重试', kind: 'added' },
            { work_id: 'PW-003', title: '重试几次由谁来定', kind: 'added' },
            { work_id: 'PW-004', title: '异常恢复的验收场景', kind: 'added' },
          ],
        },
        approval: { value: null, reason: 'not_approved' },
        publication: { value: null, reason: 'not_published' },
        activation: { value: null, reason: 'not_activated' },
        rejection: { value: null, reason: 'not_rejected' },
      },
    ],
    total: (items ?? [null]).length,
    next_cursor: null,
    as_of: { at_unix_ms: NOW },
  };
}

function makeRequiredSources(): RequiredSourcesData {
  return {
    items: [
      { source_position: 1, path: 'docs/GOALS.md', status: 'consistent', reason: null },
    ],
    total: 1,
    next_cursor: null,
    collection: { complete: true, required_total: 1, checked_at_unix_ms: NOW },
    as_of: { at_unix_ms: NOW },
  };
}

function readEnvelope(data: unknown) {
  return {
    ok: true,
    result: { data },
    freshness: {
      report_received: { status: 'not_measured' },
      authoritative: { status: 'measured', at_unix_ms: NOW },
      page_read: { status: 'measured', at_unix_ms: NOW },
    },
  };
}

interface SetupOptions {
  path?: string;
  projects?: unknown;
  overview?: unknown | (() => Promise<Response>);
  model?: unknown | (() => Promise<Response>);
  planning?: unknown;
  sources?: unknown;
  goals?: unknown;
  displayTimeZone?: string;
}

function setupPlan({
  path = '/projects/demo?tab=plan',
  projects = makeRichProjects(),
  overview = makeRichOverview(),
  model = makeRichModel(),
  planning = readEnvelope(makePlanningCandidates()),
  sources = readEnvelope(makeRequiredSources()),
  goals = readEnvelope({
    items: [{ goal_id: 'GOAL-001', project_id: 'demo', title: '订单从查询到提交在一个页面完成' }],
    total: 1,
    next_cursor: null,
  }),
  displayTimeZone = 'UTC',
}: SetupOptions = {}) {
  const fetchMock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
    const url = String(input);
    if (url.includes('/api/team/shell')) {
      return json({ ok: true, source: 'demo', organizationName: '示例组织', perspective: null, displayTimeZone });
    }
    if (url.includes('/api/team/projects')) return json(projects);
    if (url.includes('/api/team/overview-model')) {
      return typeof model === 'function' ? model() : json(model);
    }
    if (url.includes('/api/team/overview')) {
      return typeof overview === 'function' ? overview() : json(overview);
    }
    if (url.includes('/api/team/read')) {
      const request = JSON.parse(String(init?.body)) as { query: { op: string } };
      if (request.query.op === 'planning.candidates') return json(planning);
      if (request.query.op === 'sources.inspect') return json(sources);
      if (request.query.op === 'goals.list') return json(goals);
    }
    return json({ ok: false, error: { code: 'NoRoute', message: url } });
  });
  vi.stubGlobal('fetch', fetchMock);
  return { fetchMock, ...renderApp(path) };
}

afterEach(async () => {
  cleanup();
  vi.unstubAllGlobals();
  await act(async () => { await i18n.changeLanguage('zh-CN'); });
});

describe('WorkPlanPage (AWR-PPO-042)', () => {
  it('renders pending breakdown callout with exact copy, inbox link, and no approve button (AC #1)', async () => {
    const user = userEvent.setup();
    const { router } = setupPlan();

    const callout = await screen.findByTestId('work-plan-pending-callout');
    expect(callout.textContent).toContain('有一份新的拆解等批准：');
    expect(callout.textContent).toContain('“完善异常恢复”拆成了 4 件任务，陈宇的 Agent 今天 09:10 提的。批准之前它们不会出现在下面。');
    expect(screen.queryByRole('button', { name: /批准|发布|认领|退回|Approve|Publish|Claim|Reject/i })).toBeNull();

    await user.click(screen.getByTestId('work-plan-pending-open'));
    await waitFor(() => {
      expect(router.state.location.pathname).toBe('/inbox');
      expect(router.state.location.search).toContain('scope=demo');
      expect(router.state.location.search).toContain('detail=plan-approval%3AA-004%3AM-001');
    });
  });

  it('omits the pending breakdown callout once reads settle with no pending breakdown (AC #1)', async () => {
    const modelNoApproval = makeRichModel();
    modelNoApproval.input.approvals = { coverage: 'complete', items: [] };
    const { queryClient } = setupPlan({
      model: modelNoApproval,
      planning: readEnvelope(makePlanningCandidates([])),
    });

    await screen.findByTestId('workstream-group-WS-001');
    // Wait until both reads that can trigger the callout have actually settled before asserting absence.
    await waitFor(() => {
      expect(queryClient.getQueryState(['team', 'planning.candidates', 'demo', null, null])?.status).toBe('success');
      expect(queryClient.getQueryState(['team', 'overview-model', 'demo'])?.status).toBe('success');
    });
    expect(screen.queryByTestId('work-plan-pending-callout')).toBeNull();
  });

  it('renders all 5 filters with counts, goal link, completion legend, and exact count-to-row match (AC #2 & #5)', async () => {
    const user = userEvent.setup();
    const { router } = setupPlan();

    await screen.findByTestId('workstream-group-WS-001');

    const expectedCounts: Record<(typeof WORK_PLAN_FILTERS)[number], { label: string; count: number }> = {
      all: { label: '全部 7', count: 7 },
      open: { label: '没做完 5', count: 5 },
      review: { label: '等复核 1', count: 1 },
      claimed: { label: '标了完成 1', count: 1 },
      done: { label: '已核验 2', count: 2 },
    };

    for (const filterId of WORK_PLAN_FILTERS) {
      const btn = screen.getByTestId(`work-plan-filter-${filterId}`);
      expect(btn.textContent).toBe(expectedCounts[filterId].label);
      await user.click(btn);
      await waitFor(() => expect(btn.getAttribute('aria-pressed')).toBe('true'));
      const rows = document.querySelectorAll('[data-testid^="work-plan-row-"]');
      expect(rows).toHaveLength(expectedCounts[filterId].count);
    }

    // Legend is rendered beside the filters.
    expect(screen.getByTestId('work-plan-filters').textContent).toContain('核验过');
    expect(screen.getByTestId('work-plan-filters').textContent).toContain('标了完成，没核验');

    // Goal link navigates to the goal detail page.
    const goalLink = screen.getByTestId('work-plan-goal-link');
    expect(goalLink.textContent).toBe('目标：订单从查询到提交在一个页面完成');
    await user.click(goalLink);
    await waitFor(() => {
      expect(router.state.location.pathname).toBe('/goals/GOAL-001');
      expect(router.state.location.search).toContain('scope=demo');
    });
  });

  it('omits goal link gracefully when goals.list is Unsupported', async () => {
    const { queryClient } = setupPlan({
      goals: { ok: false, error: { code: 'Unsupported', message: 'not exposed' } },
    });
    await screen.findByTestId('workstream-group-WS-001');
    await waitFor(() => {
      expect(queryClient.getQueryState(['team', 'goals.list', 'demo'])?.status).toBe('error');
    });
    expect(screen.queryByTestId('work-plan-goal-link')).toBeNull();
    expect(screen.queryByTestId('work-plan-failed')).toBeNull();
    expect(screen.queryByTestId('work-plan-partial')).toBeNull();
  });

  it('groups by workstream with header bar, highlights returned row, navigates to task page, and matches group count on click (AC #3 & #5)', async () => {
    const user = userEvent.setup();
    const { router } = setupPlan();

    await screen.findByTestId('workstream-group-WS-001');
    expect(screen.getByTestId('plan-progress-table').textContent).toContain('按工作线看');

    // All 3 workstreams render with headers, counts, and completion bars.
    const g1 = screen.getByTestId('workstream-group-WS-001');
    expect(within(g1).getByText('前端')).toBeTruthy();
    expect(within(g1).getByTestId('workstream-count-WS-001').textContent).toBe('3 件');
    expect(within(g1).getByRole('img')).toBeTruthy();

    // Returned task T-105 is highlighted; non-returned T-103 is not.
    expect(screen.getByTestId('work-plan-row-T-105').getAttribute('data-highlighted')).toBe('true');
    expect(screen.getByTestId('work-plan-row-T-103').getAttribute('data-highlighted')).toBeNull();

    // Status tag, owner with avatar, and next step render in each row.
    expect(screen.getByTestId('plan-status-T-105').textContent).toBe('复核退回');
    expect(screen.getByTestId('plan-owner-T-103').textContent).toContain('林悦');
    expect(screen.getByTestId('plan-next-T-103').textContent).toContain('接口第 3 版通过复核后');

    // Clicking a workstream's count scopes to that workstream with exact count === rendered rows.
    await user.click(screen.getByTestId('workstream-count-WS-001'));
    await waitFor(() => {
      expect(router.state.location.search).toContain('stream=WS-001');
      expect(document.querySelectorAll('[data-testid^="work-plan-row-"]')).toHaveLength(3);
    });
    await user.click(screen.getByTestId('work-plan-stream-reset'));
    await waitFor(() => {
      expect(router.state.location.search).not.toContain('stream=');
      expect(document.querySelectorAll('[data-testid^="work-plan-row-"]')).toHaveLength(7);
    });

    // Clicking a task row navigates to the dedicated task page.
    await user.click(screen.getByTestId('work-plan-row-T-103'));
    await waitFor(() => {
      expect(router.state.location.pathname).toBe('/tasks/T-103');
      expect(router.state.location.search).toContain('scope=demo');
    });
  });

  it('supports keyboard Enter navigation from a task row to its detail page (AC #3)', async () => {
    const user = userEvent.setup();
    const { router } = setupPlan();

    const row = await screen.findByTestId('work-plan-row-T-105');
    row.focus();
    await user.keyboard('{Enter}');
    await waitFor(() => {
      expect(router.state.location.pathname).toBe('/tasks/T-105');
      expect(router.state.location.search).toContain('scope=demo');
    });
  });

  it('preserves plan changes, document readiness, and returned-plan explanation (AC #4)', async () => {
    const rejectedCandidates = makePlanningCandidates([
      {
        candidate_id: 'PC-REJ-1',
        state: 'rejected',
        draft_revision: 2,
        author: { display_name: '陈宇' },
        updated_at_unix_ms: NOW - 3_600_000,
        baseline: { matches_current: true },
        current_diff: {
          counts: { added: 1, removed: 0, changed: 0, unchanged: 0 },
          items: [{ work_id: 'PW-9', title: '补充重试上限说明', kind: 'added' }],
        },
        approval: { value: null, reason: 'not_approved' },
        publication: { value: null, reason: 'not_published' },
        activation: { value: null, reason: 'not_activated' },
        rejection: {
          value: {
            actor_id: 'M-001',
            display_name: '周宁',
            at_unix_ms: NOW - 1_800_000,
            reason: '需要先明确超时重试的最大次数',
          },
          reason: null,
        },
      },
    ]);

    const { container } = setupPlan({
      planning: readEnvelope(rejectedCandidates),
    });

    const changes = await screen.findByTestId('plan-changes');
    const materials = await screen.findByTestId('plan-materials');
    expect(changes.textContent).toContain('计划变更');
    expect(changes.textContent).toContain('已退回，需调整');
    expect(changes.textContent).toContain('周宁 退回');
    expect(changes.textContent).toContain('需要先明确超时重试的最大次数');
    expect(changes.textContent).toContain('等待 Agent 根据退回原因调整方案后重新提交');
    expect(materials.textContent).toContain('资料准备情况');
    expect(materials.textContent).toContain('所需资料已齐备');
    expect(materials.textContent).toContain('项目目标');

    // No diagnostic disclosures or forbidden technical terms.
    expect(container.querySelector('details, pre, code, [data-testid="technical-details"]')).toBeNull();
    expect(findForbiddenVisibleText(defaultVisibleText(container))).toEqual([]);
  });

  it('shows prototype empty-filter copy when active filter matches 0 tasks', async () => {
    const user = userEvent.setup();
    const overviewNoClaimed = makeRichOverview({
      works: makeRichOverview().works.filter((w) => w.key !== 'T-114'),
    });
    const modelNoClaimed = makeRichModel();
    modelNoClaimed.input.tasks = {
      coverage: 'complete',
      items: (modelNoClaimed.input.tasks.items ?? []).filter((t) => t.id !== 'T-114'),
    };
    setupPlan({ overview: overviewNoClaimed, model: modelNoClaimed });

    await screen.findByTestId('workstream-group-WS-001');
    const claimedBtn = screen.getByTestId('work-plan-filter-claimed');
    expect(claimedBtn.textContent).toBe('标了完成 0');
    await user.click(claimedBtn);
    expect(await screen.findByTestId('work-plan-filter-empty')).toBeTruthy();
    expect(screen.getByTestId('work-plan-filter-empty').textContent).toBe('这个筛选下没有任务。');
    expect(document.querySelectorAll('[data-testid^="work-plan-row-"]')).toHaveLength(0);
  });

  it.each(['zh-CN', 'en-US'] as const)('renders all 7 read states cleanly with freshness & coverage in %s (AC #6)', async (language) => {
    await act(async () => { await i18n.changeLanguage(language); });

    // 1. Loading
    let releaseLoading: () => void = () => undefined;
    const gate = new Promise<Response>((resolve) => {
      releaseLoading = () => resolve(json(makeRichOverview()));
    });
    const loadingView = setupPlan({
      overview: () => gate,
      model: () => new Promise<Response>(() => undefined),
    });
    expect(await screen.findByTestId('work-plan-loading')).toBeTruthy();
    expect(screen.getByTestId('work-plan-loading').textContent).toContain(language === 'zh-CN' ? '正在读取工作计划' : 'Loading work plan');
    releaseLoading();
    loadingView.unmount();

    // 2. Data (plus axe check including heading-order!)
    const dataView = setupPlan();
    await screen.findByTestId('workstream-group-WS-001');
    expect(screen.getByTestId('work-plan-coverage')).toBeTruthy();
    expect(screen.getByTestId('freshness-badge')).toBeTruthy();
    expect(screen.getByTestId('coverage-summary')).toBeTruthy();
    expect(dataView.container.textContent).not.toMatch(/\b(undefined|null|NaN)\b/);
    expect(findForbiddenVisibleText(defaultVisibleText(dataView.container))).toEqual([]);
    const axeResults = await axe(dataView.container, { rules: { 'color-contrast': { enabled: false } } });
    expect(axeResults).toHaveNoViolations();
    dataView.unmount();

    // 3. Empty
    const emptyOverview = makeRichOverview({
      works: [],
      workstreams: [],
      coverage: {
        scope: { status: 'succeeded', enumerated: 0, truncated: false, next_cursor: null },
        streams: [],
        works_enumerated: 0,
        works_total: 0,
        succeeded: 0,
        failed: 0,
        truncated: false,
      },
    });
    const emptyModel = { ...emptyOverviewModelResponse(), project: 'demo' } as unknown as ProjectListModel;
    const emptyView = setupPlan({
      overview: emptyOverview,
      model: emptyModel,
      planning: readEnvelope(makePlanningCandidates([])),
    });
    expect(await screen.findByTestId('work-plan-empty')).toBeTruthy();
    expect(screen.getByTestId('freshness-badge')).toBeTruthy();
    expect(emptyView.container.textContent).not.toMatch(/\b(undefined|null|NaN)\b/);
    emptyView.unmount();

    // 4. Failed
    const failedView = setupPlan({
      overview: { ok: false, error: { code: 'ReadTimeout', message: 'slow' } },
      model: { ok: false, error: { code: 'ReadTimeout', message: 'slow' } },
    });
    expect(await screen.findByTestId('work-plan-failed', undefined, { timeout: 5000 })).toBeTruthy();
    expect(screen.queryByTestId('work-plan-empty')).toBeNull();
    expect(screen.getByTestId('freshness-badge')).toBeTruthy();
    expect(failedView.container.textContent).not.toContain('ReadTimeout');
    failedView.unmount();

    // 5. Partial failure
    const partialOverview = makeRichOverview({
      coverage: {
        ...makeRichOverview().coverage,
        failed: 1,
        streams: [
          ...makeRichOverview().coverage.streams,
          { workstream_id: 'WS-FAIL', status: 'failed', error_code: 'Unavailable', read_at_unix_ms: NOW },
        ],
      },
    });
    const partialView = setupPlan({ overview: partialOverview });
    expect(await screen.findByTestId('work-plan-partial')).toBeTruthy();
    expect(screen.getByTestId('coverage-stream-WS-FAIL')).toBeTruthy();
    // Unconfirmed counts must show — rather than guessing 0.
    expect(screen.getByTestId('work-plan-filter-all').textContent).toContain('—');
    expect(screen.getByTestId('work-plan-filter-done').textContent).toContain('—');
    expect(partialView.container.textContent).not.toContain('Unavailable');
    partialView.unmount();

    // 6. Unknown outcome (incomparable ordering)
    const unknownView = setupPlan({
      model: makeRichModel({ ordering: 'incomparable' }),
    });
    expect(await screen.findByTestId('work-plan-unknown')).toBeTruthy();
    expect(screen.getByTestId('work-plan-filter-open').textContent).toContain('—');
    expect(screen.getByTestId('workstream-bar-unknown-WS-001')).toBeTruthy();
    expect(screen.getByTestId('plan-status-T-103').textContent).toBe(language === 'zh-CN' ? '未知' : 'Unknown');
    unknownView.unmount();

    // 7. Revoked
    const revokedView = setupPlan({
      overview: { ok: false, error: { code: 'Forbidden', message: 'denied' } },
    });
    expect(await screen.findByTestId('state-forbidden')).toBeTruthy();
    expect(screen.queryByText('订单页面联调')).toBeNull();
    await waitFor(() => expect(isObservationReadRevoked(revokedView.queryClient)).toBe(true));
    revokedView.unmount();
  }, 30_000);

  it('never leaks raw member IDs or technical fields in owner or next-step cells', async () => {
    const overview = makeRichOverview({
      works: [
        {
          key: 'T-901',
          title: '合法任务标题',
          status: 'claimed',
          owner_person: 'member_internal_001',
          agent: { id: 'agent-raw-01', state: 'running' },
          next_step: `Run session.checkpoint with ${DIGEST}`,
          detail_loaded: false,
          workstream_id: 'WS-001',
          contract_hash: 'h9',
        },
      ],
    });
    const model = makeRichModel();
    model.input.tasks = {
      coverage: 'complete',
      items: [
        { id: 'T-901', title: '合法任务标题', projectId: 'demo', ownerPersonId: null, started: true, agentActive: true, acceptance: [{ evidence: [] }], report: null, submittedResultAtUnixMs: null, integrated: false, externalWait: null, commitments: [] },
      ],
    };
    const { container } = setupPlan({ overview, model });
    expect(await screen.findByTestId('work-plan-row-T-901')).toBeTruthy();
    expect(screen.getByTestId('plan-owner-T-901').textContent).toBe('未提供');
    expect(container.textContent).not.toContain('member_internal_001');
    expect(container.textContent).not.toContain('agent-raw-01');
    expect(container.textContent).not.toContain(DIGEST);
  });
});
