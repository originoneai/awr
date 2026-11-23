import { describe, expect, it } from 'vitest';
import {
  buildDeliveryVersionMap,
  deriveProjectBlockers,
  deriveProjectDependencies,
  mergeDependenciesFreshness,
} from './project-dependencies-model';
import type { ProjectListModel } from './project-list-model';
import type { DependencyListData, DependencyObservation, TeamFreshness } from '../../lib/types';
import { deriveOverview } from '../../core/overview';
import { myWorkInput } from '../../test-fixtures/workbench';

const NOW = Date.UTC(2026, 10, 19, 10, 0, 0);
const DAY = 86_400_000;

function makeDep(overrides: Partial<DependencyObservation> = {}): DependencyObservation {
  return {
    work_id: 'T-103',
    work_title: '订单页面联调',
    provider_work_id: 'T-102',
    provider_title: '接口第 3 版：提交与错误反馈',
    dependency_position: 1,
    version_policy: 'current_contract',
    review_assurance: null,
    required_version: { receipt_id: null, contract_hash: 'hash-102', source_snapshot_id: 'snap-1' },
    available_version: null,
    adopted_version: null,
    consumed_version: null,
    adoption_valid: null,
    validity: 'unavailable',
    reason: 'completion_unavailable',
    missing: { available_version: 'not_recorded', adopted_version: 'not_recorded', consumed_version: 'not_recorded' },
    ...overrides,
  };
}

