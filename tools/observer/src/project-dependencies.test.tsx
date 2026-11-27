import { act, cleanup, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { axe, toHaveNoViolations } from 'jest-axe';
import { afterEach, describe, expect, it, vi } from 'vitest';
import i18n from './i18n';
import { defaultVisibleText, findForbiddenVisibleText, findHumanLanguageViolations } from './lib/human-language-guard';
import { revokeObservationReads } from './lib/queries';
import type { ProjectListModel } from './components/projects/project-list-model';
import { emptyOverviewModelResponse } from './test-fixtures/overview-model';
import { realShapedOverview } from './test-fixtures/real-shaped';
import { myWorkInput } from './test-fixtures/workbench';
import { DEMO_PROJECTS, renderApp } from './test-utils';

expect.extend(toHaveNoViolations);

const NOW = Date.UTC(2026, 10, 19, 10, 0, 0);
const DAY = 86_400_000;
const RECEIPT_V1 = '01J000000000000000000000V1';
const RECEIPT_V2 = '01J000000000000000000000V2';
const DIGEST = 'f'.repeat(64);

function json(payload: unknown, status = 200) {
  return new Response(JSON.stringify(payload), {
    status,
    headers: { 'content-type': 'application/json' },
  });
}

function makeProjects() {
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

function makeOverview() {
  return {
    ok: true,
    project: 'demo',
    works: [
      { key: 'T-102', title: '接口第 3 版：提交与错误反馈', status: 'in_progress', owner_person: '陈宇', agent: null, next_step: null, detail_loaded: false, workstream_id: 'WS-BE', contract_hash: DIGEST },
      { key: 'T-103', title: '订单页面联调', status: 'in_progress', owner_person: '林悦', agent: null, next_step: null, detail_loaded: false, workstream_id: 'WS-FE', contract_hash: DIGEST },
      { key: 'T-104', title: '空状态检查', status: 'in_progress', owner_person: '林悦', agent: null, next_step: null, detail_loaded: false, workstream_id: 'WS-FE', contract_hash: DIGEST },
      { key: 'T-106', title: '业务验收', status: 'planned', owner_person: '许宁', agent: null, next_step: null, detail_loaded: false, workstream_id: 'WS-QA', contract_hash: DIGEST },
    ],
    workstreams: [
      { id: 'WS-FE', title: '前端' },
      { id: 'WS-BE', title: '后端' },
      { id: 'WS-QA', title: '验证' },
    ],
    coverage: {
      scope: { status: 'succeeded', enumerated: 4, truncated: false, next_cursor: null },
      streams: [],
      works_enumerated: 4,
      works_total: 4,
      succeeded: 4,
      failed: 0,
      truncated: false,
    },
    freshness: {
      report_received: { status: 'measured', at_unix_ms: NOW - 60_000 },
      authoritative: { status: 'measured', at_unix_ms: NOW },
      page_read: { status: 'measured', at_unix_ms: NOW },
    },
  };
}

function makeDirectory(withDuties = false) {
  return {
    ok: true,
    data: {
      configured: true,
      members: [
        { personId: 'M-001', available: true, ...(withDuties ? { duty: '小组负责人' } : {}) },
        { personId: 'M-002', available: true, ...(withDuties ? { duty: '开发' } : {}) },
        { personId: 'M-003', available: true, ...(withDuties ? { duty: '开发' } : {}) },
        { personId: 'M-004', available: true, ...(withDuties ? { duty: '复核' } : {}) },
      ],
    },
  };
}

function makeRichModel(overrides: Partial<ProjectListModel> = {}): ProjectListModel {
  const input = myWorkInput({
    options: { nowUnixMs: NOW, stalledAfterDays: 2, views: { me: { personId: 'M-002', projectId: 'demo' } } },
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
        { id: 'T-102', title: '接口第 3 版：提交与错误反馈', projectId: 'demo', ownerPersonId: 'M-003', started: true, agentName: 'Codex', agentActive: false, acceptance: [{ evidence: [{ passed: true, current: true }] }], submittedResultAtUnixMs: NOW - 2 * DAY, integrated: false, report: { completed: false, reportedAtUnixMs: NOW - 2 * DAY }, externalWait: null, commitments: [{ id: 'comm-orders-closure', title: '订单查询与提交闭环', kind: 'project' }] },
        { id: 'T-103', title: '订单页面联调', projectId: 'demo', ownerPersonId: 'M-002', started: true, agentName: 'Claude 桌面', agentActive: true, acceptance: [{ evidence: [] }], submittedResultAtUnixMs: null, integrated: false, report: { completed: false, reportedAtUnixMs: NOW - 60_000 }, externalWait: null, commitments: [{ id: 'comm-orders-closure', title: '订单查询与提交闭环', kind: 'project' }] },
        { id: 'T-104', title: '空状态检查', projectId: 'demo', ownerPersonId: 'M-002', started: true, agentName: 'Codex', agentActive: false, acceptance: [{ evidence: [] }], submittedResultAtUnixMs: null, integrated: false, report: { completed: false, reportedAtUnixMs: NOW - 2 * DAY }, externalWait: null, commitments: [{ id: 'comm-orders-closure', title: '订单查询与提交闭环', kind: 'project' }] },
        { id: 'T-105', title: '登录异常处理', projectId: 'demo', ownerPersonId: 'M-003', started: true, agentName: 'Codex', agentActive: false, acceptance: [{ evidence: [] }], submittedResultAtUnixMs: NOW - 2 * DAY, integrated: false, report: { completed: false, reportedAtUnixMs: NOW - 2 * DAY }, externalWait: null, commitments: [] },
        { id: 'T-106', title: '业务验收', projectId: 'demo', ownerPersonId: 'M-004', started: false, agentActive: false, acceptance: [{ evidence: [] }], submittedResultAtUnixMs: null, integrated: false, report: null, externalWait: null, commitments: [{ id: 'comm-orders-closure', title: '订单查询与提交闭环', kind: 'project' }] },
        { id: 'T-109', title: '提交失败的错误码表', projectId: 'demo', ownerPersonId: 'M-003', started: true, agentName: 'Codex', agentActive: false, acceptance: [{ evidence: [{ passed: true, current: true }] }], submittedResultAtUnixMs: NOW - DAY, integrated: false, report: { completed: false, reportedAtUnixMs: NOW - DAY }, externalWait: null, commitments: [] },
        { id: 'T-111', title: '导出功能', projectId: 'demo', ownerPersonId: 'M-002', started: true, agentActive: false, acceptance: [{ evidence: [] }], submittedResultAtUnixMs: null, integrated: false, report: null, externalWait: { party: '甲方', what: '确认导出字段', sinceUnixMs: NOW - 5 * DAY }, commitments: [] },
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
        { id: 'R-102-1', taskId: 'T-102', roundIndex: 1, state: 'approved', current: false, independent: true, authorPersonId: 'M-003', reviewerPersonId: 'M-004', openedAtUnixMs: NOW - 4 * DAY },
        { id: 'R-102-2', taskId: 'T-102', roundIndex: 2, state: 'open', current: true, independent: true, authorPersonId: 'M-003', reviewerPersonId: 'M-004', openedAtUnixMs: NOW - 2 * DAY },
        { id: 'R-109-1', taskId: 'T-109', roundIndex: 1, state: 'open', current: true, independent: true, authorPersonId: 'M-003', reviewerPersonId: 'M-004', openedAtUnixMs: NOW - DAY },
      ],
    },
  });
  return {
    ok: true,
    project: 'demo',
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

function makeDepsReply() {
  return {
    ok: true,
    result: {
      data: {
        work_id: null,
        items: [
          {
            work_id: 'T-103',
            work_title: '订单页面联调',
            provider_work_id: 'T-102',
            provider_title: '接口第 3 版：提交与错误反馈',
            dependency_position: 1,
            version_policy: 'current_contract',
            review_assurance: { cross_workstream: { review_assurance: 'team_independent', version_policy: 'current_contract' } },
            required_version: { receipt_id: null, contract_hash: DIGEST, source_snapshot_id: 'snap-1' },
            available_version: { receipt_id: RECEIPT_V2, contract_hash: DIGEST, source_snapshot_id: 'snap-1' },
            adopted_version: { receipt_id: RECEIPT_V1, contract_hash: DIGEST, source_snapshot_id: 'snap-1' },
            consumed_version: { receipt_id: RECEIPT_V1, contract_hash: DIGEST, source_snapshot_id: 'snap-1' },
            adoption_valid: false,
            validity: 'upstream_changed',
            reason: 'upstream_contract_changed',
            missing: {},
          },
          {
            work_id: 'T-103',
            work_title: '订单页面联调',
            provider_work_id: 'T-104',
            provider_title: '空状态检查',
            dependency_position: 2,
            version_policy: 'current_contract',
            review_assurance: null,
            required_version: { receipt_id: null, contract_hash: DIGEST, source_snapshot_id: 'snap-1' },
            available_version: null,
            adopted_version: null,
            consumed_version: null,
            adoption_valid: null,
            validity: 'unavailable',
            reason: 'completion_unavailable',
            missing: { available_version: 'not_recorded', adopted_version: 'not_recorded' },
          },
          {
            work_id: 'T-106',
            work_title: '业务验收',
            provider_work_id: 'T-103',
            provider_title: '订单页面联调',
            dependency_position: 3,
            version_policy: 'current_contract',
            review_assurance: null,
            required_version: { receipt_id: null, contract_hash: DIGEST, source_snapshot_id: 'snap-1' },
            available_version: null,
            adopted_version: null,
            consumed_version: null,
            adoption_valid: null,
            validity: 'unavailable',
            reason: 'completion_unavailable',
            missing: { available_version: 'not_recorded', adopted_version: 'not_recorded' },
          },
        ],
        total: 3,
        next_cursor: null,
        as_of: { at_unix_ms: NOW },
      },
    },
    freshness: {
      report_received: { status: 'not_measured' },
      authoritative: { status: 'measured', at_unix_ms: NOW },
      page_read: { status: 'measured', at_unix_ms: NOW },
    },
  };
}

function makeHandoffsReply() {
  return {
    ok: true,
    result: {
      data: {
        items: [{
          handoff_id: 'H-001',
          work_id: 'T-103',
          work_title: '订单页面联调',
          kind: 'responsibility',
          effective_status: 'proposed',
          pending: true,
          expired: false,
          from_display_name: '周宁',
          to_display_name: '林悦',
          updated_at_unix_ms: NOW - 3600_000,
          expires_at_unix_ms: NOW + DAY,
        }],
        total: 1,
        next_cursor: null,
        as_of: { at_unix_ms: NOW },
      },
    },
    freshness: {
      report_received: { status: 'not_measured' },
      authoritative: { status: 'measured', at_unix_ms: NOW },
      page_read: { status: 'measured', at_unix_ms: NOW },
    },
  };
}

function makeGraphReply() {
  return {
    ok: true,
    project: 'demo',
    read_at_unix_ms: NOW,
    as_of: { project_revision: '12', source_snapshot_id: 'snap-1', coordinator_epoch: 'current' },
    workstreams: [{ id: 'WS-FE', title: '前端' }],
    items: [{
      work_id: 'T-103',
      external_key: 'T-103',
      title: '订单页面联调',
      workstream_id: 'WS-FE',
      state: 'claimed',
      owner: '林悦',
      last_event_at: NOW,
      claims: [],
      dependencies_complete: true,
      required_dependencies: [],
    }],
  };
}

interface StubConfig {
  model?: Response | (() => Promise<Response>);
  deps?: Response | (() => Promise<Response>);
  overview?: Response;
  directory?: Response;
  handoffs?: Response;
  graph?: Response;
}

function stubDepsFetch(cfg: StubConfig = {}) {
  vi.stubGlobal('fetch', vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
    const url = String(input);
    if (url.includes('/api/team/shell')) {
      return json({ ok: true, source: 'live', organizationName: '示例组织', perspective: null });
    }
    if (url.includes('/api/team/projects')) return json(makeProjects());
    if (url.includes('/api/team/directory')) {
      return cfg.directory ? cfg.directory.clone() : json(makeDirectory(false));
    }
    if (url.includes('/api/team/overview-model')) {
      if (typeof cfg.model === 'function') return cfg.model();
      return cfg.model ? cfg.model.clone() : json(makeRichModel());
    }
    if (url.includes('/api/team/overview')) {
      return cfg.overview ? cfg.overview.clone() : json(makeOverview());
    }
    if (url.includes('/api/team/graph')) {
      return cfg.graph ? cfg.graph.clone() : json(makeGraphReply());
    }
    if (url.includes('/api/team/read')) {
      const query = JSON.parse(String(init?.body)).query;
      if (query.op === 'dependencies.list') {
        if (typeof cfg.deps === 'function') return cfg.deps();
        return cfg.deps ? cfg.deps.clone() : json(makeDepsReply());
      }
      if (query.op === 'handoff.list') {
        return cfg.handoffs ? cfg.handoffs.clone() : json(makeHandoffsReply());
      }
    }
    return json({ ok: true, result: { data: { items: [], total: 0, next_cursor: null, as_of: { at_unix_ms: NOW } } } });
  }));
}

