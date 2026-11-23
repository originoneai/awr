import { describe, expect, it } from 'vitest';
import { myWorkInput } from '../../test-fixtures/workbench';
import { demoOverviewModelResponse } from '../../test-fixtures/overview-model';
import type { TeamOverviewResponse } from '../../lib/types';
import type { ProjectListEntry, ProjectListModel } from './project-list-model';
import {
  deriveProjectOverview,
  isoWeekNumber,
  mergeProjectFreshness,
} from './project-overview-model';

const NOW = Date.UTC(2026, 10, 19, 10, 0, 0);

function makeOverview(overrides: Partial<TeamOverviewResponse> = {}): TeamOverviewResponse {
  return {
    ok: true,
    project: 'P-001',
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
      succeeded: 3,
      failed: 0,
      truncated: false,
      works_enumerated: 6,
      works_total: 6,
    },
    ...overrides,
  };
}

function makeModel(overrides: Partial<ProjectListModel> = {}): ProjectListModel {
  const input = myWorkInput({
    options: {
      nowUnixMs: NOW,
      stalledAfterDays: 2,
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
        {
          id: 'T-101', title: '订单查询接口', projectId: 'P-001',
          ownerPersonId: 'M-003', started: true, agentActive: false,
          acceptance: [{ evidence: [{ passed: true, current: true }] }],
          submittedResultAtUnixMs: NOW - 5 * 86_400_000,
          verifiedAtUnixMs: NOW - 4 * 86_400_000,
          integrated: true,
          report: { completed: true, reportedAtUnixMs: NOW - 5 * 86_400_000 },
          externalWait: null, commitments: [],
        },
        {
          id: 'T-102', title: '接口第 3 版：提交与错误反馈', projectId: 'P-001',
          ownerPersonId: 'M-003', started: true, agentActive: false,
          acceptance: [{ evidence: [{ passed: true, current: true }] }],
          submittedResultAtUnixMs: NOW - 2 * 86_400_000,
          integrated: false,
          report: { completed: false, reportedAtUnixMs: NOW - 2 * 86_400_000 },
          externalWait: null, commitments: [],
        },
        {
          id: 'T-103', title: '订单页面联调', projectId: 'P-001',
          ownerPersonId: 'M-002', started: true, agentName: 'Claude 桌面', agentActive: true,
          acceptance: [{ evidence: [] }],
          submittedResultAtUnixMs: null, integrated: false,
          report: { completed: false, reportedAtUnixMs: NOW - 12 * 60_000 },
          externalWait: null, commitments: [],
        },
        {
          id: 'T-104', title: '空状态检查', projectId: 'P-001',
          ownerPersonId: 'M-002', started: true, agentName: 'Codex', agentActive: false,
          acceptance: [{ evidence: [] }],
          submittedResultAtUnixMs: null, integrated: false,
          report: { completed: false, reportedAtUnixMs: NOW - 2 * 86_400_000 },
          externalWait: null, commitments: [],
        },
        {
          id: 'T-106', title: '业务验收', projectId: 'P-001',
          ownerPersonId: 'M-004', started: false, agentActive: false,
          acceptance: [{ evidence: [] }],
          submittedResultAtUnixMs: null, integrated: false,
          report: null, externalWait: null, commitments: [],
        },
        {
          id: 'T-110', title: '错误提示文案', projectId: 'P-001',
          ownerPersonId: 'M-002', started: true, agentActive: false,
          acceptance: [{ evidence: [] }],
          submittedResultAtUnixMs: null, integrated: false,
          report: { completed: true, reportedAtUnixMs: NOW - 86_400_000 },
          externalWait: null, commitments: [],
        },
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
        {
          id: 'Q-001', taskId: 'T-104', state: 'open', current: true,
          question: '空状态要不要显示引导文案',
          allowedResponderIds: ['M-002'],
          createdAtUnixMs: NOW - 2 * 86_400_000,
        },
        {
          id: 'Q-002', taskId: 'T-102', state: 'open', current: true,
          question: '登录异常处理要不要返工',
          allowedResponderIds: ['M-001'],
          createdAtUnixMs: NOW - 1 * 86_400_000,
        },
        {
          id: 'Q-009', taskId: 'T-101', state: 'replied', current: true,
          question: '已答问题不应计入',
          allowedResponderIds: ['M-002'],
          createdAtUnixMs: NOW - 5 * 86_400_000,
        },
      ],
    },
    handoffs: { coverage: 'complete', items: [] },
    approvals: { coverage: 'unread', reason: 'not-read', items: [] },
    activity: { coverage: 'complete', items: [] },
  });
  const base = demoOverviewModelResponse(input) as unknown as ProjectListModel;
  return {
    ...base,
    project: 'P-001',
    periodUpdates: {
      coverage: 'complete',
      items: [
        {
          id: 'U-001', periodLabel: '第 47 周', startsOn: '2026-11-16', endsOn: '2026-11-22',
          status: 'risk', summary: '接口第 3 版的复核拖了两天，页面联调在等。',
          authorPersonId: 'M-001', recordedAtUnixMs: NOW - 30 * 60_000,
        },
        {
          id: 'U-002', startsOn: '2026-11-09', endsOn: '2026-11-15',
          status: 'ok', summary: '查询接口加缓存层后压测达标。',
          authorName: '周宁', recordedAtUnixMs: NOW - 4 * 86_400_000,
        },
        {
          id: 'U-003', startsOn: '2026-11-02', endsOn: '2026-11-08',
          status: 'ok', summary: '完成拆解。',
          authorName: '周宁', recordedAtUnixMs: NOW - 11 * 86_400_000,
        },
        {
          id: 'U-004', startsOn: '2026-10-26', endsOn: '2026-11-01',
          status: 'ok', summary: '第四条不应出现在最近三条。',
          authorName: '周宁', recordedAtUnixMs: NOW - 18 * 86_400_000,
        },
      ],
    },
    ...overrides,
  };
}

