import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { QueryClientProvider } from '@tanstack/react-query';
import { act, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { MemoryRouter, useLocation } from 'react-router';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import i18n from '../../i18n';
import { DisplayTimeZoneProvider } from '../../lib/display-clock';
import { PerspectiveProvider } from '../../lib/perspective/PerspectiveProvider';
import type { PerspectiveFacts } from '../../lib/perspective/types';
import { createAppQueryClient, queryKeys, revokeObservationReads } from '../../lib/queries';
import { RevocationProvider } from '../../lib/revocation';
import { ScopeProvider } from '../../lib/scope';
import type { TeamPortfolioResponse } from '../../lib/types';
import { WorkbenchPage } from '../../pages/WorkbenchPage';
import { stubFetch } from '../../test-utils';
import { CraftThemeProvider } from '../CraftThemeProvider';

const NOW = Date.UTC(2026, 10, 19, 12, 0, 0);
const DAY = 86_400_000;

function deptFacts(): PerspectiveFacts {
  return {
    supervisedGroupCount: 1,
    memberGroupProjectCount: 3,
    visibleProjectGroupCount: 3,
    visibleProjectDepartmentCount: 2,
    canViewAllProjects: true,
    directory: { groupsRegistered: true, departmentsRegistered: true },
    pending: { me: 0, lead: 0, dept: 1, top: 0 },
  };
}

function canonicalPortfolio(): TeamPortfolioResponse {
  return {
    ok: true,
    result: {
      protocol_version: 1,
      op: 'portfolio.overview',
      selection_basis: 'member',
      data: {
        as_of: { at_unix_ms: NOW },
        execution_authorized: false,
        total_projects: 3,
        truncated: false,
        next_cursor: null,
        projects: [
          {
            project_id: 'P-001',
            project_key: 'P-001',
            project_status: 'active',
            coordinator_epoch: 'epoch-1',
            project_revision: '47',
            responsible_person: null,
            directory: { registered: true, group: { id: 'G-001', name: '应用小组' }, departments: [{ id: 'G-004', name: '研发部' }] },
            works: { total: 14, verified: 4, delivered_unverified: 1, completed_without_independent_review: 0, in_progress: 7, not_started: 2, retired: 0, hidden_work_count: 0 },
            waits: { decision: 2, review: 3, dependency: 1, oldest_open_unix_ms: NOW - 2 * DAY },
            latest_update: {
              update_id: 'U-001',
              period_kind: 'week',
              period_key: '2026-W47',
              starts_on: '2026-11-16',
              ends_on: '2026-11-22',
              status: 'risk',
              summary: '接口第 3 版的复核拖了两天，页面联调在等。',
              author: { person_id: 'M-001', display_name: '周宁' },
              recorded_at_unix_ms: NOW,
            },
            read: { status: 'complete' },
          },
          {
            project_id: 'P-002',
            project_key: 'P-002',
            project_status: 'active',
            coordinator_epoch: 'epoch-2',
            project_revision: '47',
            responsible_person: null,
            directory: { registered: true, group: { id: 'G-002', name: '平台小组' }, departments: [{ id: 'G-004', name: '研发部' }] },
            works: { total: 8, verified: 6, delivered_unverified: 0, completed_without_independent_review: 0, in_progress: 1, not_started: 1, retired: 0, hidden_work_count: 0 },
            waits: { decision: 0, review: 0, dependency: 0, oldest_open_unix_ms: null },
            latest_update: {
              update_id: 'U-004',
              period_kind: 'week',
              period_key: '2026-W47',
              starts_on: '2026-11-16',
              ends_on: '2026-11-22',
              status: 'ok',
              summary: '试运行第二周，没有新问题。',
              author: { person_id: 'M-005', display_name: '吴桐' },
              recorded_at_unix_ms: NOW - 3 * DAY,
            },
            read: { status: 'complete' },
          },
          {
            project_id: 'P-003',
            project_key: 'P-003',
            project_status: 'active',
            coordinator_epoch: 'epoch-3',
            project_revision: '47',
            responsible_person: null,
            directory: { registered: true, group: { id: 'G-003', name: '交付小组' }, departments: [{ id: 'G-005', name: '客户交付部' }] },
            works: { total: 7, verified: 2, delivered_unverified: 1, completed_without_independent_review: 0, in_progress: 1, not_started: 3, retired: 0, hidden_work_count: 0 },
            waits: { decision: 0, review: 0, dependency: 0, oldest_open_unix_ms: null },
            latest_update: {
              update_id: 'U-007',
              period_kind: 'week',
              period_key: '2026-W45',
              starts_on: '2026-11-02',
              ends_on: '2026-11-08',
              status: 'ok',
              summary: '精简方案通过复核。',
              author: { person_id: 'M-007', display_name: '郑岚' },
              recorded_at_unix_ms: NOW - 12 * DAY,
            },
            read: { status: 'complete' },
          },
        ],
      },
    },
    read_at_unix_ms: NOW,
    coverage: {
      status: 'complete',
      returned_projects: 3,
      total_projects: 3,
      not_read_projects: 0,
      truncated: false,
      next_cursor: null,
    },
    freshness: {
      report_received: { status: 'not_measured' },
      authoritative: { status: 'measured', at_unix_ms: NOW },
      page_read: { status: 'measured', at_unix_ms: NOW },
    },
    department_facts: {
      department_name: '研发部',
      viewer_person_id: 'M-010',
      department_update: {
        id: null,
        health: 'risk',
        sentence: '3 个项目里 1 个有风险：订单协作平台只有一位复核人，排着 3 件。另有 1 个项目 12 天没有更新。',
        author_person_id: 'M-010',
        author_name: '部门负责人',
        written_at_unix_ms: NOW - 2 * DAY,
      },
      project_titles: { 'P-001': '订单协作平台', 'P-002': '统一通知服务', 'P-003': '客户接入改造' },
      project_leads: { 'P-001': '周宁', 'P-002': '吴桐', 'P-003': '郑岚' },
      monthly_verified: {
        current: {
          count: 2,
          items: [
            { id: 'T-107', title: '订单列表分页', project_key: 'P-001', project_title: '订单协作平台', owner_name: '林悦', verified_at_unix_ms: NOW - 3 * DAY },
            { id: 'T-203', title: '短信通道接入', project_key: 'P-002', project_title: '统一通知服务', owner_name: '赵衡', verified_at_unix_ms: NOW - 5 * DAY },
          ],
        },
        previous: { count: 1 },
      },
      cross_group_waits: [
        { id: 'w4', who: '应用小组', on: '平台小组', what: '接口第 3 版通过复核', kind: 'review', days: 2, started_at_unix_ms: NOW - 2 * DAY, behind: 3, task_id: 'T-102', task_title: '接口第 3 版', project_key: 'P-001' },
        { id: 'w5', who: '交付小组', on: '甲方', what: '给测试环境账号', kind: 'external', days: 9, started_at_unix_ms: NOW - 9 * DAY, behind: 2, task_id: 'T-303', task_title: '走通接入', project_key: 'P-003' },
      ],
      pending: [
        {
          id: 'Q-003',
          kind: 'decision',
          title: '订单协作平台能不能多一位复核人',
          description: '现在能复核的只有许宁一位，排着 3 件。',
          from: '周宁昨天提出',
          since_unix_ms: NOW - DAY,
          days: 1,
          project_key: 'P-001',
          project_title: '订单协作平台',
          task_id: null,
          person_id: 'M-004',
          copy_text: '回答周宁在订单协作平台提的决定：能不能多一位复核人',
          agent_guidance: '在你的 Agent 里回答，周宁的 Agent 下次开工时会读到。',
        },
      ],
      chains: [
        {
          id: 'dept-review-capacity',
          tab: '复核人手',
          footer: '周宁昨天请你决定：能不能多一位复核人。',
          cta: { label: '看这个决定', to: '/inbox?scope=P-001&item=Q-003' },
          nodes: [
            { id: 'n1', kind: 'root', root: true, kicker: '卡在这里', title: '只有一位复核人', owner_name: '许宁', queue: [{ title: '接口第 3 版', days: 2 }], foot_note: '1 件在排队，都等同一位复核人', to: '/inbox?scope=P-001&item=Q-003' },
            { id: 'n2', kind: 'project', kicker: '项目', title: '订单协作平台', meta: '本周承诺有风险', health: 'risk', to: '/projects/P-001' },
            { id: 'n3', kind: 'goal', kicker: '目标', title: '订单从查询到提交在一个页面完成', health: 'risk', foot_note: '有风险', to: '/goals/GOAL-001?scope=P-001' },
          ],
        },
        {
          id: 'dept-external-account',
          tab: '甲方账号',
          footer: '郑岚 9 天前把这件事报给了最高层，还没有回答。',
          cta: null,
          nodes: [
            { id: 'e1', kind: 'root', root: true, kicker: '卡在这里', title: '甲方没给测试环境账号', meta: '9 天', owner_name: '甲方', owner_external: true, to: '/tasks/T-303?scope=P-003' },
            { id: 'e2', kind: 'project', kicker: '项目', title: '客户接入改造', meta: '整个项目停住，12 天没有更新', health: 'none', to: '/projects/P-003' },
          ],
        },
      ],
      lessons: [
        { id: 'L-002', title: '联调前核对接口版本', stage: 'group', author_name: '许宁', why: '被平台小组采用', used: 7, passed: 6, not_applicable: 1 },
        { id: 'L-003', title: '结果不明确时先查询再重试', stage: 'project', author_name: '陈宇', why: '本月新增', used: 3, passed: 3, not_applicable: 0 },
      ],
      usage: {
        collected_groups: ['应用小组'],
        missing_groups: ['平台小组', '交付小组'],
        by_project: {},
      },
    },
  };
}

function LocationProbe() {
  const location = useLocation();
  return <output data-testid="location" data-path={location.pathname} data-search={location.search} />;
}

function renderDeptPage(
  initialEntry = '/workbench?perspective=dept',
  queryClient = createAppQueryClient({ retry: false }),
) {
  const utils = render(
    <QueryClientProvider client={queryClient}>
      <CraftThemeProvider>
        <RevocationProvider>
          <MemoryRouter initialEntries={[initialEntry]}>
            <ScopeProvider>
              <PerspectiveProvider facts={deptFacts()} requestedPerspective="dept">
                <DisplayTimeZoneProvider timeZone="UTC">
                  <WorkbenchPage />
                  <LocationProbe />
                </DisplayTimeZoneProvider>
              </PerspectiveProvider>
            </ScopeProvider>
          </MemoryRouter>
        </RevocationProvider>
      </CraftThemeProvider>
    </QueryClientProvider>,
  );
  return { queryClient, ...utils };
}

beforeEach(async () => {
  await i18n.changeLanguage('zh-CN');
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe('Department Projects workbench (AWR-PPO-021)', () => {
  it('renders prototype blocks, four KPIs, two chains, rail, three tabs, overdue mark and missing-usage note', async () => {
    const user = userEvent.setup();
    stubFetch({ '/api/team/portfolio': canonicalPortfolio() });
    const { container, queryClient } = renderDeptPage();

    const frame = await screen.findByTestId('workbench-frame');
    expect(frame.getAttribute('data-perspective')).toBe('dept');
    expect(frame.getAttribute('data-state')).toBe('data');
    expect([...frame.children].map((c) => c.getAttribute('data-block'))).toEqual(['heading', 'freshness', 'instrument', 'detail']);
    expect(screen.getByRole('heading', { level: 1, name: '部门项目' })).toBeTruthy();
    expect(container.querySelector('.obs-page-subtitle')?.textContent).toBe('11 月');

    // Verdict & worst-case health
    expect(screen.getByText('有风险', { selector: '.obs-workbench-health-word' })).toBeTruthy();
    expect(screen.getByText('你 11 月 17 日在自己的 Agent 里写的 · 汇总时取最差的那个，不取平均')).toBeTruthy();

    // Wait for query settled before asserting absence of cost column / figures
    await waitFor(() => expect(queryClient.getQueryState(queryKeys.portfolio)?.status).toBe('success'));
    expect(screen.queryByRole('columnheader', { name: '投入' })).toBeNull();
    expect(screen.getByTestId('dept-usage-note').textContent).toContain('应用小组');
    expect(screen.getByTestId('dept-usage-note').textContent).toBe('投入只采集到应用小组的用量，另外两个小组的缺失，所以这里不显示费用。');

    // Overdue project marks 该更新了
    expect(screen.getByText('该更新了')).toBeTruthy();
    expect(container.querySelectorAll('.obs-legend')).toHaveLength(1);

    // Chain switch between 复核人手 and 甲方账号
    const chainGroup = screen.getByRole('group', { name: '链路' });
    const chainButtons = within(chainGroup).getAllByRole('button');
    expect(chainButtons.map((b) => b.textContent)).toEqual(['复核人手', '甲方账号']);
    await user.click(chainButtons[1]);
    expect(screen.getByText('甲方没给测试环境账号')).toBeTruthy();

    // Switch to 跨小组等待 and 经验 tabs
    await user.click(screen.getByRole('tab', { name: /跨小组等待/ }));
    expect(screen.getByText('接口第 3 版通过复核')).toBeTruthy();
    expect(screen.getByText('给测试环境账号')).toBeTruthy();
    expect(container.querySelectorAll('.obs-legend')).toHaveLength(0);

    await user.click(screen.getByRole('tab', { name: /经验/ }));
    expect(screen.getByText('联调前核对接口版本')).toBeTruthy();
    expect(screen.getByText('被平台小组采用')).toBeTruthy();
    expect(screen.getByText('7 次，6 次那件事通过了，1 次不适用')).toBeTruthy();
  });

  it('opens detail lists with exact count parity for every KPI and navigates to item details', async () => {
    const user = userEvent.setup();
    stubFetch({ '/api/team/portfolio': canonicalPortfolio() });
    const { container } = renderDeptPage();
    await screen.findByTestId('workbench-frame');

    const expected: Array<[string, number, string]> = [
      ['项目', 3, '/projects/P-001'],
      ['本月核验通过', 2, '/tasks/T-107'],
      ['跨小组等待', 2, '/tasks/T-102'],
      ['等你处理', 1, '/inbox'],
    ];

    for (const [label, count] of expected) {
      const kpiButtons = Array.from(container.querySelectorAll('button.obs-kpi-open'));
      const button = kpiButtons.find((b) => b.querySelector('.obs-kpi-label')?.textContent?.includes(label));
      expect(button, `KPI ${label} should be clickable`).toBeTruthy();
      await user.click(button!);
      const detail = await screen.findByTestId('workbench-metric-detail');
      expect(Number(detail.getAttribute('data-count'))).toBe(count);
      expect(detail.querySelectorAll('.obs-row-item')).toHaveLength(count);
      await user.click(screen.getByRole('button', { name: /返回工作台/ }));
    }

    // Clicking a project row navigates to its project page
    await user.click(screen.getByText('订单协作平台', { selector: '.obs-workbench-unit-name' }));
    expect(screen.getByTestId('location').getAttribute('data-path')).toBe('/projects/P-001');
  });

  it('shows per-project usage only for projects with usage data and never invents figures for missing groups', async () => {
    const withUsage = canonicalPortfolio();
    withUsage.department_facts!.usage = {
      collected_groups: ['应用小组'],
      missing_groups: ['平台小组', '交付小组'],
      by_project: { 'P-001': { display_text: '24 小时' }, 'P-002': null, 'P-003': null },
    };
    stubFetch({ '/api/team/portfolio': withUsage });
    const { queryClient } = renderDeptPage();
    await screen.findByTestId('workbench-frame');
    await waitFor(() => expect(queryClient.getQueryState(queryKeys.portfolio)?.status).toBe('success'));

    expect(screen.getByRole('columnheader', { name: '投入' })).toBeTruthy();
    expect(screen.getByText('24 小时')).toBeTruthy();
    expect(screen.getAllByText('未采集')).toHaveLength(2);
    expect(screen.getByTestId('dept-usage-note').textContent).toBe('投入只采集到应用小组的用量，另外两个小组的缺失，所以这里不显示费用。');
  });

  it('shows the live portfolio KPIs it cannot know as a dash, never as zero', async () => {
    const live = canonicalPortfolio() as TeamPortfolioResponse & { department_facts?: unknown };
    delete live.department_facts;
    stubFetch({ '/api/team/portfolio': live });
    const { container } = renderDeptPage();
    await screen.findByTestId('workbench-frame');
    await waitFor(() => expect(container.querySelectorAll('.obs-kpi-value')).toHaveLength(4));
    const values = [...container.querySelectorAll('.obs-kpi-value')].map((value) => value.textContent?.trim() ?? '');
    expect(values[0]).toMatch(/^\d/);
    for (const value of values.slice(1)) {
      expect(value.startsWith('—')).toBe(true);
      expect(value).not.toMatch(/^0/);
    }
  });

  it('issues only one portfolio read for 40 projects and zero per-project requests', async () => {
    const forty = Array.from({ length: 40 }, (_, i) => ({
      ...canonicalPortfolio().result.data.projects[1],
      project_id: `P-${String(i + 1).padStart(3, '0')}`,
      project_key: `P-${String(i + 1).padStart(3, '0')}`,
    }));
    const response: TeamPortfolioResponse = {
      ...canonicalPortfolio(),
      department_facts: null,
      result: {
        ...canonicalPortfolio().result,
        data: {
          ...canonicalPortfolio().result.data,
          projects: forty,
          total_projects: 40,
        },
      },
      coverage: {
        status: 'complete',
        returned_projects: 40,
        total_projects: 40,
        not_read_projects: 0,
        truncated: false,
        next_cursor: null,
      },
    };
    const fetchSpy = stubFetch({ '/api/team/portfolio': response });
    const { queryClient } = renderDeptPage();
    await screen.findByTestId('workbench-frame');
    await waitFor(() => expect(queryClient.getQueryState(queryKeys.portfolio)?.status).toBe('success'));

    const urls = fetchSpy.mock.calls.map(([input]) => String(input));
    expect(urls).toEqual(['/api/team/portfolio']);
    expect(urls.some((u) => u.includes('/api/team/overview-model'))).toBe(false);
    expect(urls.some((u) => u.includes('/api/team/projects'))).toBe(false);
    expect(screen.getByTestId('dept-read-coverage').textContent).toBe('已读取全部 40 个项目');
  });

  it.each(['zh-CN', 'en-US'] as const)('renders all seven states cleanly in %s with freshness badge', async (language) => {
    await i18n.changeLanguage(language);

    // 1. data
    stubFetch({ '/api/team/portfolio': canonicalPortfolio() });
    const dataView = renderDeptPage();
    expect((await screen.findByTestId('workbench-frame')).getAttribute('data-state')).toBe('data');
    expect(screen.getByTestId('freshness-badge')).toBeTruthy();
    dataView.unmount();

    // 2. empty
    const emptyRes: TeamPortfolioResponse = {
      ...canonicalPortfolio(),
      department_facts: null,
      result: { ...canonicalPortfolio().result, data: { ...canonicalPortfolio().result.data, projects: [], total_projects: 0 } },
      coverage: { status: 'complete', returned_projects: 0, total_projects: 0, not_read_projects: 0, truncated: false, next_cursor: null },
    };
    stubFetch({ '/api/team/portfolio': emptyRes });
    const emptyView = renderDeptPage();
    expect((await screen.findByTestId('workbench-frame')).getAttribute('data-state')).toBe('empty');
    expect(screen.getAllByTestId('obs-empty-state').length).toBeGreaterThan(0);
    emptyView.unmount();

    // 3. partial (not_read project)
    const partialRes: TeamPortfolioResponse = {
      ...canonicalPortfolio(),
      department_facts: null,
      result: {
        ...canonicalPortfolio().result,
        data: {
          ...canonicalPortfolio().result.data,
          projects: [
            canonicalPortfolio().result.data.projects[0],
            { project_id: 'P-003', project_key: 'P-003', read: { status: 'not_read', reason: 'read_failed' } },
          ],
          total_projects: 2,
        },
      },
      coverage: { status: 'partial', returned_projects: 2, total_projects: 2, not_read_projects: 1, truncated: false, next_cursor: null },
    };
    stubFetch({ '/api/team/portfolio': partialRes });
    const partialView = renderDeptPage();
    expect((await screen.findByTestId('workbench-frame')).getAttribute('data-state')).toBe('partial');
    expect(screen.getByTestId('workbench-partial')).toBeTruthy();
    expect(document.body.textContent).not.toMatch(/\bread_failed\b|\bnot_read\b/);
    partialView.unmount();

    // 4. unknown (incomparable / not_measured authoritative)
    stubFetch({ '/api/team/portfolio': { ...canonicalPortfolio(), ordering: 'incomparable' } });
    const unknownView = renderDeptPage();
    expect((await screen.findByTestId('workbench-frame')).getAttribute('data-state')).toBe('unknown');
    expect(screen.getByTestId('workbench-ordering-unknown')).toBeTruthy();
    unknownView.unmount();

    // 5. loading
    vi.stubGlobal('fetch', vi.fn(() => new Promise<Response>(() => {})));
    const loadingView = renderDeptPage();
    expect(await screen.findByTestId('workbench-loading')).toBeTruthy();
    expect(screen.getByTestId('freshness-badge')).toBeTruthy();
    loadingView.unmount();

    // 6. failed
    stubFetch({ '/api/team/portfolio': { ok: false, error: { code: 'Busy', message: 'synthetic upstream timeout' } } });
    const failedView = renderDeptPage();
    expect(await screen.findByTestId('workbench-failed')).toBeTruthy();
    expect(screen.getByTestId('freshness-badge')).toBeTruthy();
    expect(document.body.textContent).not.toContain('Busy');
    expect(document.body.textContent).not.toContain('synthetic upstream timeout');
    failedView.unmount();

    // 7. revoked (clears content & stops polling immediately)
    stubFetch({ '/api/team/portfolio': canonicalPortfolio() });
    const revokedView = renderDeptPage();
    expect(await screen.findByTestId('workbench-frame')).toBeTruthy();
    const callsBefore = vi.mocked(fetch).mock.calls.length;
    act(() => revokeObservationReads(revokedView.queryClient));
    expect(await screen.findByTestId('state-forbidden')).toBeTruthy();
    expect(screen.queryByTestId('workbench-frame')).toBeNull();
    expect(document.body.textContent).not.toContain('订单协作平台');
    expect(vi.mocked(fetch).mock.calls.length).toBe(callsBefore);
    revokedView.unmount();
  });

  it.each(['zh-CN', 'en-US'] as const)('renders verbatim real portfolio-web-member and portfolio-web-agent fixtures cleanly in %s', async (language) => {
    await i18n.changeLanguage(language);
    for (const fixtureName of ['portfolio-web-member.json', 'portfolio-web-agent.json']) {
      const raw = JSON.parse(readFileSync(resolve('test/fixtures', fixtureName), 'utf8'));
      const asOf = raw.data.as_of.at_unix_ms;
      const notRead = raw.data.projects.filter((p: { read: { status: string } }) => p.read.status === 'not_read').length;
      const wrapped: TeamPortfolioResponse = {
        ok: true,
        result: raw,
        read_at_unix_ms: asOf + 500,
        coverage: {
          status: raw.data.truncated || notRead > 0 ? 'partial' : 'complete',
          returned_projects: raw.data.projects.length,
          total_projects: raw.data.total_projects,
          not_read_projects: notRead,
          truncated: raw.data.truncated,
          next_cursor: raw.data.next_cursor,
        },
        freshness: {
          report_received: { status: 'not_measured' },
          authoritative: { status: 'measured', at_unix_ms: asOf },
          page_read: { status: 'measured', at_unix_ms: asOf + 500 },
        },
      };
      stubFetch({ '/api/team/portfolio': wrapped });
      const view = renderDeptPage();
      const frame = await screen.findByTestId('workbench-frame');
      const text = frame.textContent ?? '';
      expect(text).not.toMatch(/\b(?:undefined|null|NaN|protocol_version|execution_authorized|selection_basis|coordinator_epoch|project_revision|not_measured|not_read|read_failed|no_read_authority)\b/);
      view.unmount();
    }
  });

  it('keeps local handback copy completely read-only with zero network writes', async () => {
    const user = userEvent.setup();
    const fetchSpy = stubFetch({ '/api/team/portfolio': canonicalPortfolio() });
    renderDeptPage();
    await screen.findByTestId('workbench-frame');
    const callsAfterLoad = fetchSpy.mock.calls.length;

    await user.click(screen.getByText('在你的 Agent 里处理'));
    const handback = await screen.findByTestId('agent-handback');
    expect(handback.textContent).toContain('回答周宁在订单协作平台提的决定：能不能多一位复核人');
    await user.click(within(handback).getByRole('button', { name: /复制这句话/ }));
    expect(fetchSpy.mock.calls.length).toBe(callsAfterLoad);
  });

  it.each(['zh-CN', 'en-US'] as const)('hides cost figures and shows coverage explanation when live portfolio carries partial usage.summary coverage in %s', async (language) => {
    await i18n.changeLanguage(language);
    const auditSample = JSON.parse(readFileSync(resolve('test/fixtures/usage-summary-web-audit.json'), 'utf8'));
    const uncollectedSample = JSON.parse(readFileSync(resolve('test/fixtures/usage-summary-web-uncollected.json'), 'utf8'));
    const livePortfolio = canonicalPortfolio();
    livePortfolio.department_facts = null;
    const [first, second, third] = livePortfolio.result.data.projects;
    if ('works' in first) {
      first.usage = {
        status: auditSample.data.coverage.status,
        cost_status: auditSample.data.coverage.cost_status,
        usage_status: auditSample.data.coverage.usage_status,
        duration_status: auditSample.data.coverage.duration_status,
        groups: auditSample.data.coverage.groups,
      };
    }
    if ('works' in second) {
      second.usage = {
        status: uncollectedSample.data.coverage.status,
        cost_status: uncollectedSample.data.coverage.cost_status,
        usage_status: uncollectedSample.data.coverage.usage_status,
        duration_status: uncollectedSample.data.coverage.duration_status,
        groups: { registered: true, collected: [], missing: [{ id: 'grp-platform', name: 'Platform Group' }] },
      };
    }
    if ('works' in third) {
      third.usage = {
        status: uncollectedSample.data.coverage.status,
        cost_status: uncollectedSample.data.coverage.cost_status,
        usage_status: uncollectedSample.data.coverage.usage_status,
        duration_status: uncollectedSample.data.coverage.duration_status,
        groups: { registered: true, collected: [], missing: [{ id: 'grp-delivery', name: 'Delivery Group' }] },
      };
    }
    stubFetch({ '/api/team/portfolio': livePortfolio });
    const { queryClient } = renderDeptPage();
    const frame = await screen.findByTestId('workbench-frame');
    await waitFor(() => expect(queryClient.getQueryState(queryKeys.portfolio)?.status).toBe('success'));

    // Cost column must stay hidden when coverage is incomplete / cost_status is not_collected.
    expect(screen.queryByRole('columnheader', { name: language === 'zh-CN' ? '投入' : 'Usage' })).toBeNull();
    const note = screen.getByTestId('dept-usage-note').textContent ?? '';
    expect(note).toContain('App Group');
    if (language === 'zh-CN') {
      expect(note).toContain('两个小组');
      expect(note).toContain('不显示费用');
    } else {
      expect(note).toContain('2 other groups');
      expect(note).toContain('no cost figures');
    }
    expect(frame.textContent ?? '').not.toMatch(/\b(?:undefined|null|NaN|not_collected|cost_status)\b/);
  });
});