afterEach(async () => {
  cleanup();
  vi.unstubAllGlobals();
  await act(async () => { await i18n.changeLanguage('zh-CN'); });
});

describe('ProjectDependencies (AWR-PPO-043)', () => {
  it('renders honest versions (no guessed numbers, not_recorded -> 未记录), exact queue counts, commitment title without invented risk, and tied blocker ordering', async () => {
    const user = userEvent.setup();
    stubDepsFetch();
    const { router } = renderApp('/projects/demo?tab=deps');

    await waitFor(() => expect(screen.getByTestId('project-deps-versions-count').textContent).toBe('3'));
    expect(screen.getByTestId('project-deps-blockers-count').textContent).toBe('4');
    // T-105 standalone decision stays in blockers only; top chains include review, multi-task decision (T-104), and external (T-111).
    expect(screen.getByTestId('project-deps-chain-count').textContent).toContain('3 条链路卡住');

    // Commitment end node shows recorded commitment title ('订单查询与提交闭环') without invented '有风险' or risk lamp (F13).
    const chainsBlock = screen.getByTestId('project-deps-chains');
    const commitmentEndNode = chainsBlock.querySelector('[data-node-kind="commitment"]');
    expect(commitmentEndNode).not.toBeNull();
    expect(commitmentEndNode!.textContent).toContain('订单查询与提交闭环');
    expect(commitmentEndNode!.textContent).toContain('还差 4 件');
    expect(commitmentEndNode!.textContent).not.toContain('有风险');
    expect(commitmentEndNode!.querySelector('.obs-lamp')).toBeNull();
    // Exact review queue count in chain root node (catches off-by-one fault).
    expect(chainsBlock.textContent).toContain('2 件在排队，最久 2 天');
    // Footer reports factual queue impact without fabricating a sole-reviewer claim.
    expect(chainsBlock.textContent).toContain('许宁的复核队列压着 4 件事。');
    expect(chainsBlock.textContent).not.toContain('能复核的只有许宁一位');

    // Left table: recorded deliveries use factual labels (no guessed '第 1 版' / '第 2 版'), not_recorded uses '未记录' (never '还没有').
    const versionsBlock = screen.getByTestId('project-deps-versions');
    const tableRows = within(versionsBlock).getAllByTestId(/^dep-row-/);
    expect(tableRows.length).toBe(3);
    expect(versionsBlock.textContent).toContain('与可用不同');
    expect(versionsBlock.textContent).toContain('已有可用版本');
    expect(versionsBlock.textContent).toContain('未记录');
    expect(versionsBlock.textContent).not.toContain('第 1 版');
    expect(versionsBlock.textContent).not.toContain('第 2 版');
    expect(versionsBlock.textContent).not.toContain('还没有\n');
    expect(versionsBlock.textContent).toContain('上游有新版本');
    expect(versionsBlock.textContent).toContain('还没有可采用的');
    expect(versionsBlock.textContent).toContain('下游只能采用通过复核的那一版。上游出了新版本，下游要重新检查。');
    expect(versionsBlock.textContent).not.toContain(RECEIPT_V1);
    expect(versionsBlock.textContent).not.toContain(RECEIPT_V2);
    expect(versionsBlock.textContent).not.toContain(DIGEST);

    // Right blockers: sorted by behind count desc, then internal before external (周宁 before 甲方 on behind 1).
    const blockerRows = screen.getAllByTestId('project-deps-blocker-row');
    expect(blockerRows.length).toBe(4);
    expect(blockerRows[0].textContent).toContain('许宁的复核队列');
    expect(blockerRows[0].textContent).toContain('2 件在排队，最久 2 天');
    expect(blockerRows[0].textContent).toContain('压着 4 件');
    expect(blockerRows[1].textContent).toContain('等林悦回答一个问题');
    expect(blockerRows[1].textContent).toContain('压着 3 件');
    expect(blockerRows[2].textContent).toContain('等周宁做一个决定');
    expect(blockerRows[2].textContent).toContain('压着 1 件');
    expect(blockerRows[3].textContent).toContain('等甲方确认导出字段');
    expect(blockerRows[3].textContent).toContain('压着 1 件');

    // Clicking "压着 4 件" opens inline detail panel with EXACTLY 4 tasks.
    await user.click(within(blockerRows[0]).getByTestId('project-deps-blocker-behind'));
    const blockerDetail = await screen.findByTestId('project-deps-detail');
    expect(blockerDetail.getAttribute('data-count')).toBe('4');
    expect(within(blockerDetail).getAllByRole('listitem').length).toBe(4);
    await user.click(screen.getByTestId('project-deps-detail-back'));
    expect(screen.queryByTestId('project-deps-detail')).toBeNull();

    // Clicking "3 条链路卡住" opens inline detail panel with EXACTLY 3 chains.
    await user.click(screen.getByTestId('project-deps-chain-count'));
    const chainDetail = await screen.findByTestId('project-deps-detail');
    expect(chainDetail.getAttribute('data-count')).toBe('3');
    expect(within(chainDetail).getAllByRole('listitem').length).toBe(3);
    await user.click(screen.getByTestId('project-deps-detail-back'));

    expect(screen.getByTestId('freshness-badge')).toBeTruthy();
    expect(screen.getByTestId('project-deps-coverage').textContent).toContain('数据读取情况');
    expect(document.querySelector('details')).toBeNull();
    expect(findHumanLanguageViolations(screen.getByTestId('page-project-dependencies'))).toEqual([]);

    // Clicking the review blocker navigates to the reviewer's person page.
    await user.click(within(blockerRows[0]).getByRole('button', { name: /许宁的复核队列/ }));
    expect(router.state.location.pathname).toBe('/people/M-004');
  });

  it('never infers sole-reviewer claims without structured roles, attaches related open decisions, and distinguishes 回答 vs 决定 only for a single responder', async () => {
    const user = userEvent.setup();
    const model = makeRichModel();
    // Add T-112 with a 2-responder open decision that blocks T-106 (impact > 1 -> appears in chain tabs AND blockers),
    // and make T-105 block T-106 so 周宁's single non-owner decision also appears in the chain tabs.
    model.input.tasks = {
      coverage: 'complete',
      items: [
        ...(model.input.tasks.items ?? []),
        { id: 'T-112', title: '多方确认项', projectId: 'demo', ownerPersonId: 'M-003', started: true, agentActive: false, acceptance: [{ evidence: [] }], submittedResultAtUnixMs: null, integrated: false, report: null, externalWait: null, commitments: [] },
      ],
    };
    model.input.dependencies = {
      coverage: 'complete',
      items: [
        ...(model.input.dependencies.items ?? []),
        { id: 'D-4', consumerTaskId: 'T-106', providerTaskId: 'T-105', satisfied: false, what: '已核验', sinceUnixMs: NOW - DAY },
        { id: 'D-5', consumerTaskId: 'T-106', providerTaskId: 'T-112', satisfied: false, what: '已核验', sinceUnixMs: NOW - DAY },
      ],
    };
    model.input.decisions = {
      coverage: 'complete',
      items: [
        ...(model.input.decisions.items ?? []),
        { id: 'Q-3', taskId: 'T-102', state: 'open', current: true, question: '能不能多一位复核人', allowedResponderIds: ['M-010'], createdAtUnixMs: NOW - DAY },
        { id: 'Q-4', taskId: 'T-112', state: 'open', current: true, question: '上线窗口选哪天', allowedResponderIds: ['M-001', 'M-002'], createdAtUnixMs: NOW - DAY },
      ],
    };
    stubDepsFetch({
      model: json(model),
      directory: json(makeDirectory(true)),
    });
    renderApp('/projects/demo?tab=deps');

    const chainsBlock = await screen.findByTestId('project-deps-chains');
    await waitFor(() => expect(chainsBlock.textContent).toContain('相关问题“能不能多一位复核人”还在等部门负责人回答。'));
    // Even when directory returns free-text duty '复核', never claim "能复核的只有许宁一位".
    expect(chainsBlock.textContent).toContain('许宁的复核队列压着 4 件事。');
    expect(chainsBlock.textContent).not.toContain('能复核的只有许宁一位');

    // Tab switcher & root nodes: 1 owner -> 回答, 1 non-owner -> 决定, 2 responders -> neutral 等回复 (never picks list[0]).
    const ownerAnswerTab = within(chainsBlock).getByRole('button', { name: '等林悦回答' });
    const nonOwnerDecideTab = within(chainsBlock).getByRole('button', { name: '等周宁决定' });
    const multiResponderTab = within(chainsBlock).getByRole('button', { name: '等回复' });

    await user.click(ownerAnswerTab);
    expect(chainsBlock.querySelector('.obs-chain-node-root .obs-chain-title')?.textContent).toBe('等林悦回答一个问题');

    await user.click(nonOwnerDecideTab);
    expect(chainsBlock.querySelector('.obs-chain-node-root .obs-chain-title')?.textContent).toBe('等周宁做一个决定');

    await user.click(multiResponderTab);
    expect(chainsBlock.querySelector('.obs-chain-node-root .obs-chain-title')?.textContent).toBe('等回复');
    expect(chainsBlock.querySelector('.obs-chain-node-root .obs-chain-avatar')).toBeNull();

    // Right-column blockers also distinguish 1 owner, 1 non-owner, and 2 responders neutrally.
    const blockerTitles = screen.getAllByTestId('project-deps-blocker-row')
      .map((row) => row.querySelector('.project-deps-blocker-title')?.textContent);
    expect(blockerTitles).toContain('等林悦回答一个问题');
    expect(blockerTitles).toContain('等周宁做一个决定');
    expect(blockerTitles).toContain('等回复');
  });

  it('pins fixed_delivery status protection, permission_restricted available cell protection, and honest in-review cell wording (F12)', async () => {
    stubDepsFetch({
      deps: json({
        ok: true,
        result: {
          data: {
            work_id: null,
            items: [
              {
                work_id: 'T-103',
                work_title: '订单页面联调',
                provider_work_id: 'T-102',
                provider_title: '接口第 3 版：提交与错误反馈',
                dependency_position: 1,
                version_policy: 'fixed_delivery',
                review_assurance: null,
                required_version: { receipt_id: null, contract_hash: DIGEST, source_snapshot_id: 'snap-1' },
                available_version: { receipt_id: RECEIPT_V2, contract_hash: DIGEST, source_snapshot_id: 'snap-1' },
                adopted_version: { receipt_id: RECEIPT_V1, contract_hash: DIGEST, source_snapshot_id: 'snap-1' },
                consumed_version: { receipt_id: RECEIPT_V1, contract_hash: DIGEST, source_snapshot_id: 'snap-1' },
                adoption_valid: true,
                validity: 'valid',
                reason: null,
                missing: {},
              },
              {
                work_id: 'T-106',
                work_title: '业务验收',
                provider_work_id: 'T-102',
                provider_title: null,
                dependency_position: 2,
                version_policy: 'current_contract',
                review_assurance: null,
                required_version: null,
                available_version: null,
                adopted_version: null,
                consumed_version: null,
                adoption_valid: null,
                validity: 'unavailable',
                reason: 'permission_restricted',
                missing: {
                  required_version: 'permission_restricted',
                  available_version: 'permission_restricted',
                  adopted_version: 'permission_restricted',
                },
              },
              {
                work_id: 'T-104',
                work_title: '空状态检查',
                provider_work_id: 'T-102',
                provider_title: '接口第 3 版：提交与错误反馈',
                dependency_position: 3,
                version_policy: 'current_contract',
                review_assurance: null,
                required_version: { receipt_id: null, contract_hash: DIGEST, source_snapshot_id: 'snap-1' },
                available_version: null,
                adopted_version: { receipt_id: RECEIPT_V1, contract_hash: DIGEST, source_snapshot_id: 'snap-1' },
                consumed_version: { receipt_id: RECEIPT_V1, contract_hash: DIGEST, source_snapshot_id: 'snap-1' },
                adoption_valid: false,
                validity: 'upstream_changed',
                reason: 'upstream_changed',
                missing: { available_version: 'not_recorded' },
              },
            ],
            total: 3,
            next_cursor: null,
            as_of: { at_unix_ms: NOW },
          },
        },
        freshness: {
          report_received: { status: 'not_measured' },
          authoritative: { status: 'measured', at_unix_ms: NOW },
          page_read: { status: 'measured', at_unix_ms: NOW },
        },
      }),
    });
    renderApp('/projects/demo?tab=deps');

    const fixedRow = await screen.findByTestId('dep-row-T-103-T-102');
    expect(fixedRow.textContent).toContain('用的是最新的');
    expect(fixedRow.textContent).not.toContain('上游有新版本');

    const restrictedRow = screen.getByTestId('dep-row-T-106-T-102');
    const cells = within(restrictedRow).getAllByRole('cell');
    // Column 3 (adopted) and Column 4 (available) must BOTH say '当前无权查看', even though T-102 is in overview-model!
    expect(cells[2].textContent).toBe('当前无权查看');
    expect(cells[3].textContent).toBe('当前无权查看');

    // F12: when provider T-102 is in review (roundIndex 2 open) and available_version is not_recorded,
    // column 4 must say '等复核' and NEVER fabricate '第 2 版，等复核'.
    const inReviewRow = screen.getByTestId('dep-row-T-104-T-102');
    const inReviewCells = within(inReviewRow).getAllByRole('cell');
    expect(inReviewCells[3].textContent).toBe('等复核');
    expect(inReviewCells[3].textContent).not.toMatch(/第\s*\d+\s*版/);
  });

  it('determines external avatar strictly from wait kind rather than party name suffixes (F15)', async () => {
    const user = userEvent.setup();
    const model = makeRichModel();
    model.input.members = {
      coverage: 'complete',
      items: [
        { personId: 'M-001', displayName: '平台小组' },
        { personId: 'M-002', displayName: '林悦' },
        { personId: 'M-003', displayName: '陈宇' },
        { personId: 'M-004', displayName: '研发部' },
      ],
    };
    model.input.tasks = {
      coverage: 'complete',
      items: [
        { id: 'T-102', title: '接口开发', projectId: 'demo', ownerPersonId: 'M-003', started: true, agentActive: false, acceptance: [{ evidence: [{ passed: true, current: true }] }], submittedResultAtUnixMs: NOW - 2 * DAY, integrated: false, report: { completed: false, reportedAtUnixMs: NOW - 2 * DAY }, externalWait: null, commitments: [] },
        { id: 'T-103', title: '联调任务', projectId: 'demo', ownerPersonId: 'M-002', started: true, agentActive: false, acceptance: [{ evidence: [] }], submittedResultAtUnixMs: null, integrated: false, report: null, externalWait: null, commitments: [] },
        { id: 'T-111', title: '外部联调', projectId: 'demo', ownerPersonId: 'M-002', started: true, agentActive: false, acceptance: [{ evidence: [] }], submittedResultAtUnixMs: null, integrated: false, report: null, externalWait: { party: '王强', what: '确认接口参数', sinceUnixMs: NOW - 3 * DAY }, commitments: [] },
      ],
    };
    model.input.dependencies = {
      coverage: 'complete',
      items: [
        { id: 'D-1', consumerTaskId: 'T-103', providerTaskId: 'T-102', satisfied: false, what: '已核验', sinceUnixMs: NOW - DAY },
      ],
    };
    model.input.decisions = {
      coverage: 'complete',
      items: [
        { id: 'Q-dept', taskId: 'T-102', state: 'open', current: true, question: '字段是否兼容', allowedResponderIds: ['M-001'], createdAtUnixMs: NOW - DAY },
      ],
    };
    model.input.reviewRounds = {
      coverage: 'complete',
      items: [
        { id: 'R-dept', taskId: 'T-102', roundIndex: 1, state: 'open', current: true, independent: true, authorPersonId: 'M-003', reviewerPersonId: 'M-004', openedAtUnixMs: NOW - DAY },
      ],
    };
    stubDepsFetch({ model: json(model) });
    renderApp('/projects/demo?tab=deps');

    const chainsBlock = await screen.findByTestId('project-deps-chains');
    await waitFor(() => expect(within(chainsBlock).getByRole('button', { name: '复核排队' })).toBeTruthy());

    // Review wait whose person name ends with '部' ('研发部') is NOT external.
    await user.click(within(chainsBlock).getByRole('button', { name: '复核排队' }));
    expect(chainsBlock.querySelector('.obs-chain-node-root .obs-chain-title')?.textContent).toBe('研发部的复核队列');
    const reviewAvatar = chainsBlock.querySelector('.obs-chain-node-root .obs-chain-avatar');
    expect(reviewAvatar).not.toBeNull();
    expect(reviewAvatar!.getAttribute('data-external')).toBeNull();

    // Decision wait whose responder name ends with '小组' ('平台小组') is NOT external.
    await user.click(within(chainsBlock).getByRole('button', { name: '等平台小组决定' }));
    expect(chainsBlock.querySelector('.obs-chain-node-root .obs-chain-title')?.textContent).toBe('等平台小组做一个决定');
    const decisionAvatar = chainsBlock.querySelector('.obs-chain-node-root .obs-chain-avatar');
    expect(decisionAvatar).not.toBeNull();
    expect(decisionAvatar!.getAttribute('data-external')).toBeNull();

    // External wait whose party is an ordinary name ('王强') IS external.
    await user.click(within(chainsBlock).getByRole('button', { name: '王强没回复' }));
    expect(chainsBlock.querySelector('.obs-chain-node-root .obs-chain-title')?.textContent).toBe('王强确认接口参数');
    const externalAvatar = chainsBlock.querySelector('.obs-chain-node-root .obs-chain-avatar');
    expect(externalAvatar).not.toBeNull();
    expect(externalAvatar!.getAttribute('data-external')).toBe('true');
  });

  it('keeps real commitment titles like MVP-2, falls back to commitmentProjectFallback when title equals id/projectKey, and never sniffs -week- IDs (F14, F26)', async () => {
    const modelWithMvp = makeRichModel();
    if (modelWithMvp.input.tasks.coverage !== 'unread') {
      modelWithMvp.input.tasks.items.forEach((task) => {
        if (task.commitments && task.commitments.length > 0) {
          task.commitments = [{ id: 'demo-week-47', title: 'MVP-2', kind: 'project' }];
        }
      });
    }
    stubDepsFetch({ model: json(modelWithMvp) });
    renderApp('/projects/demo?tab=deps');

    const chainsMvp = await screen.findByTestId('project-deps-chains');
    await waitFor(() => expect(chainsMvp.querySelector('[data-node-kind="commitment"]')).not.toBeNull());
    const mvpEnd = chainsMvp.querySelector('[data-node-kind="commitment"]')!;
    // Real title 'MVP-2' is preserved; '-week-' ID does not invent '本周承诺' kicker (F14).
    expect(mvpEnd.querySelector('.obs-chain-title')?.textContent).toBe('MVP-2');
    expect(mvpEnd.querySelector('.obs-chain-kicker')?.textContent).toBe('项目承诺');
    expect(mvpEnd.querySelector('.obs-chain-kicker')?.textContent).not.toContain('本周承诺');
    cleanup();

    // Title equal to projectKey ('demo') falls back to neutral commitmentProjectFallback ('项目（还没有名称）'), NOT project's own title ('订单协作平台').
    const modelWithProjectKeyTitle = makeRichModel();
    if (modelWithProjectKeyTitle.input.tasks.coverage !== 'unread') {
      modelWithProjectKeyTitle.input.tasks.items.forEach((task) => {
        if (task.commitments && task.commitments.length > 0) {
          task.commitments = [{ id: 'project-demo', title: 'demo', kind: 'project' }];
        }
      });
    }
    stubDepsFetch({ model: json(modelWithProjectKeyTitle) });
    renderApp('/projects/demo?tab=deps');

    const chainsKey = await screen.findByTestId('project-deps-chains');
    await waitFor(() => expect(chainsKey.querySelector('[data-node-kind="commitment"]')).not.toBeNull());
    const keyEnd = chainsKey.querySelector('[data-node-kind="commitment"]')!;
    expect(keyEnd.querySelector('.obs-chain-title')?.textContent).toBe('项目（还没有名称）');
    expect(keyEnd.textContent).not.toContain('订单协作平台');
    cleanup();

    // Title equal to commitment.id ('P-001') falls back to neutral commitmentProjectFallback (F26).
    const modelWithIdTitle = makeRichModel();
    if (modelWithIdTitle.input.tasks.coverage !== 'unread') {
      modelWithIdTitle.input.tasks.items.forEach((task) => {
        if (task.commitments && task.commitments.length > 0) {
          task.commitments = [{ id: 'P-001', title: 'P-001', kind: 'project' }];
        }
      });
    }
    stubDepsFetch({ model: json(modelWithIdTitle) });
    renderApp('/projects/demo?tab=deps');

    const chainsId = await screen.findByTestId('project-deps-chains');
    await waitFor(() => expect(chainsId.querySelector('[data-node-kind="commitment"]')).not.toBeNull());
    const idEnd = chainsId.querySelector('[data-node-kind="commitment"]')!;
    expect(idEnd.querySelector('.obs-chain-title')?.textContent).toBe('项目（还没有名称）');
    expect(idEnd.textContent).not.toContain('P-001');
  });

  it('never shows calm empty states per block while its own read is loading, failed, or partially covered', async () => {
    // Case A: model resolves empty while dependencies.list is still pending -> table must show loading, NOT empty.
    let resolveDeps!: (res: Response) => void;
    const pendingDeps = new Promise<Response>((resolve) => { resolveDeps = resolve; });
    stubDepsFetch({
      model: json(emptyOverviewModelResponse()),
      deps: () => pendingDeps,
    });
    renderApp('/projects/demo?tab=deps');
    expect(await screen.findByTestId('project-deps-table-loading')).toBeTruthy();
    expect(screen.queryByTestId('project-deps-table-empty')).toBeNull();
    resolveDeps(json({ ok: true, result: { data: { work_id: null, items: [], total: 0, next_cursor: null, as_of: { at_unix_ms: NOW } } }, freshness: { report_received: { status: 'not_measured' }, authoritative: { status: 'measured', at_unix_ms: NOW }, page_read: { status: 'measured', at_unix_ms: NOW } } }));
    expect(await screen.findByTestId('project-deps-table-empty')).toBeTruthy();
    cleanup();

    // Case B: dependencies.list resolves empty while overview-model is still pending -> chains & blockers show loading, NOT calm/empty.
    let resolveModel!: (res: Response) => void;
    const pendingModel = new Promise<Response>((resolve) => { resolveModel = resolve; });
    stubDepsFetch({
      model: () => pendingModel,
      deps: json({ ok: true, result: { data: { work_id: null, items: [], total: 0, next_cursor: null, as_of: { at_unix_ms: NOW } } }, freshness: { report_received: { status: 'not_measured' }, authoritative: { status: 'measured', at_unix_ms: NOW }, page_read: { status: 'measured', at_unix_ms: NOW } } }),
    });
    renderApp('/projects/demo?tab=deps');
    expect(await screen.findByTestId('project-deps-chains-loading')).toBeTruthy();
    expect(screen.getByTestId('project-deps-blockers-loading')).toBeTruthy();
    expect(screen.queryByTestId('project-deps-calm')).toBeNull();
    expect(screen.queryByTestId('project-deps-blockers-empty')).toBeNull();
    resolveModel(json(emptyOverviewModelResponse()));
    expect(await screen.findByTestId('project-deps-calm')).toBeTruthy();
    cleanup();

    // Case C: overview-model fails while dependencies.list succeeds -> chains & blockers show failed (NOT calm), freshness reports failed.
    stubDepsFetch({
      model: json({ ok: false, error: { code: 'Unavailable', message: 'model down' } }, 503),
      deps: json(makeDepsReply()),
    });
    renderApp('/projects/demo?tab=deps');
    expect(await screen.findByTestId('project-deps-chains-failed')).toBeTruthy();
    expect(screen.getByTestId('project-deps-blockers-failed')).toBeTruthy();
    expect(screen.queryByTestId('project-deps-calm')).toBeNull();
    expect(screen.queryByTestId('project-deps-blockers-empty')).toBeNull();
    expect(screen.getByTestId('freshness-authoritative').textContent).toContain('读取失败');
    cleanup();

    // Case D: overview-model succeeds with unread decisions and 0 waits -> chains & blockers show partial naming missing part, NOT calm.
    const partialEmpty = emptyOverviewModelResponse();
    partialEmpty.input.decisions = { coverage: 'unread', reason: 'read-failed' };
    (partialEmpty.coverage as Record<string, { status: string; missing: string[] }>).decisions = { status: 'unread', missing: ['decisions.list'] };
    stubDepsFetch({
      model: json(partialEmpty),
      deps: json({ ok: true, result: { data: { work_id: null, items: [], total: 0, next_cursor: null, as_of: { at_unix_ms: NOW } } }, freshness: { report_received: { status: 'not_measured' }, authoritative: { status: 'measured', at_unix_ms: NOW }, page_read: { status: 'measured', at_unix_ms: NOW } } }),
    });
    renderApp('/projects/demo?tab=deps');
    const chainsPartial = await screen.findByTestId('project-deps-chains-partial');
    expect(chainsPartial.textContent).toContain('等待回答的问题');
    expect(screen.getByTestId('project-deps-blockers-partial').textContent).toContain('等待回答的问题');
    expect(screen.queryByTestId('project-deps-calm')).toBeNull();
    expect(screen.queryByTestId('project-deps-blockers-empty')).toBeNull();
    cleanup();

    // Case E (F10): dependencies.list returns 0 items but still has next_cursor -> must show project-deps-table-partial, NOT "还没有登记依赖".
    stubDepsFetch({
      model: json(emptyOverviewModelResponse()),
      deps: json({
        ok: true,
        result: { data: { work_id: null, items: [], total: 0, next_cursor: 'cursor-page-2', as_of: { at_unix_ms: NOW } } },
        freshness: { report_received: { status: 'not_measured' }, authoritative: { status: 'measured', at_unix_ms: NOW }, page_read: { status: 'measured', at_unix_ms: NOW } },
      }),
    });
    renderApp('/projects/demo?tab=deps');
    expect(await screen.findByTestId('project-deps-table-partial')).toBeTruthy();
    expect(screen.queryByTestId('project-deps-table-empty')).toBeNull();
    expect(screen.getByTestId('project-deps-versions').textContent).not.toContain('还没有登记依赖');
    cleanup();

    // Case F (Item 3): when chain read is incomplete while confirmed chains exist, chain count tag must say "至少 N 条链路卡住" (not definite "N 条链路卡住").
    const partialWithChains = makeRichModel();
    partialWithChains.input.decisions = { coverage: 'partial', items: [], reason: 'truncated' };
    partialWithChains.coverage = {
      ...partialWithChains.coverage,
      decisions: { status: 'partial', missing: [{ op: 'decisions.list', reason: 'truncated' }] },
    };
    stubDepsFetch({ model: json(partialWithChains) });
    renderApp('/projects/demo?tab=deps');
    const partialChainCount = await screen.findByTestId('project-deps-chain-count');
    expect(partialChainCount.textContent).toBe('至少 2 条链路卡住');
    expect(screen.getByTestId('project-deps-chains-partial-note').textContent).toContain('等待回答的问题');
  });

  it('navigates to task detail when clicking a dependency row', async () => {
    const user = userEvent.setup();
    stubDepsFetch();
    const { router } = renderApp('/projects/demo?tab=deps');
    await waitFor(() => expect(screen.getByTestId('dep-row-T-103-T-102')).toBeTruthy());

    await user.click(screen.getByTestId('dep-row-T-103-T-102'));
    expect(router.state.location.pathname).toBe('/tasks/T-103');
  });

  it('switches to Relationship map, Dependency details, and People & handoffs views on the same tab', async () => {
    const user = userEvent.setup();
    stubDepsFetch();
    renderApp('/projects/demo?tab=deps');
    await waitFor(() => expect(screen.getByTestId('project-deps-view-switcher')).toBeTruthy());

    await user.click(screen.getByTestId('project-deps-view-map'));
    await waitFor(() => expect(screen.getByTestId('project-map-drawing')).toBeTruthy());

    await user.click(screen.getByTestId('project-deps-view-details'));
    await waitFor(() => expect(screen.getByTestId('dependency-detail')).toBeTruthy());

    await user.click(screen.getByTestId('project-deps-view-handoffs'));
    const handoffs = await screen.findByTestId('project-deps-handoffs');
    await waitFor(() => expect(handoffs.textContent).toContain('周宁 → 林悦'));
    expect(handoffs.textContent).toContain('订单页面联调');

    await user.click(screen.getByTestId('project-deps-view-overview'));
    await waitFor(() => expect(screen.getByTestId('project-deps-chains')).toBeTruthy());
  });

  it.each(['zh-CN', 'en-US'] as const)('renders all 7 states cleanly in %s (loading, empty, data, failed, partial, revoked, unknown)', async (language) => {
    await act(async () => { await i18n.changeLanguage(language); });

    // 1. Loading
    let resolvePending!: () => void;
    const pending = new Promise<Response>((resolve) => {
      resolvePending = () => resolve(json(emptyOverviewModelResponse()));
    });
    stubDepsFetch({ model: () => pending, deps: () => pending });
    renderApp('/projects/demo?tab=deps');
    expect(await screen.findByTestId('project-deps-loading')).toBeTruthy();
    resolvePending();
    cleanup();

    // 2. Empty (calm design state only when both reads are complete and genuinely empty)
    stubDepsFetch({
      model: json(emptyOverviewModelResponse()),
      deps: json({ ok: true, result: { data: { work_id: null, items: [], total: 0, next_cursor: null, as_of: { at_unix_ms: NOW } } }, freshness: { report_received: { status: 'not_measured' }, authoritative: { status: 'measured', at_unix_ms: NOW }, page_read: { status: 'measured', at_unix_ms: NOW } } }),
    });
    renderApp('/projects/demo?tab=deps');
    const calm = await screen.findByTestId('project-deps-calm');
    expect(calm.textContent).toContain(language === 'zh-CN' ? '现在没有卡住的链路' : 'No blocked chains right now');
    expect(screen.getByTestId('project-deps-table-empty').textContent).toContain(
      language === 'zh-CN' ? '这个项目的任务之间还没有登记依赖。' : 'No dependencies have been recorded',
    );
    expect(screen.getByTestId('project-deps-blockers-empty').textContent).toContain(
      language === 'zh-CN' ? '现在没有卡点。' : 'No blockers right now.',
    );
    cleanup();

    // 3. Failed
    stubDepsFetch({
      model: json({ ok: false, error: { code: 'Unavailable', message: 'failed' } }, 503),
      deps: json({ ok: false, error: { code: 'Unavailable', message: 'failed' } }, 503),
    });
    renderApp('/projects/demo?tab=deps');
    expect(await screen.findByTestId('project-deps-failed')).toBeTruthy();
    expect(screen.getByTestId('freshness-badge')).toBeTruthy();
    cleanup();

    // 4. Partial failure (dependencies.list fails while overview-model succeeds)
    stubDepsFetch({
      model: json(makeRichModel()),
      deps: json({ ok: false, error: { code: 'Unavailable', message: 'partial' } }, 503),
    });
    renderApp('/projects/demo?tab=deps');
    expect(await screen.findByTestId('project-deps-partial')).toBeTruthy();
    expect(await screen.findByTestId('project-deps-table-failed')).toBeTruthy();
    expect(screen.queryByTestId('project-deps-table-empty')).toBeNull();
    cleanup();

    // 5. Unknown (ordering incomparable)
    stubDepsFetch({ model: json(makeRichModel({ ordering: 'incomparable' })) });
    renderApp('/projects/demo?tab=deps');
    expect(await screen.findByTestId('project-deps-unknown')).toBeTruthy();
    cleanup();

    // 6. Revoked (clears all previously rendered facts)
    stubDepsFetch();
    const { queryClient } = renderApp('/projects/demo?tab=deps');
    await screen.findByTestId('project-deps-versions-count');
    act(() => { revokeObservationReads(queryClient); });
    await screen.findByTestId('state-forbidden');
    expect(screen.queryByTestId('project-deps-chains')).toBeNull();
    expect(screen.queryByTestId('project-deps-versions')).toBeNull();
  });

  it('sanitizes ALL human-facing free text fields across every chain tab and detail panel (including review footer related decisions, F21) and passes axe', async () => {
    const user = userEvent.setup();
    const realOverview = realShapedOverview();
    const dirtyModel = makeRichModel();
    dirtyModel.input.tasks = {
      coverage: 'complete',
      items: [
        {
          id: 'issue-filter-api',
          title: 'client[secret]=leak-task-title',
          projectId: 'demo',
          ownerPersonId: 'M-003',
          agentName: 'payload={"secret":"leak-agent-name"}',
          started: true,
          agentActive: false,
          acceptance: [{ evidence: [{ passed: true, current: true }] }],
          submittedResultAtUnixMs: NOW - DAY,
          integrated: false,
          report: { completed: false, reportedAtUnixMs: NOW - DAY },
          externalWait: null,
          commitments: [{ id: 'comm-1', title: 'payload={"secret":"leak-commitment"}', kind: 'project' }],
        },
        {
          id: 'issue-filter-mid',
          title: 'client[secret]=leak-mid-title',
          projectId: 'demo',
          ownerPersonId: 'M-002',
          started: true,
          agentActive: false,
          acceptance: [{ evidence: [] }],
          submittedResultAtUnixMs: null,
          integrated: false,
          report: null,
          externalWait: null,
          commitments: [{ id: 'comm-1', title: 'payload={"secret":"leak-commitment"}', kind: 'project' }],
        },
        {
          id: 'issue-filter-page',
          title: 'payload={"token":"leak-1"}',
          projectId: 'demo',
          ownerPersonId: 'M-002',
          started: true,
          agentActive: false,
          acceptance: [{ evidence: [] }],
          submittedResultAtUnixMs: null,
          integrated: false,
          report: null,
          externalWait: {
            party: 'party_token={"secret":"leak-party"}',
            what: 'client[key]=leak-ext-what',
            sinceUnixMs: NOW - 3 * DAY,
          },
          commitments: [{ id: 'comm-1', title: 'payload={"secret":"leak-commitment"}', kind: 'project' }],
        },
      ],
    };
    dirtyModel.input.dependencies = {
      coverage: 'complete',
      items: [
        {
          id: 'D-dirty-1',
          consumerTaskId: 'issue-filter-page',
          providerTaskId: 'issue-filter-api',
          satisfied: false,
          what: 'payload={"token":"leak-requirement"}',
          sinceUnixMs: NOW - DAY,
        },
        {
          id: 'D-dirty-2',
          consumerTaskId: 'issue-filter-page',
          providerTaskId: 'issue-filter-mid',
          satisfied: false,
          what: 'payload={"token":"leak-requirement-2"}',
          sinceUnixMs: NOW - DAY,
        },
      ],
    };
    dirtyModel.input.decisions = {
      coverage: 'complete',
      items: [
        {
          id: 'Q-dirty-review-related',
          taskId: 'issue-filter-api',
          state: 'open',
          current: true,
          question: 'client[secret]=leak-related-decision-question',
          allowedResponderIds: ['M-010'],
          createdAtUnixMs: NOW - DAY,
        },
        {
          id: 'Q-dirty-decision',
          taskId: 'issue-filter-mid',
          state: 'open',
          current: true,
          question: 'client[secret]=leak-decision-question',
          allowedResponderIds: ['M-001'],
          createdAtUnixMs: NOW - DAY,
        },
      ],
    };
    dirtyModel.input.reviewRounds = {
      coverage: 'complete',
      items: [
        {
          id: 'R-dirty-1',
          taskId: 'issue-filter-api',
          roundIndex: 1,
          state: 'open',
          current: true,
          independent: true,
          authorPersonId: 'M-003',
          reviewerPersonId: 'M-004',
          openedAtUnixMs: NOW - DAY,
        },
      ],
    };

    stubDepsFetch({
      overview: json(realOverview),
      model: json(dirtyModel),
      deps: json({
        ok: true,
        result: {
          data: {
            work_id: null,
            items: [{
              work_id: 'issue-filter-page',
              work_title: 'payload={"token":"leak-1"}',
              provider_work_id: 'issue-filter-api',
              provider_title: 'client[secret]=leak-2',
              dependency_position: 1,
              version_policy: 'current_contract',
              review_assurance: null,
              required_version: { receipt_id: null, contract_hash: DIGEST, source_snapshot_id: 'snap' },
              available_version: { receipt_id: RECEIPT_V1, contract_hash: DIGEST, source_snapshot_id: 'snap' },
              adopted_version: { receipt_id: RECEIPT_V1, contract_hash: DIGEST, source_snapshot_id: 'snap' },
              consumed_version: { receipt_id: RECEIPT_V1, contract_hash: DIGEST, source_snapshot_id: 'snap' },
              adoption_valid: true,
              validity: 'valid',
              reason: null,
              missing: {},
            }],
            total: 1,
            next_cursor: null,
            as_of: { at_unix_ms: NOW },
          },
        },
        freshness: realOverview.freshness,
      }),
    });
    const { container } = renderApp('/projects/demo?tab=deps');
    await waitFor(() => expect(screen.getByTestId('project-deps-versions-count').textContent).toBe('1'));
    await waitFor(() => expect(screen.getByTestId('project-deps-blockers-count').textContent).toBe('4'));

    const page = screen.getByTestId('page-project-dependencies');
    const leakPattern = /undefined|null|NaN|leak-1|leak-2|leak-task-title|leak-mid-title|leak-agent-name|leak-commitment|leak-party|leak-ext-what|leak-requirement|leak-requirement-2|leak-decision-question|leak-related-decision-question|contract_hash|receipt_id|current_contract/;
    const assertClean = () => {
      const visible = defaultVisibleText(page);
      expect(visible).not.toMatch(leakPattern);
      expect(findForbiddenVisibleText(visible)).toEqual([]);
      expect(findHumanLanguageViolations(page)).toEqual([]);
    };

    assertClean();

    // Open EVERY chain tab (review with related open decision, decision, external) and its impact detail panel.
    const chainTabs = within(screen.getByRole('group', { name: '链路' })).getAllByRole('button');
    expect(chainTabs.length).toBeGreaterThanOrEqual(3);
    for (const tab of chainTabs) {
      await user.click(tab);
      assertClean();
      const impactBtn = screen.queryByTestId('project-deps-chain-impact-btn');
      if (impactBtn) {
        await user.click(impactBtn);
        await screen.findByTestId('project-deps-detail');
        assertClean();
        await user.click(screen.getByTestId('project-deps-detail-back'));
      }
    }

    // Open the chains count detail panel.
    await user.click(screen.getByTestId('project-deps-chain-count'));
    await screen.findByTestId('project-deps-detail');
    assertClean();
    await user.click(screen.getByTestId('project-deps-detail-back'));

    // Open EVERY blocker behind-count detail panel.
    const behindButtons = screen.getAllByTestId('project-deps-blocker-behind');
    expect(behindButtons.length).toBeGreaterThanOrEqual(3);
    for (const button of behindButtons) {
      await user.click(button);
      await screen.findByTestId('project-deps-detail');
      assertClean();
      await user.click(screen.getByTestId('project-deps-detail-back'));
    }

    const results = await axe(container);
    expect(results).toHaveNoViolations();
  }, 15_000);
});
