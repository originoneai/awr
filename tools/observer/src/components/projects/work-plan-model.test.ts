import { describe, expect, it } from 'vitest';
import type { PlanningCandidatesData, TeamOverviewResponse } from '../../lib/types';
import { demoOverviewModelResponse } from '../../test-fixtures/overview-model';
import { myWorkInput } from '../../test-fixtures/workbench';
import type { ProjectListModel } from './project-list-model';
import { asWorkPlanFilter, deriveWorkPlan, WORK_PLAN_FILTERS } from './work-plan-model';

const NOW = Date.UTC(2026, 10, 19, 12, 0, 0);
const DAY = 86_400_000;

function makeOverview(overrides: Partial<TeamOverviewResponse> = {}): TeamOverviewResponse {
  return {
    ok: true,
    project: 'P-001',
    workstreams: [
      { id: 'WS-001', external_key: 'WS-001', title: '前端' },
      { id: 'WS-002', external_key: 'WS-002', title: '后端' },
      { id: 'WS-003', external_key: 'WS-003', title: '验证' },
    ],
    works: [
      { key: 'T-103', title: '订单页面联调', status: 'claimed', owner_person: '林悦', agent: null, next_step: '等接口第 3 版通过复核', detail_loaded: false, workstream_id: 'WS-001', contract_hash: 'c1' },
      { key: 'T-107', title: '订单列表分页', status: 'completed', owner_person: '林悦', agent: null, next_step: '没有下一步', detail_loaded: false, workstream_id: 'WS-001', contract_hash: 'c2' },
      { key: 'T-114', title: '错误提示文案', status: 'claimed', owner_person: '林悦', agent: null, next_step: '提交复核', detail_loaded: false, workstream_id: 'WS-001', contract_hash: 'c3' },
      { key: 'T-101', title: '订单查询接口', status: 'completed', owner_person: '陈宇', agent: null, next_step: '没有下一步', detail_loaded: false, workstream_id: 'WS-002', contract_hash: 'c4' },
      { key: 'T-102', title: '接口第 3 版：提交与错误反馈', status: 'claimed', owner_person: '陈宇', agent: null, next_step: '等许宁复核', detail_loaded: false, workstream_id: 'WS-002', contract_hash: 'c5' },
      { key: 'T-105', title: '登录异常处理', status: 'claimed', owner_person: '陈宇', agent: null, next_step: '周宁决定返工还是接受现状', detail_loaded: false, workstream_id: 'WS-002', contract_hash: 'c6' },
      { key: 'T-106', title: '业务验收', status: 'planned', owner_person: '许宁', agent: null, next_step: '等订单页面联调', detail_loaded: false, workstream_id: 'WS-003', contract_hash: 'c7' },
    ],
    coverage: {
      scope: { status: 'succeeded', enumerated: 7, truncated: false, next_cursor: null },
      streams: [
        { workstream_id: 'WS-001', status: 'succeeded', works_enumerated: 3, works_total: 3, truncated: false, next_cursor: null, read_at_unix_ms: NOW },
        { workstream_id: 'WS-002', status: 'succeeded', works_enumerated: 3, works_total: 3, truncated: false, next_cursor: null, read_at_unix_ms: NOW },
        { workstream_id: 'WS-003', status: 'succeeded', works_enumerated: 1, works_total: 1, truncated: false, next_cursor: null, read_at_unix_ms: NOW },
      ],
      works_enumerated: 7,
      works_total: 7,
      succeeded: 3,
      failed: 0,
      truncated: false,
    },
    freshness: {
      report_received: { status: 'not_measured' },
      authoritative: { status: 'measured', at_unix_ms: NOW },
      page_read: { status: 'measured', at_unix_ms: NOW },
    },
    ...overrides,
  };
}