function makeModel(overrides: Partial<ProjectListModel> = {}): ProjectListModel {
  const input = myWorkInput({
    options: { nowUnixMs: NOW, stalledAfterDays: 2, views: { me: { personId: 'M-002', projectId: 'P-001' } } },
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
        { id: 'T-102', title: '接口第 3 版：提交与错误反馈', projectId: 'P-001', ownerPersonId: 'M-003', started: true, agentActive: false, acceptance: [{ evidence: [{ passed: true, current: true }] }], submittedResultAtUnixMs: NOW - 2 * DAY, integrated: false, report: { completed: false, reportedAtUnixMs: NOW - 2 * DAY }, externalWait: null, commitments: [{ id: 'P-001-week-47', title: 'Week 47', kind: 'project' }] },
        { id: 'T-103', title: '订单页面联调', projectId: 'P-001', ownerPersonId: 'M-002', started: true, agentName: 'Claude 桌面', agentActive: true, acceptance: [{ evidence: [] }], submittedResultAtUnixMs: null, integrated: false, report: { completed: false, reportedAtUnixMs: NOW - 60_000 }, externalWait: null, commitments: [{ id: 'P-001-week-47', title: 'Week 47', kind: 'project' }] },
        { id: 'T-104', title: '空状态检查', projectId: 'P-001', ownerPersonId: 'M-002', started: true, agentName: 'Codex', agentActive: false, acceptance: [{ evidence: [] }], submittedResultAtUnixMs: null, integrated: false, report: { completed: false, reportedAtUnixMs: NOW - 2 * DAY }, externalWait: null, commitments: [{ id: 'P-001-week-47', title: 'Week 47', kind: 'project' }] },
        { id: 'T-105', title: '登录异常处理', projectId: 'P-001', ownerPersonId: 'M-003', started: true, agentActive: false, acceptance: [{ evidence: [] }], submittedResultAtUnixMs: NOW - 2 * DAY, integrated: false, report: { completed: false, reportedAtUnixMs: NOW - 2 * DAY }, externalWait: null, commitments: [] },
        { id: 'T-106', title: '业务验收', projectId: 'P-001', ownerPersonId: 'M-004', started: false, agentActive: false, acceptance: [{ evidence: [] }], submittedResultAtUnixMs: null, integrated: false, report: null, externalWait: null, commitments: [{ id: 'P-001-week-47', title: 'Week 47', kind: 'project' }] },
        { id: 'T-109', title: '提交失败的错误码表', projectId: 'P-001', ownerPersonId: 'M-003', started: true, agentActive: false, acceptance: [{ evidence: [{ passed: true, current: true }] }], submittedResultAtUnixMs: NOW - DAY, integrated: false, report: { completed: false, reportedAtUnixMs: NOW - DAY }, externalWait: null, commitments: [] },
        { id: 'T-110', title: '订单状态机说明', projectId: 'P-001', ownerPersonId: 'M-003', started: true, agentActive: false, acceptance: [{ evidence: [{ passed: true, current: true }] }], submittedResultAtUnixMs: NOW - DAY, integrated: false, report: { completed: false, reportedAtUnixMs: NOW - DAY }, externalWait: null, commitments: [] },
        { id: 'T-111', title: '导出功能', projectId: 'P-001', ownerPersonId: 'M-002', started: true, agentActive: false, acceptance: [{ evidence: [] }], submittedResultAtUnixMs: null, integrated: false, report: null, externalWait: { party: '甲方', what: '确认导出字段', sinceUnixMs: NOW - 5 * DAY }, commitments: [] },
      ],
    },
    dependencies: {
      coverage: 'complete',
      items: [
        { id: 'D-1', consumerTaskId: 'T-103', providerTaskId: 'T-102', satisfied: false, what: '通过复核的版本', sinceUnixMs: NOW - 2 * DAY },
        { id: 'D-2', consumerTaskId: 'T-103', providerTaskId: 'T-104', satisfied: false, what: '已核验', sinceUnixMs: NOW - 2 * DAY },
        { id: 'D-3', consumerTaskId: 'T-106', providerTaskId: 'T-103', satisfied: false, what: '已核验', sinceUnixMs: null },
      ],
    },
    decisions: {
      coverage: 'complete',
      items: [
        { id: 'Q-1', taskId: 'T-104', state: 'open', current: true, question: '空状态要不要显示引导文案', allowedResponderIds: ['M-002'], createdAtUnixMs: NOW - 2 * DAY },
        { id: 'Q-2', taskId: 'T-105', state: 'open', current: true, question: '要不要返工', allowedResponderIds: ['M-001'], createdAtUnixMs: NOW - DAY },
      ],
    },
    reviewRounds: {
      coverage: 'complete',
      items: [
        { id: 'R-102-1', taskId: 'T-102', roundIndex: 1, state: 'rejected', current: false, independent: true, authorPersonId: 'M-003', reviewerPersonId: 'M-004', openedAtUnixMs: NOW - 5 * DAY },
        { id: 'R-102-2', taskId: 'T-102', roundIndex: 2, state: 'approved', current: false, independent: true, authorPersonId: 'M-003', reviewerPersonId: 'M-004', openedAtUnixMs: NOW - 4 * DAY },
        { id: 'R-102-3', taskId: 'T-102', roundIndex: 3, state: 'open', current: true, independent: true, authorPersonId: 'M-003', reviewerPersonId: 'M-004', openedAtUnixMs: NOW - 2 * DAY },
        { id: 'R-109-1', taskId: 'T-109', roundIndex: 1, state: 'open', current: true, independent: true, authorPersonId: 'M-003', reviewerPersonId: 'M-004', openedAtUnixMs: NOW - DAY },
        { id: 'R-110-1', taskId: 'T-110', roundIndex: 1, state: 'open', current: true, independent: true, authorPersonId: 'M-003', reviewerPersonId: 'M-004', openedAtUnixMs: NOW - DAY },
      ],
    },
    handoffs: { coverage: 'complete', items: [] },
    approvals: { coverage: 'complete', items: [] },
    activity: { coverage: 'complete', items: [] },
  });
  return {
    ok: true,
    project: 'P-001',
    read_at_unix_ms: NOW,
    input,
    coverage: {
      tasks: { status: 'complete', missing: [] },
      dependencies: { status: 'complete', missing: [] },
      decisions: { status: 'complete', missing: [] },
      reviewRounds: { status: 'complete', missing: [] },
      handoffs: { status: 'complete', missing: [] },
      approvals: { status: 'complete', missing: [] },
      members: { status: 'complete', missing: [] },
      activity: { status: 'complete', missing: [] },
    },
    freshness: {
      report_received: { status: 'not_measured' },
      authoritative: { status: 'measured', at_unix_ms: NOW },
      page_read: { status: 'measured', at_unix_ms: NOW },
    },
    ...overrides,
  };
}

