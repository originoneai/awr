import { describe, expect, it } from 'vitest';
import type { ProjectListModel } from './project-list-model';
import {
  deriveProjectAcceptance,
  hasDeliveredOutcome,
  normalizeAcceptanceDetail,
  type RoundsReadState,
  type SnapshotReadState,
} from './project-acceptance-model';
import type { ReviewRoundsData, TeamFreshness, TeamOverviewResponse, TeamWork, WorkSnapshotData } from '../../lib/types';

const NOW = 1795089600000;
const DAY = 86_400_000;

const MEASURED_FRESHNESS: TeamFreshness = {
  report_received: { status: 'measured', at_unix_ms: NOW },
  authoritative: { status: 'measured', at_unix_ms: NOW },
  page_read: { status: 'measured', at_unix_ms: NOW },
};

function makeWork(key: string, title: string, owner: unknown = null): TeamWork {
  return {
    key,
    title,
    status: 'active',
    owner_person: owner,
    agent: null,
    next_step: null,
    detail_loaded: false,
    workstream_id: 'WS-1',
    contract_hash: 'c'.repeat(64),
  };
}

function makeOverview(works: TeamWork[], overrides: Partial<TeamOverviewResponse> = {}): TeamOverviewResponse {
  return {
    ok: true,
    project: 'P-001',
    works,
    workstreams: [{ id: 'WS-1', title: 'Backend' }],
    coverage: {
      scope: { status: 'succeeded', enumerated: works.length, truncated: false },
      streams: [],
      works_enumerated: works.length,
      works_total: works.length,
      succeeded: 1,
      failed: 0,
      truncated: false,
    },
    freshness: MEASURED_FRESHNESS,
    ...overrides,
  };
}

function makeSnapshot(workId: string, overrides: Partial<WorkSnapshotData> = {}): WorkSnapshotData {
  return {
    work_id: workId,
    contract_hash: 'c'.repeat(64),
    observation: {
      responsibility: { owner_name: '陈宇', independent_reviewer: { person_id: 'M-004', display_name: '许宁' } },
      collaboration: {
        delivery_required: true,
        candidate: { current: true, digest: 'd'.repeat(64), selection_version: '2', required_checks: ['单元测试'] },
        review: { id: `round-${workId}`, state: 'approved', current_candidate: true },
        integration: { id: `integ-${workId}`, state: 'confirmed', current_binding: true },
        verification: [{ check: '单元测试', outcome: 'passed' }],
      },
    },
    ...overrides,
  };
}

function makeModel(overrides: Partial<ProjectListModel> = {}): ProjectListModel {
  const complete = <T,>(items: T[]) => ({ coverage: 'complete' as const, items });
  return {
    ok: true,
    project: 'P-001',
    read_at_unix_ms: NOW,
    input: {
      tasks: complete([]),
      dependencies: complete([]),
      decisions: complete([]),
      reviewRounds: complete([
        { id: 'round-T-102', taskId: 'T-102', roundIndex: 1, state: 'open', current: true, independent: true, reviewerPersonId: 'M-004', openedAtUnixMs: NOW - 2 * DAY },
      ]),
      handoffs: complete([]),
      approvals: complete([]),
      members: complete([
        { personId: 'M-003', displayName: '陈宇', groupId: 'G-1' },
        { personId: 'M-004', displayName: '许宁', groupId: 'G-2' },
      ]),
      activity: complete([]),
      options: { nowUnixMs: NOW, stalledAfterDays: 1 },
    },
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
    freshness: MEASURED_FRESHNESS,
    ...overrides,
  };
}

