import { describe, expect, it } from 'vitest';
import {
  deriveProjectBlockers,
  deriveProjectDependencies,
  hasRecordedCommitmentTitle,
  isProjectBlockedChain,
  mergeDependenciesFreshness,
  soleDecisionResponder,
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

function makeModel(overrides: Partial<ProjectListModel> = {}, extraDeps: Array<{ id: string; consumerTaskId: string; providerTaskId: string; satisfied: boolean; what: string; sinceUnixMs: number | null }> = []): ProjectListModel {
  const input = myWorkInput({
    options: { nowUnixMs: NOW, stalledAfterDays: 2, views: { me: { personId: 'M-002', projectId: 'P-001' } } },
    members: {
      coverage: 'complete',
      items: [
        { personId: 'M-001', displayName: '周宁' },
        { personId: 'M-002', displayName: '林悦' },
        { personId: 'M-003', displayName: '陈宇' },
        { personId: 'M-004', displayName: '许宁' },
        { personId: 'M-010', displayName: '部门负责人' },
      ],
    },
    tasks: {
      coverage: 'complete',
      items: [
        { id: 'T-102', title: '接口第 3 版：提交与错误反馈', projectId: 'P-001', ownerPersonId: 'M-003', started: true, agentActive: false, acceptance: [{ evidence: [{ passed: true, current: true }] }], submittedResultAtUnixMs: NOW - 2 * DAY, integrated: false, report: { completed: false, reportedAtUnixMs: NOW - 2 * DAY }, externalWait: null, commitments: [{ id: 'comm-orders-closure', title: '订单查询与提交闭环', kind: 'project' }] },
        { id: 'T-103', title: '订单页面联调', projectId: 'P-001', ownerPersonId: 'M-002', started: true, agentName: 'Claude 桌面', agentActive: true, acceptance: [{ evidence: [] }], submittedResultAtUnixMs: null, integrated: false, report: { completed: false, reportedAtUnixMs: NOW - 60_000 }, externalWait: null, commitments: [{ id: 'comm-orders-closure', title: '订单查询与提交闭环', kind: 'project' }] },
        { id: 'T-104', title: '空状态检查', projectId: 'P-001', ownerPersonId: 'M-002', started: true, agentName: 'Codex', agentActive: false, acceptance: [{ evidence: [] }], submittedResultAtUnixMs: null, integrated: false, report: { completed: false, reportedAtUnixMs: NOW - 2 * DAY }, externalWait: null, commitments: [{ id: 'comm-orders-closure', title: '订单查询与提交闭环', kind: 'project' }] },
        { id: 'T-105', title: '登录异常处理', projectId: 'P-001', ownerPersonId: 'M-003', started: true, agentActive: false, acceptance: [{ evidence: [] }], submittedResultAtUnixMs: NOW - 2 * DAY, integrated: false, report: { completed: false, reportedAtUnixMs: NOW - 2 * DAY }, externalWait: null, commitments: [] },
        { id: 'T-106', title: '业务验收', projectId: 'P-001', ownerPersonId: 'M-004', started: false, agentActive: false, acceptance: [{ evidence: [] }], submittedResultAtUnixMs: null, integrated: false, report: null, externalWait: null, commitments: [{ id: 'comm-orders-closure', title: '订单查询与提交闭环', kind: 'project' }] },
        { id: 'T-109', title: '提交失败的错误码表', projectId: 'P-001', ownerPersonId: 'M-003', started: true, agentActive: false, acceptance: [{ evidence: [{ passed: true, current: true }] }], submittedResultAtUnixMs: NOW - DAY, integrated: false, report: { completed: false, reportedAtUnixMs: NOW - DAY }, externalWait: null, commitments: [] },
        { id: 'T-110', title: '订单状态机说明', projectId: 'P-001', ownerPersonId: 'M-003', started: true, agentActive: false, acceptance: [{ evidence: [{ passed: true, current: true }] }], submittedResultAtUnixMs: NOW - DAY, integrated: false, report: { completed: false, reportedAtUnixMs: NOW - DAY }, externalWait: null, commitments: [] },
        { id: 'T-111', title: '导出功能', projectId: 'P-001', ownerPersonId: 'M-002', started: true, agentActive: false, acceptance: [{ evidence: [] }], submittedResultAtUnixMs: null, integrated: false, report: null, externalWait: { party: '甲方', what: '确认导出字段', sinceUnixMs: NOW - 5 * DAY }, commitments: [] },
      ],
    },
    dependencies: {
      coverage: 'complete',
      items: [
        { id: 'D-1', consumerTaskId: 'T-103', providerTaskId: 'T-102', satisfied: false, what: '通过复核的版本', sinceUnixMs: NOW - 2 * DAY },
        { id: 'D-3', consumerTaskId: 'T-106', providerTaskId: 'T-103', satisfied: false, what: '已核验', sinceUnixMs: null },
        ...extraDeps,
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

describe('deriveProjectDependencies', () => {
  it('sorts tied blockers with internal decisions before external waits, pins exact queuedCount, and avoids guessing version numbers', () => {
    const model = makeModel({}, [
      { id: 'D-2', consumerTaskId: 'T-103', providerTaskId: 'T-104', satisfied: false, what: '已核验', sinceUnixMs: NOW - 2 * DAY },
    ]);
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
          missing: { available_version: 'not_recorded', adopted_version: null },
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

    // Blockers sorted descending by behind.count, then internal kind (review -> decision -> external),
    // so 周宁 (decision, behind 1, 1d) precedes 甲方 (external, behind 1, 5d) matching prototype order.
    expect(result.blockersCount).toBe(4);
    expect(result.blockers.map((b) => [b.kind, b.personName ?? b.externalParty, b.behind.count, b.queuedCount, b.lamp, b.destination])).toEqual([
      ['review', '许宁', 5, 3, 'blocked', { kind: 'person', id: 'M-004' }],
      ['decision', '林悦', 3, 1, 'stalled', { kind: 'task', id: 'T-104' }],
      ['decision', '周宁', 1, 1, 'stalled', { kind: 'task', id: 'T-105' }],
      ['external', '甲方', 1, 1, 'stalled', { kind: 'task', id: 'T-111' }],
    ]);
    for (const blocker of result.blockers) {
      expect(blocker.behind.items.length).toBe(blocker.behind.count);
    }

    // Dependency rows never invent version numbers ("第 N 版" or "第 N 版，等复核") and map not_recorded to notRecorded.
    expect(result.rows.map((r) => [r.consumerWorkId, r.providerWorkId, r.status, r.highlighted, r.adopted, r.available])).toEqual([
      ['T-103', 'T-102', 'changed', true, { kind: 'recorded' }, { kind: 'inReview' }],
      ['T-103', 'T-104', 'none', false, { kind: 'notRecorded' }, { kind: 'stalledAnswer' }],
      ['T-106', 'T-103', 'none', false, { kind: 'notRecorded' }, { kind: 'waitingUpstream' }],
    ]);

    // Primary chain selects the recorded commitment without sniffing '-week-' in commitment.id.
    const topChain = result.chains[0];
    expect(topChain.displayNodes.length).toBe(4);
    expect(topChain.displayNodes[0].root).toBe(true);
    expect(topChain.displayNodes[1].task?.id).toBe('T-102');
    expect(topChain.displayNodes[2].task?.id).toBe('T-103');
    expect(topChain.displayNodes[3].kind).toBe('commitment');
    expect(topChain.displayNodes[3].commitment?.title).toBe('订单查询与提交闭环');
    expect(topChain.commitmentProgress).toEqual({ done: 0, total: 4 });
    expect(topChain.impact.count).toBe(5);
    expect(topChain.impact.items.length).toBe(5);
  });

  it('selects project blocked chains deterministically (2 chains when decisions have no downstream, retaining standalone decisions in blockers)', () => {
    const model = makeModel();
    const result = deriveProjectDependencies({ projectKey: 'P-001', model });
    expect(result.chainsComplete).toBe(true);
    expect(result.chainsCount).toBe(2);
    expect(result.chains.map((c) => c.chain.root.kind)).toEqual(['review', 'external']);
    // Standalone decision chains are excluded from the top chain switcher, while all 4 remain in blockers.
    expect(result.blockersCount).toBe(4);
    expect(result.derived?.chains.filter((c) => !isProjectBlockedChain(c)).length).toBe(2);
  });

  it('honors fixed_delivery policy (shows ok instead of changed when upstream has a newer version)', () => {
    const v1 = { receipt_id: 'r1', contract_hash: 'h1', source_snapshot_id: 's1' };
    const v2 = { receipt_id: 'r2', contract_hash: 'h2', source_snapshot_id: 's1' };
    const deps: DependencyListData = {
      work_id: null,
      items: [
        makeDep({
          work_id: 'W-PIN',
          provider_work_id: 'P-1',
          version_policy: 'fixed_delivery',
          adopted_version: v1,
          available_version: v2,
          validity: 'valid',
          reason: null,
          missing: {},
        }),
        makeDep({
          work_id: 'W-CUR',
          provider_work_id: 'P-1',
          dependency_position: 2,
          version_policy: 'current_contract',
          adopted_version: v1,
          available_version: v2,
          validity: 'valid',
          reason: null,
          missing: {},
        }),
      ],
      total: 2,
      next_cursor: null,
      as_of: { at_unix_ms: NOW },
    };

    const result = deriveProjectDependencies({ projectKey: 'P-001', dependencies: deps });
    expect(result.rows[0].status).toBe('ok');
    expect(result.rows[0].highlighted).toBe(false);
    expect(result.rows[0].adopted).toEqual({ kind: 'differsFromAvailable' });
    expect(result.rows[0].available).toEqual({ kind: 'recorded' });
    // Contrast: current_contract with mismatched versions IS changed.
    expect(result.rows[1].status).toBe('changed');
    expect(result.rows[1].highlighted).toBe(true);
  });

  it('protects permission_restricted available_version even when providerTask exists in overview-model', () => {
    const model = makeModel();
    const deps: DependencyListData = {
      work_id: null,
      items: [
        makeDep({
          work_id: 'T-103',
          provider_work_id: 'T-102', // T-102 is in 'review' in model!
          provider_title: null,
          available_version: null,
          adopted_version: null,
          validity: 'unavailable',
          reason: 'permission_restricted',
          missing: {
            required_version: 'permission_restricted',
            available_version: 'permission_restricted',
            adopted_version: 'permission_restricted',
          },
        }),
      ],
      total: 1,
      next_cursor: null,
      as_of: { at_unix_ms: NOW },
    };

    const result = deriveProjectDependencies({ projectKey: 'P-001', model, dependencies: deps });
    expect(result.rows[0].status).toBe('restricted');
    expect(result.rows[0].adopted).toEqual({ kind: 'restricted' });
    expect(result.rows[0].available).toEqual({ kind: 'restricted' });
  });

  it('derives ok, unadopted, restricted, notRecorded, and unknown dependency rows honestly', () => {
    const v1 = { receipt_id: 'r1', contract_hash: 'h1', source_snapshot_id: 's1' };
    const deps: DependencyListData = {
      work_id: null,
      items: [
        makeDep({ work_id: 'W-1', provider_work_id: 'P-1', adopted_version: v1, available_version: v1, validity: 'valid', reason: null, missing: {} }),
        makeDep({ work_id: 'W-2', provider_work_id: 'P-1', dependency_position: 2, adopted_version: null, available_version: v1, validity: 'valid', reason: 'adoption_not_recorded', missing: { adopted_version: 'not_recorded' } }),
        makeDep({ work_id: 'W-3', provider_work_id: 'P-2', provider_title: null, dependency_position: 3, validity: 'unavailable', reason: 'permission_restricted', missing: { required_version: 'permission_restricted', available_version: 'permission_restricted', adopted_version: 'permission_restricted' } }),
        makeDep({ work_id: 'W-4', provider_work_id: 'P-3', dependency_position: 4, validity: 'unexpected' as unknown as DependencyObservation['validity'], missing: { available_version: 'not_recorded', adopted_version: 'not_recorded' } }),
      ],
      total: 4,
      next_cursor: null,
      as_of: { at_unix_ms: NOW },
    };
    const result = deriveProjectDependencies({ projectKey: 'P-001', dependencies: deps });
    expect(result.rows.map((r) => [r.status, r.adopted.kind, r.available.kind])).toEqual([
      ['ok', 'matchesAvailable', 'recorded'],
      ['unadopted', 'notRecorded', 'recorded'],
      ['restricted', 'restricted', 'restricted'],
      ['unknown', 'notRecorded', 'notRecorded'],
    ]);
    expect(result.coverage.hasUnknownFacts).toBe(true);
  });

  it('keeps unknown reviewers separate and marks counts unknown when ordering is incomparable', () => {
    const model = makeModel({ ordering: 'incomparable' });
    const result = deriveProjectDependencies({ projectKey: 'P-001', model });
    expect(result.chainsCount).toBeNull();
    expect(result.chainsComplete).toBe(false);
    expect(result.blockersCount).toBeNull();
    expect(result.blockersComplete).toBe(false);
    expect(result.coverage.orderingIncomparable).toBe(true);
    expect(result.coverage.hasUnknownFacts).toBe(true);
    for (const blocker of result.blockers) {
      expect(blocker.behind.count).toBeNull();
    }

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
    expect(anonBlockers.items[0].queuedCount).toBe(1);
    expect(anonBlockers.items[0].destination).toEqual({ kind: 'task', id: 'T-1' });
    expect(anonBlockers.items[1].destination).toEqual({ kind: 'task', id: 'T-2' });
  });

  it('attaches related open review decisions without inventing sole-reviewer claims, and only names responder when strictly one responder is allowed', () => {
    const model = makeModel();
    model.input.decisions = {
      coverage: 'complete',
      items: [
        ...(model.input.decisions.items ?? []),
        { id: 'Q-3', taskId: 'T-102', state: 'open', current: true, question: '能不能多一位复核人', allowedResponderIds: ['M-010'], createdAtUnixMs: NOW - DAY },
      ],
    };
    const singleResponderResult = deriveProjectDependencies({
      projectKey: 'P-001',
      model,
    });
    expect(singleResponderResult.chains[0].relatedOpenDecision).toEqual({
      id: 'Q-3',
      question: '能不能多一位复核人',
      responderName: '部门负责人',
    });

    // When two responders are allowed, never pick list[0] by order.
    const multiResponderModel = makeModel();
    multiResponderModel.input.decisions = {
      coverage: 'complete',
      items: [
        { id: 'Q-3', taskId: 'T-102', state: 'open', current: true, question: '能不能多一位复核人', allowedResponderIds: ['M-010', 'M-001'], createdAtUnixMs: NOW - DAY },
      ],
    };
    const multiResponderResult = deriveProjectDependencies({
      projectKey: 'P-001',
      model: multiResponderModel,
    });
    expect(multiResponderResult.chains[0].relatedOpenDecision).toEqual({
      id: 'Q-3',
      question: '能不能多一位复核人',
      responderName: null,
    });
  });

  it('distinguishes answer vs decision only when there is strictly one allowed responder (1 owner, 1 non-owner, 2 responders, 0 responders)', () => {
    const derived = deriveOverview(myWorkInput({
      options: { nowUnixMs: NOW, stalledAfterDays: 2 },
      members: {
        coverage: 'complete',
        items: [
          { personId: 'M-001', displayName: '周宁' },
          { personId: 'M-002', displayName: '林悦' },
        ],
      },
      tasks: {
        coverage: 'complete',
        items: [
          { id: 'T-1', title: '自答任务', ownerPersonId: 'M-002', started: true, externalWait: null, acceptance: [] },
          { id: 'T-2', title: '他人决定任务', ownerPersonId: 'M-002', started: true, externalWait: null, acceptance: [] },
          { id: 'T-3', title: '双人回复任务', ownerPersonId: 'M-002', started: true, externalWait: null, acceptance: [] },
          { id: 'T-4', title: '无回复人任务', ownerPersonId: 'M-002', started: true, externalWait: null, acceptance: [] },
        ],
      },
      decisions: {
        coverage: 'complete',
        items: [
          { id: 'Q-own', taskId: 'T-1', state: 'open', current: true, question: '问题一', allowedResponderIds: ['M-002'], createdAtUnixMs: NOW - DAY },
          { id: 'Q-other', taskId: 'T-2', state: 'open', current: true, question: '问题二', allowedResponderIds: ['M-001'], createdAtUnixMs: NOW - DAY },
          { id: 'Q-two', taskId: 'T-3', state: 'open', current: true, question: '问题三', allowedResponderIds: ['M-001', 'M-002'], createdAtUnixMs: NOW - DAY },
          { id: 'Q-none', taskId: 'T-4', state: 'open', current: true, question: '问题四', allowedResponderIds: [], createdAtUnixMs: NOW - DAY },
        ],
      },
    }));

    const byTask = new Map(derived.waits.map((w) => [w.task.id, w]));
    expect(soleDecisionResponder(byTask.get('T-1'))).toEqual({
      person: { id: 'M-002', displayName: '林悦' },
      isOwnTask: true,
    });
    expect(soleDecisionResponder(byTask.get('T-2'))).toEqual({
      person: { id: 'M-001', displayName: '周宁' },
      isOwnTask: false,
    });
    expect(soleDecisionResponder(byTask.get('T-3'))).toBeNull();
    expect(soleDecisionResponder(byTask.get('T-4'))).toBeNull();

    const blockers = deriveProjectBlockers(derived, false).items;
    const blockerByTask = new Map(blockers.map((b) => [b.destination?.id, b]));
    expect(blockerByTask.get('T-1')).toMatchObject({ personId: 'M-002', personName: '林悦', isOwnTaskAnswer: true });
    expect(blockerByTask.get('T-2')).toMatchObject({ personId: 'M-001', personName: '周宁', isOwnTaskAnswer: false });
    expect(blockerByTask.get('T-3')).toMatchObject({ personId: null, personName: null, isOwnTaskAnswer: false });
    expect(blockerByTask.get('T-4')).toMatchObject({ personId: null, personName: null, isOwnTaskAnswer: false });
  });

  it('keeps real commitment titles like MVP-2 and only treats empty, commitment.id, or projectKey titles as missing', () => {
    expect(hasRecordedCommitmentTitle({ id: 'comm-mvp', title: 'MVP-2', kind: 'project' }, 'P-001')).toBe(true);
    expect(hasRecordedCommitmentTitle({ id: 'P-001-week-47', title: '订单查询与提交闭环', kind: 'project' }, 'P-001')).toBe(true);
    expect(hasRecordedCommitmentTitle({ id: 'comm-empty', title: '   ', kind: 'project' }, 'P-001')).toBe(false);
    expect(hasRecordedCommitmentTitle({ id: 'GOAL-001', title: 'GOAL-001', kind: 'goal' }, 'P-001')).toBe(false);
    expect(hasRecordedCommitmentTitle({ id: 'project-P-001', title: 'P-001', kind: 'project' }, 'P-001')).toBe(false);
  });

  it('marks chainsCount null when chain reads are incomplete even if confirmed chains exist', () => {
    const model = makeModel();
    model.input.decisions = { coverage: 'partial', items: [], reason: 'truncated' };
    model.coverage = {
      ...model.coverage,
      decisions: { status: 'partial', missing: [{ op: 'decisions.list', reason: 'truncated' }] },
    };
    const result = deriveProjectDependencies({ projectKey: 'P-001', model });
    expect(result.chains.length).toBeGreaterThanOrEqual(1);
    expect(result.chainsComplete).toBe(false);
    expect(result.chainsCount).toBeNull();
  });

  it('merges freshness conservatively: failed drops timestamps, and measured/stale keep the oldest timestamp', () => {
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
    // Authoritative is failed with NO timestamp; page_read takes oldest (2000, not 3000).
    expect(mergeDependenciesFreshness(a, b)).toEqual({
      report_received: { status: 'stale', at_unix_ms: 500 },
      authoritative: { status: 'failed' },
      page_read: { status: 'measured', at_unix_ms: 2000 },
    });

    // Even a single source with a timestamp attached to a failed plane strips the timestamp.
    expect(mergeDependenciesFreshness({
      report_received: { status: 'failed', at_unix_ms: 9000 },
      authoritative: { status: 'stale', at_unix_ms: 4000 },
      page_read: { status: 'measured', at_unix_ms: 5000 },
    }, {
      report_received: { status: 'measured', at_unix_ms: 8000 },
      authoritative: { status: 'stale', at_unix_ms: 1500 },
      page_read: { status: 'measured', at_unix_ms: 6000 },
    })).toEqual({
      report_received: { status: 'failed' },
      authoritative: { status: 'stale', at_unix_ms: 1500 },
      page_read: { status: 'measured', at_unix_ms: 5000 },
    });
  });
});