describe('buildDeliveryVersionMap', () => {
  it('numbers deliveries by sequence per provider without exposing receipt identifiers', () => {
    const v1 = { receipt_id: '01J-RECEIPT-ALPHA', contract_hash: 'h1', source_snapshot_id: 's1' };
    const v2 = { receipt_id: '01J-RECEIPT-BETA', contract_hash: 'h2', source_snapshot_id: 's2' };
    const v3 = { receipt_id: '01J-RECEIPT-GAMMA', contract_hash: 'h3', source_snapshot_id: 's3' };
    const map = buildDeliveryVersionMap([
      makeDep({
        provider_work_id: 'T-201',
        consumed_version: v1,
        adopted_version: v2,
        available_version: v3,
        validity: 'upstream_changed',
      }),
      makeDep({
        work_id: 'T-204',
        provider_work_id: 'T-201',
        adopted_version: v3,
        available_version: v3,
        validity: 'valid',
      }),
    ], []);

    expect(map.versionOf('T-201', v1)).toBe(1);
    expect(map.versionOf('T-201', v2)).toBe(2);
    expect(map.versionOf('T-201', v3)).toBe(3);
  });

  it('aligns a single delivered receipt with the approved round index when earlier rounds were rejected', () => {
    const v2 = { receipt_id: '01J-RECEIPT-V2', contract_hash: 'h2', source_snapshot_id: 's2' };
    const map = buildDeliveryVersionMap(
      [makeDep({ provider_work_id: 'T-101', adopted_version: v2, available_version: v2, validity: 'valid' })],
      [
        { id: 'r1', taskId: 'T-101', roundIndex: 1, state: 'rejected', current: false },
        { id: 'r2', taskId: 'T-101', roundIndex: 2, state: 'approved', current: true },
      ],
    );
    expect(map.versionOf('T-101', v2)).toBe(2);
    expect(map.latestRoundNumberOf('T-101')).toBe(2);
  });
});