describe('isoWeekNumber', () => {
  it('computes the ISO week for a Mon-Sun range and rejects non-week ranges', () => {
    expect(isoWeekNumber('2026-11-16', '2026-11-22')).toBe(47);
    expect(isoWeekNumber('2026-11-09', '2026-11-15')).toBe(46);
    expect(isoWeekNumber('2026-11-17', '2026-11-23')).toBeNull();
    expect(isoWeekNumber('2026-11-16', '2026-11-20')).toBeNull();
    expect(isoWeekNumber(null, null)).toBeNull();
  });
});

describe('mergeProjectFreshness', () => {
  it('picks the worst plane status and latest measured timestamp', () => {
    const merged = mergeProjectFreshness(
      {
        report_received: { status: 'measured', at_unix_ms: 1000 },
        authoritative: { status: 'measured', at_unix_ms: 2000 },
        page_read: { status: 'measured', at_unix_ms: 3000 },
      },
      {
        report_received: { status: 'stale', at_unix_ms: 1500 },
        authoritative: { status: 'failed', at_unix_ms: 2500 },
        page_read: { status: 'measured', at_unix_ms: 3500 },
      },
    );
    expect(merged?.report_received).toEqual({ status: 'stale', at_unix_ms: 1500 });
    expect(merged?.authoritative).toEqual({ status: 'failed', at_unix_ms: 2500 });
    expect(merged?.page_read).toEqual({ status: 'measured', at_unix_ms: 3500 });
  });
});

