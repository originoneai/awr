import { act, cleanup, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { axe, toHaveNoViolations } from 'jest-axe';
import { afterEach, describe, expect, it, vi } from 'vitest';
import i18n from './i18n';
import { defaultVisibleText, findForbiddenVisibleText } from './lib/human-language-guard';
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
      ],
    },
    tasks: {
      coverage: 'complete',
      items: [
        { id: 'T-102', title: '接口第 3 版：提交与错误反馈', projectId: 'demo', ownerPersonId: 'M-003', started: true, agentName: 'Codex', agentActive: false, acceptance: [{ evidence: [{ passed: true, current: true }] }], submittedResultAtUnixMs: NOW - 2 * DAY, integrated: false, report: { completed: false, reportedAtUnixMs: NOW - 2 * DAY }, externalWait: null, commitments: [{ id: 'demo-week-47', title: 'Week 47', kind: 'project' }] },
        { id: 'T-103', title: '订单页面联调', projectId: 'demo', ownerPersonId: 'M-002', started: true, agentName: 'Claude 桌面', agentActive: true, acceptance: [{ evidence: [] }], submittedResultAtUnixMs: null, integrated: false, report: { completed: false, reportedAtUnixMs: NOW - 60_000 }, externalWait: null, commitments: [{ id: 'demo-week-47', title: 'Week 47', kind: 'project' }] },
        { id: 'T-104', title: '空状态检查', projectId: 'demo', ownerPersonId: 'M-002', started: true, agentName: 'Codex', agentActive: false, acceptance: [{ evidence: [] }], submittedResultAtUnixMs: null, integrated: false, report: { completed: false, reportedAtUnixMs: NOW - 2 * DAY }, externalWait: null, commitments: [{ id: 'demo-week-47', title: 'Week 47', kind: 'project' }] },
        { id: 'T-106', title: '业务验收', projectId: 'demo', ownerPersonId: 'M-004', started: false, agentActive: false, acceptance: [{ evidence: [] }], submittedResultAtUnixMs: null, integrated: false, report: null, externalWait: null, commitments: [{ id: 'demo-week-47', title: 'Week 47', kind: 'project' }] },
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
  it('renders prototype structure, version numbers without receipt IDs, impact-sorted blockers, and exact count-to-detail parity', async () => {
    const user = userEvent.setup();
    stubDepsFetch();
    const { router } = renderApp('/projects/demo?tab=deps');

    await waitFor(() => expect(screen.getByTestId('project-deps-versions-count').textContent).toBe('3'));
    expect(screen.getByTestId('project-deps-blockers-count').textContent).toBe('3');
    expect(screen.getByTestId('project-deps-chain-count').textContent).toContain('3 条链路卡住');

    // Left table: 3 rows with sequential version numbers ("第 1 版", "第 2 版"), never receipt IDs.
    const versionsBlock = screen.getByTestId('project-deps-versions');
    const tableRows = within(versionsBlock).getAllByTestId(/^dep-row-/);
    expect(tableRows.length).toBe(3);
    expect(versionsBlock.textContent).toContain('第 1 版');
    expect(versionsBlock.textContent).toContain('第 2 版');
    expect(versionsBlock.textContent).toContain('上游有新版本');
    expect(versionsBlock.textContent).toContain('还没有可采用的');
    expect(versionsBlock.textContent).toContain('下游只能采用通过复核的那一版。上游出了新版本，下游要重新检查。');
    expect(versionsBlock.textContent).not.toContain(RECEIPT_V1);
    expect(versionsBlock.textContent).not.toContain(RECEIPT_V2);
    expect(versionsBlock.textContent).not.toContain(DIGEST);

    // Right blockers: sorted by behind count descending (4, 3, 1).
    const blockerRows = screen.getAllByTestId('project-deps-blocker-row');
    expect(blockerRows.length).toBe(3);
    expect(blockerRows[0].textContent).toContain('许宁的复核队列');
    expect(blockerRows[0].textContent).toContain('压着 4 件');
    expect(blockerRows[1].textContent).toContain('等林悦回答一个问题');
    expect(blockerRows[1].textContent).toContain('压着 3 件');
    expect(blockerRows[2].textContent).toContain('等甲方确认导出字段');
    expect(blockerRows[2].textContent).toContain('压着 1 件');

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

    // Freshness badge and coverage footer are present, and no forbidden engineering strings or <details> exist.
    expect(screen.getByTestId('freshness-badge')).toBeTruthy();
    expect(screen.getByTestId('project-deps-coverage').textContent).toContain('数据读取情况');
    expect(document.querySelector('details')).toBeNull();
    expect(findForbiddenVisibleText(defaultVisibleText(screen.getByTestId('page-project-dependencies')))).toEqual([]);

    // Clicking the review blocker navigates to the reviewer's person page.
    await user.click(within(blockerRows[0]).getByRole('button', { name: /许宁的复核队列/ }));
    expect(router.state.location.pathname).toBe('/people/M-004');
  });

  it('navigates to task detail when clicking a dependency row or a task blocker', async () => {
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

    // Switch to Relationship map
    await user.click(screen.getByTestId('project-deps-view-map'));
    await waitFor(() => expect(screen.getByTestId('project-map-drawing')).toBeTruthy());

    // Switch to Dependency details
    await user.click(screen.getByTestId('project-deps-view-details'));
    await waitFor(() => expect(screen.getByTestId('dependency-detail')).toBeTruthy());

    // Switch to People & handoffs
    await user.click(screen.getByTestId('project-deps-view-handoffs'));
    const handoffs = await screen.findByTestId('project-deps-handoffs');
    await waitFor(() => expect(handoffs.textContent).toContain('周宁 → 林悦'));
    expect(handoffs.textContent).toContain('订单页面联调');

    // Switch back to Dependency overview
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

    // 2. Empty (calm design state: "现在没有卡住的链路" / "No blocked chains right now")
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
    // Wait until the failed deps query settled before asserting the empty text is NOT shown!
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

  it('sanitizes real-shaped/live-sample payloads with zero undefined, null, NaN, or raw fields and passes axe', async () => {
    const realOverview = realShapedOverview();
    stubDepsFetch({
      overview: json(realOverview),
      model: json(makeRichModel()),
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

    const visible = defaultVisibleText(screen.getByTestId('page-project-dependencies'));
    expect(visible).not.toMatch(/undefined|null|NaN|leak-1|leak-2|contract_hash|receipt_id|current_contract/);
    expect(findForbiddenVisibleText(visible)).toEqual([]);

    const results = await axe(container);
    expect(results).toHaveNoViolations();
  });
});