function makeModel(overrides: Partial<ProjectListModel> = {}): ProjectListModel {
  const input = myWorkInput({
    options: {
      nowUnixMs: NOW,
      stalledAfterDays: 1,
      views: { me: { personId: 'M-002', projectId: 'P-001' } },
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
        { id: 'T-103', title: '订单页面联调', projectId: 'P-001', ownerPersonId: 'M-002', agentName: 'Claude 桌面', agentActive: true, started: true, acceptance: [{ evidence: [] }], report: { completed: false, reportedAtUnixMs: NOW - 60_000 }, submittedResultAtUnixMs: null, integrated: false, externalWait: null, commitments: [] },
        { id: 'T-107', title: '订单列表分页', projectId: 'P-001', ownerPersonId: 'M-002', started: true, agentActive: false, acceptance: [{ evidence: [{ passed: true, current: true }] }], report: { completed: true, reportedAtUnixMs: NOW - 3 * DAY }, submittedResultAtUnixMs: NOW - 3 * DAY, verifiedAtUnixMs: NOW - 3 * DAY, integrated: true, externalWait: null, commitments: [] },
        { id: 'T-114', title: '错误提示文案', projectId: 'P-001', ownerPersonId: 'M-002', started: true, agentActive: false, acceptance: [{ evidence: [] }], report: { completed: true, reportedAtUnixMs: NOW - DAY }, submittedResultAtUnixMs: null, integrated: false, externalWait: null, commitments: [] },
        { id: 'T-101', title: '订单查询接口', projectId: 'P-001', ownerPersonId: 'M-003', started: true, agentActive: false, acceptance: [{ evidence: [{ passed: true, current: true }] }], report: { completed: true, reportedAtUnixMs: NOW - 5 * DAY }, submittedResultAtUnixMs: NOW - 5 * DAY, verifiedAtUnixMs: NOW - 5 * DAY, integrated: true, externalWait: null, commitments: [] },
        { id: 'T-102', title: '接口第 3 版：提交与错误反馈', projectId: 'P-001', ownerPersonId: 'M-003', started: true, agentActive: false, acceptance: [{ evidence: [{ passed: true, current: true }] }], report: { completed: false, reportedAtUnixMs: NOW - 2 * DAY }, submittedResultAtUnixMs: NOW - 2 * DAY, integrated: false, externalWait: null, commitments: [] },
        { id: 'T-105', title: '登录异常处理', projectId: 'P-001', ownerPersonId: 'M-003', started: true, agentActive: false, acceptance: [{ evidence: [{ passed: false, current: true }] }], report: { completed: false, reportedAtUnixMs: NOW - 2 * DAY }, submittedResultAtUnixMs: NOW - 2 * DAY, integrated: false, externalWait: null, commitments: [] },
        { id: 'T-106', title: '业务验收', projectId: 'P-001', ownerPersonId: 'M-004', started: false, agentActive: false, acceptance: [{ evidence: [] }], report: null, submittedResultAtUnixMs: null, integrated: false, externalWait: null, commitments: [] },
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
          projectId: 'P-001',
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
    project: 'P-001',
    input: {
      ...base.input,
      tasks: {
        ...base.input.tasks,
        items: (base.input.tasks.items ?? []).map((task) => ({ ...task, projectId: 'P-001' })),
      },
      approvals: {
        ...base.input.approvals,
        items: (base.input.approvals.items ?? []).map((approval) => ({ ...approval, projectId: 'P-001' })),
      },
    },
    ...overrides,
  };
}

function makePlanningCandidates(): PlanningCandidatesData {
  return {
    items: [
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
    total: 1,
    next_cursor: null,
    as_of: { at_unix_ms: NOW },
  };
}

describe('deriveWorkPlan', () => {
  it('derives exact filter counts, grouped workstreams, returned highlight, pending breakdown, and goal', () => {
    const result = deriveWorkPlan({
      projectKey: 'P-001',
      overview: makeOverview(),
      model: makeModel(),
      planningCandidates: makePlanningCandidates(),
      goals: [{ goal_id: 'GOAL-001', project_id: 'P-001', title: '订单从查询到提交在一个页面完成' }],
      activeFilter: 'all',
      referenceUnixMs: NOW,
    });

    expect(result.filters.all.count).toBe(7);
    expect(result.filters.open.count).toBe(5);
    expect(result.filters.review.count).toBe(1);
    expect(result.filters.claimed.count).toBe(1);
    expect(result.filters.done.count).toBe(2);

    // Every filter's count matches the exact rendered row count when selected.
    for (const filterId of WORK_PLAN_FILTERS) {
      const filtered = deriveWorkPlan({
        projectKey: 'P-001',
        overview: makeOverview(),
        model: makeModel(),
        activeFilter: filterId,
        referenceUnixMs: NOW,
      });
      expect(filtered.filters[filterId].count).toBe(filtered.totalVisibleTasks);
      expect(filtered.filters[filterId].items).toHaveLength(filtered.totalVisibleTasks);
    }

    // Returned row is highlighted; non-returned rows are not.
    const returned = result.allTasks.find((t) => t.id === 'T-105');
    expect(returned?.status).toBe('returned');
    expect(returned?.highlighted).toBe(true);
    expect(result.allTasks.filter((t) => t.highlighted).map((t) => t.id)).toEqual(['T-105']);

    // Workstream order and group counts match overview.
    expect(result.visibleGroups.map((g) => [g.id, g.totalCount, g.counts])).toEqual([
      ['WS-001', 3, { verified: 1, reported: 1, inProgress: 1, notStarted: 0 }],
      ['WS-002', 3, { verified: 1, reported: 0, inProgress: 2, notStarted: 0 }],
      ['WS-003', 1, { verified: 0, reported: 0, inProgress: 0, notStarted: 1 }],
    ]);

    // Pending breakdown extracts goal, count, author, timestamp, and exact inbox detail key.
    expect(result.pendingBreakdown).toEqual({
      goalTitle: '完善异常恢复',
      taskCount: 4,
      authorName: '陈宇的 Agent',
      proposedAtUnixMs: Date.UTC(2026, 10, 19, 9, 10, 0),
      inboxDetailKey: 'plan-approval:A-004:M-001',
    });

    // Goal link resolved from goals.list.
    expect(result.goal).toEqual({ id: 'GOAL-001', title: '订单从查询到提交在一个页面完成' });
    expect(result.coverage.hasUnknownFacts).toBe(false);
  });

  it('hides workstreams that have zero tasks under the active filter', () => {
    const reviewOnly = deriveWorkPlan({
      projectKey: 'P-001',
      overview: makeOverview(),
      model: makeModel(),
      activeFilter: 'review',
      referenceUnixMs: NOW,
    });
    expect(reviewOnly.visibleGroups.map((g) => g.id)).toEqual(['WS-002']);
    expect(reviewOnly.totalVisibleTasks).toBe(1);
    expect(reviewOnly.visibleGroups[0].totalCount).toBe(3);
  });

  it('scopes counts and rows to a single workstream when streamFilter is active', () => {
    const scoped = deriveWorkPlan({
      projectKey: 'P-001',
      overview: makeOverview(),
      model: makeModel(),
      streamFilter: 'WS-001',
      activeFilter: 'all',
      referenceUnixMs: NOW,
    });
    expect(scoped.activeStreamId).toBe('WS-001');
    expect(scoped.activeStreamTitle).toBe('前端');
    expect(scoped.filters.all.count).toBe(3);
    expect(scoped.totalVisibleTasks).toBe(3);
    expect(scoped.visibleGroups.map((g) => g.id)).toEqual(['WS-001']);
  });

  it('keeps counts null and completion bars null when ordering is incomparable or overview scope failed', () => {
    const incomparable = deriveWorkPlan({
      projectKey: 'P-001',
      overview: makeOverview(),
      model: makeModel({ ordering: 'incomparable' }),
      activeFilter: 'all',
      referenceUnixMs: NOW,
    });
    expect(incomparable.coverage.orderingIncomparable).toBe(true);
    for (const filterId of WORK_PLAN_FILTERS) {
      expect(incomparable.filters[filterId].count).toBeNull();
    }
    expect(incomparable.visibleGroups.every((g) => g.counts === null && g.totalCount === null)).toBe(true);
    expect(incomparable.allTasks.every((t) => t.status === 'unknown')).toBe(true);

    const partialOverview = makeOverview({
      coverage: {
        ...makeOverview().coverage,
        failed: 1,
      },
    });
    const partial = deriveWorkPlan({
      projectKey: 'P-001',
      overview: partialOverview,
      model: makeModel(),
      activeFilter: 'all',
      referenceUnixMs: NOW,
    });
    expect(partial.coverage.overviewScopeFailed).toBe(true);
    expect(partial.filters.all.count).toBeNull();
    expect(partial.filters.done.count).toBeNull();
    expect(partial.visibleGroups.every((g) => g.counts === null && g.totalCount === null)).toBe(true);
  });

  it('omits pendingBreakdown when candidates are already approved or rejected and no plan-approval exists', () => {
    const modelNoApproval = makeModel();
    modelNoApproval.input.approvals = { coverage: 'complete', items: [] };
    const approved = makePlanningCandidates();
    approved.items[0].state = 'approved';
    approved.items[0].approval = {
      value: { actor_id: 'M-001', display_name: '周宁', at_unix_ms: NOW, self_approved: false },
      reason: null,
    };
    const result = deriveWorkPlan({
      projectKey: 'P-001',
      overview: makeOverview(),
      model: modelNoApproval,
      planningCandidates: approved,
      activeFilter: 'all',
      referenceUnixMs: NOW,
    });
    expect(result.pendingBreakdown).toBeNull();
  });

  it('ignores plan-approval entries belonging to a different project', () => {
    const modelOtherProject = makeModel();
    modelOtherProject.input.approvals = {
      coverage: 'complete',
      items: [
        {
          id: 'A-999',
          kind: 'plan-approval',
          recipientPersonId: 'M-001',
          projectId: 'P-OTHER',
          title: '“其他项目拆解”拆成了 9 件事',
          sinceUnixMs: NOW - 3_600_000,
        },
      ],
    };
    const result = deriveWorkPlan({
      projectKey: 'P-001',
      overview: makeOverview(),
      model: modelOtherProject,
      planningCandidates: { items: [], total: 0, next_cursor: null, as_of: { at_unix_ms: NOW } },
      activeFilter: 'all',
      referenceUnixMs: NOW,
    });
    expect(result.pendingBreakdown).toBeNull();
  });

  it('uses model authoritative freshness when usableModel is present and flags unknown facts when absent', () => {
    const overviewUnmeasured = makeOverview({
      freshness: {
        report_received: { status: 'not_measured' },
        authoritative: { status: 'not_measured' },
        page_read: { status: 'measured', at_unix_ms: NOW },
      },
    });
    const withModel = deriveWorkPlan({
      projectKey: 'P-001',
      overview: overviewUnmeasured,
      model: makeModel(),
      activeFilter: 'all',
      referenceUnixMs: NOW,
    });
    expect(withModel.coverage.hasUnknownFacts).toBe(false);

    const withoutModel = deriveWorkPlan({
      projectKey: 'P-001',
      overview: overviewUnmeasured,
      activeFilter: 'all',
      referenceUnixMs: NOW,
    });
    expect(withoutModel.coverage.hasUnknownFacts).toBe(true);
    expect(withoutModel.coverage.hasUnknownStatus).toBe(true);
  });

  it('rejects non-name identifiers in owner_person and planning author fields', () => {
    const overview = makeOverview({
      works: [
        { key: 'T-900', title: '任务 A', status: 'claimed', owner_person: 'member_internal_001', agent: null, next_step: null, detail_loaded: false, workstream_id: 'WS-001', contract_hash: 'c9' },
      ],
    });
    const candidates = makePlanningCandidates();
    candidates.items[0].author.display_name = 'actor_internal_id_12345';
    const result = deriveWorkPlan({
      projectKey: 'P-001',
      overview,
      planningCandidates: candidates,
      activeFilter: 'all',
      referenceUnixMs: NOW,
    });
    expect(result.allTasks[0].ownerName).toBeNull();
    expect(result.pendingBreakdown?.authorName).toBeNull();
  });

  it('normalizes unknown filter query values to all', () => {
    expect(asWorkPlanFilter(null)).toBe('all');
    expect(asWorkPlanFilter('bogus')).toBe('all');
    expect(asWorkPlanFilter('review')).toBe('review');
    expect(asWorkPlanFilter('claimed')).toBe('claimed');
    expect(asWorkPlanFilter('done')).toBe('done');
    expect(asWorkPlanFilter('open')).toBe('open');
  });
});