describe('deriveProjectOverview', () => {
  const entry: ProjectListEntry = {
    key: 'P-001',
    title: '订单协作平台',
    leadPersonId: 'M-001',
    groupName: '应用小组',
  };

  it('derives header facts, verdict, 4 count-backed KPIs, workstream bars, decisions and recent 3 updates', () => {
    const result = deriveProjectOverview({
      projectKey: 'P-001',
      projectEntry: entry,
      overview: makeOverview(),
      model: makeModel(),
      timeZone: 'UTC',
      referenceUnixMs: NOW,
    });

    expect(result.health).toBe('risk');
    expect(result.lead).toEqual({ id: 'M-001', name: '周宁' });
    expect(result.groupName).toBe('应用小组');
    expect(result.taskCount.count).toBe(6);
    expect(result.taskCount.items).toHaveLength(6);
    expect(result.workstreamCount).toBe(3);

    expect(result.verdict.kind).toBe('update');
    expect(result.verdict.latestUpdate?.id).toBe('U-001');
    expect(result.verdict.latestUpdate?.authorName).toBe('周宁');

    // Every KPI count matches its backing items array length exactly.
    expect(result.kpis.verified.count).toBe(1);
    expect(result.kpis.verified.items).toHaveLength(1);
    expect(result.kpis.claimed.count).toBe(1);
    expect(result.kpis.claimed.items).toHaveLength(1);
    expect(result.kpis.claimed.items[0].id).toBe('T-110');
    expect(result.kpis.review.count).toBe(1);
    expect(result.kpis.review.items).toHaveLength(1);
    expect(result.kpis.review.reviewerNames).toEqual(['许宁']);
    expect(result.kpis.review.oldestDays).toBe(2);
    expect(result.kpis.decisions.count).toBe(2);
    expect(result.kpis.decisions.items).toHaveLength(2);
    expect(result.kpis.decisions.oldestDays).toBe(2);
    // Longest-waiting decision sorts first and builds inboxDetailKey.
    expect(result.kpis.decisions.items[0].id).toBe('Q-001');
    expect(result.kpis.decisions.items[0].inboxDetailKey).toBe('answer:Q-001:M-002');
    expect(result.kpis.decisions.items[1].id).toBe('Q-002');
    expect(result.kpis.decisions.items[1].inboxDetailKey).toBe('decision:Q-002:M-001');

    // Workstreams retain order, 4-segment counts sum to task count, and blocked tasks are listed.
    expect(result.workstreams.map((ws) => ws.id)).toEqual(['WS-FE', 'WS-BE', 'WS-QA']);
    expect(result.workstreams[0].counts).toEqual({ verified: 0, reported: 1, inProgress: 2, notStarted: 0 });
    expect(result.workstreams[0].blockedTasks.map((t) => t.id)).toEqual(['T-103', 'T-104']);
    expect(result.workstreams[1].counts).toEqual({ verified: 1, reported: 0, inProgress: 1, notStarted: 0 });
    expect(result.workstreams[1].blockedTasks.map((t) => t.id)).toEqual(['T-102']);
    expect(result.workstreams[2].counts).toEqual({ verified: 0, reported: 0, inProgress: 0, notStarted: 1 });

    // Recent updates capped at 3, newest first, total reflects all 4 recorded updates.
    expect(result.recentUpdates.items.map((u) => u.id)).toEqual(['U-001', 'U-002', 'U-003']);
    expect(result.recentUpdates.total).toBe(4);
    expect(result.coverage.hasUnknownFacts).toBe(false);
  });

  it('produces a deterministic program-fact verdict when no period updates exist', () => {
    const model = makeModel({ periodUpdates: { coverage: 'complete', items: [] } });
    const result = deriveProjectOverview({
      projectKey: 'P-001',
      projectEntry: entry,
      overview: makeOverview(),
      model,
      timeZone: 'UTC',
      referenceUnixMs: NOW,
    });
    expect(result.health).toBe('none');
    expect(result.verdict.kind).toBe('facts');
    expect(result.verdict.factCounts).toEqual({ total: 6, verified: 1, review: 1, decisions: 2 });
    expect(result.recentUpdates.items).toEqual([]);
    expect(result.recentUpdates.total).toBe(0);
  });

  it('produces a stale-update verdict when the latest update is older than 7 days', () => {
    const model = makeModel({
      periodUpdates: {
        coverage: 'complete',
        items: [{
          id: 'U-OLD', startsOn: '2026-11-02', endsOn: '2026-11-08',
          status: 'ok', summary: '旧更新', authorPersonId: 'M-001',
          recordedAtUnixMs: NOW - 12 * 86_400_000,
        }],
      },
    });
    const result = deriveProjectOverview({
      projectKey: 'P-001',
      projectEntry: entry,
      overview: makeOverview(),
      model,
      timeZone: 'UTC',
      referenceUnixMs: NOW,
    });
    expect(result.health).toBe('none');
    expect(result.verdict.kind).toBe('stale-update');
    expect(result.verdict.daysSinceLastUpdate).toBe(12);
  });

  it('never coerces unread or incomparable facts to zero', () => {
    const model = makeModel({ ordering: 'incomparable' });
    const result = deriveProjectOverview({
      projectKey: 'P-001',
      projectEntry: entry,
      overview: makeOverview(),
      model,
      timeZone: 'UTC',
      referenceUnixMs: NOW,
    });
    expect(result.health).toBe('unknown');
    expect(result.verdict.kind).toBe('unknown');
    expect(result.taskCount.count).toBeNull();
    expect(result.kpis.verified.count).toBeNull();
    expect(result.kpis.claimed.count).toBeNull();
    expect(result.kpis.review.count).toBeNull();
    expect(result.kpis.decisions.count).toBeNull();
    expect(result.workstreams.every((ws) => ws.counts === null)).toBe(true);
    expect(result.coverage.orderingIncomparable).toBe(true);
    expect(result.coverage.hasUnknownFacts).toBe(true);
  });

  it('keeps decisions count null when decisions slice is unread or has unknown state', () => {
    const base = makeModel();
    const unreadModel: ProjectListModel = {
      ...base,
      input: { ...base.input, decisions: { coverage: 'unread', reason: 'not-read', items: [] } },
    };
    const res1 = deriveProjectOverview({
      projectKey: 'P-001',
      projectEntry: entry,
      overview: makeOverview(),
      model: unreadModel,
      timeZone: 'UTC',
      referenceUnixMs: NOW,
    });
    expect(res1.kpis.decisions.count).toBeNull();
    expect(res1.coverage.modelUnreadParts).toContain('decisions');

    const badStateModel: ProjectListModel = {
      ...base,
      input: {
        ...base.input,
        decisions: {
          coverage: 'complete',
          items: [{ id: 'Q-X', taskId: 'T-101', state: 'mystery', current: true }],
        },
      },
    };
    const res2 = deriveProjectOverview({
      projectKey: 'P-001',
      projectEntry: entry,
      overview: makeOverview(),
      model: badStateModel,
      timeZone: 'UTC',
      referenceUnixMs: NOW,
    });
    expect(res2.kpis.decisions.count).toBeNull();
  });

  it('marks workstreamCount null when overview coverage has failed streams or truncation', () => {
    const ov = makeOverview();
    ov.coverage.failed = 1;
    const result = deriveProjectOverview({
      projectKey: 'P-001',
      projectEntry: entry,
      overview: ov,
      model: makeModel(),
      timeZone: 'UTC',
      referenceUnixMs: NOW,
    });
    expect(result.workstreamCount).toBeNull();
  });
});