describe('project-acceptance-model', () => {
  it('normalizes detail filter values strictly', () => {
    expect(normalizeAcceptanceDetail('approved')).toBe('approved');
    expect(normalizeAcceptanceDetail('open')).toBe('open');
    expect(normalizeAcceptanceDetail('rejected')).toBe('rejected');
    expect(normalizeAcceptanceDetail('all')).toBe('all');
    expect(normalizeAcceptanceDetail('bogus')).toBe('all');
    expect(normalizeAcceptanceDetail(null)).toBe('all');
  });

  it('detects delivered outcomes from candidate, review, or verification without including undelivered tasks', () => {
    expect(hasDeliveredOutcome(undefined)).toBe(false);
    expect(hasDeliveredOutcome({ work_id: 'T-103', observation: { collaboration: { delivery_required: false, candidate: undefined, review: null, verification: [] } } })).toBe(false);
    expect(hasDeliveredOutcome(makeSnapshot('T-101'))).toBe(true);
    expect(hasDeliveredOutcome({
      work_id: 'T-RO',
      observation: { collaboration: { delivery_required: false, candidate: undefined, review: { id: 'r1', state: 'approved' }, verification: [] } },
    })).toBe(true);
  });

  it('derives exact summary counts matching filtered row arrays, rejected highlight, and open wait days', () => {
    const works = [
      makeWork('T-101', '订单查询接口', '陈宇'),
      makeWork('T-102', '接口第 3 版', '陈宇'),
      makeWork('T-103', '订单页面联调', '林悦'),
      makeWork('T-105', '登录异常处理', '陈宇'),
    ];
    const snapshotsByKey = new Map<string, WorkSnapshotData | undefined>([
      ['T-101', makeSnapshot('T-101')],
      ['T-102', makeSnapshot('T-102', {
        observation: {
          responsibility: { owner_name: '陈宇', independent_reviewer: { person_id: 'M-004', display_name: '许宁' } },
          collaboration: {
            delivery_required: true,
            candidate: { current: true, selection_version: '3' },
            review: { id: 'round-T-102', state: 'open', current_candidate: true },
            integration: null,
            verification: [{ check: '单元测试', outcome: 'passed' }, { check: '接口约定检查', outcome: 'passed' }],
          },
        },
      })],
      ['T-103', { work_id: 'T-103', observation: { collaboration: { delivery_required: false, candidate: undefined, review: null, verification: [] } } }],
      ['T-105', makeSnapshot('T-105', {
        observation: {
          responsibility: { owner_name: '陈宇', independent_reviewer: { person_id: 'M-004', display_name: '许宁' } },
          collaboration: {
            delivery_required: true,
            candidate: { current: true, selection_version: '1' },
            review: { id: 'round-T-105', state: 'rejected', current_candidate: true },
            integration: null,
            verification: [{ check: '单元测试', outcome: 'passed' }],
          },
        },
      })],
    ]);
    const snapshotStatusByKey = new Map<string, SnapshotReadState>(works.map((w) => [w.key, 'ready']));
    const reviewRoundsByKey = new Map<string, ReviewRoundsData | undefined>([
      ['T-101', {
        work_id: 'T-101',
        total: 2,
        next_cursor: null,
        as_of: { at_unix_ms: NOW },
        items: [
          { round_id: 'round-T-101-1', round_index: 1, round_position: 1, readable: true, state: 'rejected', contract_matches_current: false, reviewer_role: 'independent_reviewer', reviewer_actor_id: 'M-004-agent', reviewer_person_id: 'M-004', decided_at_unix_ms: NOW - 4 * DAY, reason_first_line: '压测不达标' },
          { round_id: 'round-T-101', round_index: 2, round_position: 2, readable: true, state: 'approved', contract_matches_current: true, reviewer_role: 'independent_reviewer', reviewer_actor_id: 'M-004-agent', reviewer_person_id: 'M-004', decided_at_unix_ms: NOW - 3 * DAY, reason_first_line: '加缓存层后达标' },
        ],
      }],
      ['T-102', {
        work_id: 'T-102',
        total: 1,
        next_cursor: null,
        as_of: { at_unix_ms: NOW },
        items: [
          { round_id: 'round-T-102', round_index: 1, round_position: 1, readable: true, state: 'open', contract_matches_current: true, reviewer_role: null, reviewer_actor_id: null, reviewer_person_id: null, decided_at_unix_ms: null, reason_first_line: null },
        ],
      }],
      ['T-105', {
        work_id: 'T-105',
        total: 1,
        next_cursor: null,
        as_of: { at_unix_ms: NOW },
        items: [
          { round_id: 'round-T-105', round_index: 1, round_position: 1, readable: true, state: 'rejected', contract_matches_current: true, reviewer_role: 'independent_reviewer', reviewer_actor_id: 'M-004-agent', reviewer_person_id: 'M-004', decided_at_unix_ms: NOW - DAY, reason_first_line: '错误提示分支没有验证' },
        ],
      }],
    ]);
    const reviewRoundsStatusByKey = new Map<string, RoundsReadState>([
      ['T-101', 'ready'],
      ['T-102', 'ready'],
      ['T-105', 'ready'],
    ]);

    const result = deriveProjectAcceptance({
      projectKey: 'P-001',
      overview: makeOverview(works),
      visibleWorks: works,
      snapshotsByKey,
      snapshotStatusByKey,
      reviewRoundsByKey,
      reviewRoundsStatusByKey,
      model: makeModel(),
      nowUnixMs: NOW,
    });

    expect(result.counts.total.count).toBe(3);
    expect(result.counts.total.items).toHaveLength(3);
    expect(result.counts.approved.count).toBe(1);
    expect(result.counts.approved.items).toHaveLength(1);
    expect(result.counts.open.count).toBe(1);
    expect(result.counts.open.items).toHaveLength(1);
    expect(result.counts.rejected.count).toBe(1);
    expect(result.counts.rejected.items).toHaveLength(1);

    const [r101, r102, r105] = result.rows;
    expect(r101.version).toEqual({ kind: 'numbered', number: 2 });
    expect(r101.integration).toBe('integrated');
    expect(r101.highlighted).toBe(false);
    expect(r101.latestReview.roundIndex).toBe(2);
    expect(r101.latestReview.reviewerName).toBe('许宁');
    expect(r101.latestReview.reviewerRole).toBe('independent_reviewer');
    expect(r101.latestReview.reasonFirstLine).toBe('加缓存层后达标');

    expect(r102.version).toEqual({ kind: 'numbered', number: 3 });
    expect(r102.integration).toBe('awaiting_review');
    expect(r102.latestReview.waitDays).toBe(2);
    expect(r102.latestReview.reviewerName).toBe('许宁');
    expect(r102.latestReview.reviewerRole).toBe('independent_reviewer');

    expect(r105.highlighted).toBe(true);
    expect(r105.integration).toBe('not_integrated');
    expect(r105.latestReview.reasonFirstLine).toBe('错误提示分支没有验证');
    expect(result.coverage.hasUnknownFacts).toBe(false);
  });

  it('keeps aggregate counts unknown (null) when any work snapshot fails or overview is partial', () => {
    const works = [makeWork('T-101', '订单查询接口'), makeWork('T-102', '接口第 3 版')];
    const snapshotsByKey = new Map<string, WorkSnapshotData | undefined>([['T-101', makeSnapshot('T-101')]]);
    const snapshotStatusByKey = new Map<string, SnapshotReadState>([
      ['T-101', 'ready'],
      ['T-102', 'failed'],
    ]);

    const result = deriveProjectAcceptance({
      projectKey: 'P-001',
      overview: makeOverview(works),
      visibleWorks: works,
      snapshotsByKey,
      snapshotStatusByKey,
      reviewRoundsByKey: new Map(),
      reviewRoundsStatusByKey: new Map(),
      model: makeModel(),
      nowUnixMs: NOW,
    });

    expect(result.counts.total.count).toBeNull();
    expect(result.counts.total.items).toHaveLength(1);
    expect(result.counts.approved.count).toBeNull();
    expect(result.counts.approved.items).toHaveLength(1);
    expect(result.coverage.snapshotsFailed).toBe(1);
    expect(result.coverage.allSnapshotsFailed).toBe(false);
  });

  it('never leaks raw machine IDs as reviewer names and keeps restricted rounds hidden', () => {
    const works = [makeWork('T-101', 'Filter API')];
    const snapshotsByKey = new Map<string, WorkSnapshotData | undefined>([
      ['T-101', makeSnapshot('T-101', {
        observation: {
          responsibility: { owner_name: 'lin-agent-native-20261101T045929Z', independent_reviewer: null },
          collaboration: {
            delivery_required: true,
            candidate: { current: true, selection_version: '1' },
            review: { id: 'round-1', state: 'approved' },
            integration: { id: 'i1', state: 'confirmed', current_binding: true },
            verification: [{ check: 'local_git.manifest', outcome: 'passed' }],
          },
        },
      })],
    ]);
    const reviewRoundsByKey = new Map<string, ReviewRoundsData | undefined>([
      ['T-101', {
        work_id: 'T-101',
        total: 1,
        next_cursor: null,
        as_of: { at_unix_ms: NOW },
        items: [{
          round_id: 'round-1',
          round_index: 1,
          round_position: 1,
          readable: false,
          state: null,
          contract_matches_current: null,
          reviewer_role: null,
          reviewer_actor_id: 'chen-agent-native-12345',
          reviewer_person_id: 'chen',
          decided_at_unix_ms: null,
          reason_first_line: 'secret-reason',
        }],
      }],
    ]);

    const result = deriveProjectAcceptance({
      projectKey: 'P-001',
      overview: makeOverview(works),
      visibleWorks: works,
      snapshotsByKey,
      snapshotStatusByKey: new Map([['T-101', 'ready']]),
      reviewRoundsByKey,
      reviewRoundsStatusByKey: new Map([['T-101', 'ready']]),
      nowUnixMs: NOW,
    });

    expect(result.rows[0].submitterName).toBeNull();
    expect(result.rows[0].latestReview.restricted).toBe(true);
    expect(result.rows[0].latestReview.reviewerName).toBeNull();
    expect(result.rows[0].latestReview.reasonFirstLine).toBeNull();
  });

  it('marks unknown facts when ordering is incomparable or review/integration state is unknown', () => {
    const works = [makeWork('T-101', 'Order query')];
    const snapshotsByKey = new Map<string, WorkSnapshotData | undefined>([
      ['T-101', makeSnapshot('T-101', {
        observation: {
          collaboration: {
            delivery_required: true,
            candidate: { current: true, selection_version: '1' },
            review: { id: 'r1', state: 'mystery_state' },
            integration: { id: 'i1', state: 'unknown', current_binding: true },
            verification: [],
          },
        },
      })],
    ]);

    const result = deriveProjectAcceptance({
      projectKey: 'P-001',
      overview: makeOverview(works),
      visibleWorks: works,
      snapshotsByKey,
      snapshotStatusByKey: new Map([['T-101', 'ready']]),
      reviewRoundsByKey: new Map(),
      reviewRoundsStatusByKey: new Map(),
      model: makeModel({ ordering: 'incomparable' }),
      nowUnixMs: NOW,
    });

    expect(result.counts.total.count).toBeNull();
    expect(result.counts.approved.count).toBeNull();
    expect(result.rows[0].latestReview.state).toBe('unknown');
    expect(result.rows[0].integration).toBe('unknown');
    expect(result.coverage.orderingIncomparable).toBe(true);
    expect(result.coverage.hasUnknownFacts).toBe(true);
  });

  it('orders rows by overview-model task order when available and does not flag unknown facts from overview list freshness alone', () => {
    const works = [makeWork('T-107', 'Pagination'), makeWork('T-101', 'Order API'), makeWork('T-102', 'Submit API')];
    const snapshotsByKey = new Map<string, WorkSnapshotData | undefined>([
      ['T-107', makeSnapshot('T-107')],
      ['T-101', makeSnapshot('T-101')],
      ['T-102', makeSnapshot('T-102')],
    ]);
    const baseModel = makeModel();
    const model: ProjectListModel = {
      ...baseModel,
      input: {
        ...baseModel.input,
        tasks: {
          coverage: 'complete',
          items: [
            { id: 'T-101', title: 'Order API' },
            { id: 'T-102', title: 'Submit API' },
            { id: 'T-107', title: 'Pagination' },
          ],
        },
      },
    };
    const overview = makeOverview(works, {
      freshness: {
        report_received: { status: 'not_measured' },
        authoritative: { status: 'not_measured' },
        page_read: { status: 'measured', at_unix_ms: NOW },
      },
    });

    const result = deriveProjectAcceptance({
      projectKey: 'P-001',
      overview,
      visibleWorks: works,
      snapshotsByKey,
      snapshotStatusByKey: new Map([['T-107', 'ready'], ['T-101', 'ready'], ['T-102', 'ready']]),
      reviewRoundsByKey: new Map(),
      reviewRoundsStatusByKey: new Map(),
      model,
      nowUnixMs: NOW,
    });

    expect(result.rows.map((row) => row.workKey)).toEqual(['T-101', 'T-102', 'T-107']);
    expect(result.coverage.hasUnknownFacts).toBe(false);
    expect(result.freshness?.authoritative.status).toBe('measured');
  });
});