describe('deriveProjectDependencies', () => {
  it('groups review blockers by explicit reviewer, sorts descending by behind count, and maps destinations to person or task', () => {
    const model = makeModel();
    const deps: DependencyListData = {
      work_id: null,
      items: [
        makeDep({
          work_id: 'T-103',
          work_title: '订单页面联调',
          provider_work_id: 'T-102',
          provider_title: '接口第 3 版：提交与错误反馈',
          adopted_version: { receipt_id: 'rcpt-2', contract_hash: 'h-2', source_snapshot_id: 's1' },
          available_version: null,
          validity: 'upstream_changed',
          reason: 'upstream_changed',
        }),
        makeDep({
          work_id: 'T-103',
          work_title: '订单页面联调',
          provider_work_id: 'T-104',
          provider_title: '空状态检查',
          dependency_position: 2,
          validity: 'unavailable',
          reason: 'completion_unavailable',
        }),
        makeDep({
          work_id: 'T-106',
          work_title: '业务验收',
          provider_work_id: 'T-103',
          provider_title: '订单页面联调',
          dependency_position: 3,
          validity: 'unavailable',
          reason: 'completion_unavailable',
        }),
      ],
      total: 3,
      next_cursor: null,
      as_of: { at_unix_ms: NOW },
    };

    const result = deriveProjectDependencies({ projectKey: 'P-001', model, dependencies: deps });

    // Blockers sorted descending by behind.count:
    // 1. 许宁的复核队列 (behind 5: T-102, T-103, T-106, T-109, T-110) -> person M-004, lamp 'blocked'
    // 2. 林悦回答 (behind 3: T-104, T-103, T-106) -> task T-104, lamp 'stalled'
    // 3. 甲方外部等待 (behind 1, 5 days) -> task T-111, lamp 'stalled'
    // 4. 周宁决定 (behind 1, 1 day) -> task T-105, lamp 'stalled'
    expect(result.blockersCount).toBe(4);
    expect(result.blockers.map((b) => [b.kind, b.behind.count, b.lamp, b.destination])).toEqual([
      ['review', 5, 'blocked', { kind: 'person', id: 'M-004' }],
      ['decision', 3, 'stalled', { kind: 'task', id: 'T-104' }],
      ['external', 1, 'stalled', { kind: 'task', id: 'T-111' }],
      ['decision', 1, 'stalled', { kind: 'task', id: 'T-105' }],
    ]);
    // Count === detail items length for every blocker.
    for (const blocker of result.blockers) {
      expect(blocker.behind.items.length).toBe(blocker.behind.count);
    }

    // Dependency rows preserve statuses, version numbering, and provider status fallbacks.
    expect(result.rows.map((r) => [r.consumerWorkId, r.providerWorkId, r.status, r.highlighted, r.adopted, r.available])).toEqual([
      ['T-103', 'T-102', 'changed', true, { kind: 'version', version: 2 }, { kind: 'inReviewVersion', version: 3 }],
      ['T-103', 'T-104', 'none', false, { kind: 'notYet' }, { kind: 'stalledAnswer' }],
      ['T-106', 'T-103', 'none', false, { kind: 'notYet' }, { kind: 'waitingUpstream' }],
    ]);

    // Primary chain walks root -> T-102 -> T-103 -> commitment (4 nodes), preserving full impact of 5.
    const topChain = result.chains[0];
    expect(topChain.displayNodes.length).toBe(4);
    expect(topChain.displayNodes[0].root).toBe(true);
    expect(topChain.displayNodes[1].task?.id).toBe('T-102');
    expect(topChain.displayNodes[2].task?.id).toBe('T-103');
    expect(topChain.displayNodes[3].kind).toBe('commitment');
    expect(topChain.impact.count).toBe(5);
    expect(topChain.impact.items.length).toBe(5);
  });

  it('keeps unknown reviewers separate and marks counts unknown when ordering is incomparable', () => {
    const model = makeModel({ ordering: 'incomparable' });
    const result = deriveProjectDependencies({ projectKey: 'P-001', model });
    expect(result.chainsCount).toBeNull();
    expect(result.blockersCount).toBeNull();
    expect(result.coverage.orderingIncomparable).toBe(true);
    expect(result.coverage.hasUnknownFacts).toBe(true);
    for (const blocker of result.blockers) {
      expect(blocker.behind.count).toBeNull();
    }

    // Two review rounds with unknown reviewers must NOT merge into one queue.
    const derived = deriveOverview(myWorkInput({
      options: { nowUnixMs: NOW, stalledAfterDays: 2 },
      tasks: {
        coverage: 'complete',
        items: [
          { id: 'T-1', title: 'A', started: true, externalWait: null, acceptance: [] },
          { id: 'T-2', title: 'B', started: true, externalWait: null, acceptance: [] },
        ],
      },
      reviewRounds: {
        coverage: 'complete',
        items: [
          { id: 'r1', taskId: 'T-1', roundIndex: 1, state: 'open', current: true, reviewerPersonId: null, openedAtUnixMs: NOW - DAY },
          { id: 'r2', taskId: 'T-2', roundIndex: 1, state: 'open', current: true, reviewerPersonId: null, openedAtUnixMs: NOW - DAY },
        ],
      },
    }));
    const anonBlockers = deriveProjectBlockers(derived, false);
    expect(anonBlockers.items.length).toBe(2);
    expect(anonBlockers.items[0].destination).toEqual({ kind: 'task', id: 'T-1' });
    expect(anonBlockers.items[1].destination).toEqual({ kind: 'task', id: 'T-2' });
  });

  it('derives ok, unadopted, restricted, and unknown dependency row statuses honestly', () => {
    const v1 = { receipt_id: 'r1', contract_hash: 'h1', source_snapshot_id: 's1' };
    const deps: DependencyListData = {
      work_id: null,
      items: [
        makeDep({ work_id: 'W-1', provider_work_id: 'P-1', adopted_version: v1, available_version: v1, validity: 'valid', reason: null }),
        makeDep({ work_id: 'W-2', provider_work_id: 'P-1', dependency_position: 2, adopted_version: null, available_version: v1, validity: 'valid', reason: 'adoption_not_recorded' }),
        makeDep({ work_id: 'W-3', provider_work_id: 'P-2', provider_title: null, dependency_position: 3, validity: 'unavailable', reason: 'permission_restricted', missing: { required_version: 'permission_restricted', available_version: 'permission_restricted', adopted_version: 'permission_restricted' } }),
        makeDep({ work_id: 'W-4', provider_work_id: 'P-3', dependency_position: 4, validity: 'unexpected' as unknown as DependencyObservation['validity'] }),
      ],
      total: 4,
      next_cursor: null,
      as_of: { at_unix_ms: NOW },
    };
    const result = deriveProjectDependencies({ projectKey: 'P-001', dependencies: deps });
    expect(result.rows.map((r) => r.status)).toEqual(['ok', 'unadopted', 'restricted', 'unknown']);
    expect(result.coverage.hasUnknownFacts).toBe(true);
  });

  it('merges freshness planes conservatively (failed > stale > measured > not_measured)', () => {
    const a: TeamFreshness = {
      report_received: { status: 'not_measured' },
      authoritative: { status: 'measured', at_unix_ms: 1000 },
      page_read: { status: 'measured', at_unix_ms: 2000 },
    };
    const b: TeamFreshness = {
      report_received: { status: 'stale', at_unix_ms: 500 },
      authoritative: { status: 'failed' },
      page_read: { status: 'measured', at_unix_ms: 3000 },
    };
    expect(mergeDependenciesFreshness(a, b)).toEqual({
      report_received: { status: 'stale', at_unix_ms: 500 },
      authoritative: { status: 'failed', at_unix_ms: 1000 },
      page_read: { status: 'measured', at_unix_ms: 3000 },
    });
  });
});
