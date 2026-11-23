import { QueryClientProvider } from '@tanstack/react-query';
import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { createMemoryRouter, RouterProvider } from 'react-router';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { CraftThemeProvider } from '../components/CraftThemeProvider';
import i18n from '../i18n';
import { DisplayTimeZoneProvider } from '../lib/display-clock';
import { findHumanLanguageViolations } from '../lib/human-language-guard';
import { createAppQueryClient, revokeObservationReads } from '../lib/queries';
import { RevocationProvider } from '../lib/revocation';
import { ScopeProvider } from '../lib/scope';
import { TaskPage } from './TaskPage';
import { deriveTaskPageModel } from './task-page-model';

const NOW_UNIX_MS = 1795089600000; // 2026-11-19T12:00:00Z
const DAY = 86_400_000;

const MEASURED_FRESHNESS = {
  report_received: { status: 'measured' as const, at_unix_ms: NOW_UNIX_MS - 60_000 },
  authoritative: { status: 'measured' as const, at_unix_ms: NOW_UNIX_MS },
  page_read: { status: 'measured' as const, at_unix_ms: NOW_UNIX_MS },
};

function canonicalFixtures() {
  return {
    projects: {
      ok: true,
      projects: [{ key: 'P-001', title: '订单协作平台', role: 'developer' }],
    },
    overview: {
      ok: true,
      project: 'P-001',
      works: [
        {
          key: 'T-102',
          title: '接口第 3 版：提交与错误反馈',
          status: 'claimed',
          owner_person: '陈宇',
          agent: 'Codex' as string | null,
          next_step: '等许宁复核；通过后林悦的页面联调改用第 3 版' as string | null,
          detail_loaded: true,
          workstream_id: 'WS-002',
          contract_hash: 'c'.repeat(64),
        },
        {
          key: 'T-103',
          title: '订单页面联调',
          status: 'claimed',
          owner_person: '林悦',
          agent: 'Claude 桌面',
          next_step: '接口第 3 版通过复核后重新联调',
          detail_loaded: true,
          workstream_id: 'WS-001',
          contract_hash: 'd'.repeat(64),
        },
        {
          key: 'T-106',
          title: '业务验收',
          status: 'unclaimed',
          owner_person: '许宁',
          agent: null,
          next_step: '等订单页面联调',
          detail_loaded: true,
          workstream_id: 'WS-003',
          contract_hash: 'e'.repeat(64),
        },
      ],
      workstreams: [
        { id: 'WS-001', title: '前端' },
        { id: 'WS-002', title: '后端' },
        { id: 'WS-003', title: '验证' },
      ],
      members: [
        { person_id: 'M-002', display_name: '林悦' },
        { person_id: 'M-003', display_name: '陈宇' },
        { person_id: 'M-004', display_name: '许宁' },
      ],
      coverage: {
        scope: { status: 'succeeded' },
        streams: [{ workstream_id: 'WS-002', status: 'succeeded' }],
        works_enumerated: 3,
        works_total: 3,
        succeeded: 1,
        failed: 0,
        truncated: false,
      },
      freshness: MEASURED_FRESHNESS,
    },
    overviewModel: {
      ok: true,
      project: 'P-001',
      read_at_unix_ms: NOW_UNIX_MS,
      input: {
        tasks: {
          coverage: 'complete',
          items: [
            {
              id: 'T-102',
              title: '接口第 3 版：提交与错误反馈',
              ownerPersonId: 'M-003',
              agentName: 'Codex',
              projectId: 'P-001',
              groupId: 'G-001',
              goalIds: ['GOAL-001'],
              commitments: [
                { id: 'project-P-001', title: 'P-001', kind: 'project' },
                { id: 'P-001-week-47', title: 'Week 47 commitment', kind: 'project' },
              ],
              verifiedAtUnixMs: null as number | null,
              acceptance: [
                { evidence: [{ passed: true, current: true }] },
                { evidence: [{ passed: true, current: true }] },
                { evidence: [{ passed: false, current: true }] },
              ],
              report: { completed: false, reportedAtUnixMs: NOW_UNIX_MS - 2 * DAY },
              submittedResultAtUnixMs: NOW_UNIX_MS - 2 * DAY,
              integrated: false,
              started: true,
              agentActive: false,
              externalWait: null,
            },
            {
              id: 'T-103',
              title: '订单页面联调',
              ownerPersonId: 'M-002',
              agentName: 'Claude 桌面',
              projectId: 'P-001',
              groupId: 'G-001',
              goalIds: ['GOAL-001'],
              commitments: [{ id: 'P-001-week-47', title: 'Week 47 commitment', kind: 'project' }],
              verifiedAtUnixMs: null as number | null,
              acceptance: [],
              report: { completed: false, reportedAtUnixMs: NOW_UNIX_MS - 720_000 },
              submittedResultAtUnixMs: null,
              integrated: false,
              started: true,
              agentActive: true,
              externalWait: null,
            },
            {
              id: 'T-106',
              title: '业务验收',
              ownerPersonId: 'M-004',
              agentName: null,
              projectId: 'P-001',
              groupId: 'G-001',
              goalIds: ['GOAL-001'],
              commitments: [{ id: 'P-001-week-47', title: 'Week 47 commitment', kind: 'project' }],
              verifiedAtUnixMs: null as number | null,
              acceptance: [],
              report: null,
              submittedResultAtUnixMs: null,
              integrated: false,
              started: false,
              agentActive: false,
              externalWait: null,
            },
          ],
        },
        dependencies: {
          coverage: 'complete',
          items: [
            { id: 'dep-1', consumerTaskId: 'T-103', providerTaskId: 'T-102', satisfied: false, what: '接口第 3 版', sinceUnixMs: NOW_UNIX_MS - 2 * DAY },
            { id: 'dep-2', consumerTaskId: 'T-106', providerTaskId: 'T-103', satisfied: false, what: '订单页面联调', sinceUnixMs: NOW_UNIX_MS - 2 * DAY },
          ],
        },
        decisions: { coverage: 'complete', items: [] },
        reviewRounds: {
          coverage: 'complete',
          items: [
            {
              id: 'round-t-102-1',
              taskId: 'T-102',
              roundIndex: 1,
              state: 'open',
              current: true,
              independent: true,
              authorPersonId: 'M-003',
              reviewerPersonId: 'M-004',
              openedAtUnixMs: NOW_UNIX_MS - 2 * DAY,
            },
          ],
        },
        handoffs: { coverage: 'complete', items: [] },
        approvals: { coverage: 'complete', items: [] },
        members: {
          coverage: 'complete',
          items: [
            { personId: 'M-002', displayName: '林悦', groupId: 'G-001' },
            { personId: 'M-003', displayName: '陈宇', groupId: 'G-001' },
            { personId: 'M-004', displayName: '许宁', groupId: 'G-002' },
          ],
        },
        activity: { coverage: 'complete', items: [] },
        options: { nowUnixMs: NOW_UNIX_MS, stalledAfterDays: 1 },
      },
      coverage: {
        tasks: { status: 'complete', missing: [] as string[] },
        dependencies: { status: 'complete', missing: [] as string[] },
        decisions: { status: 'complete', missing: [] as string[] },
        reviewRounds: { status: 'complete', missing: [] as string[] },
        handoffs: { status: 'complete', missing: [] as string[] },
        approvals: { status: 'complete', missing: [] as string[] },
        members: { status: 'complete', missing: [] as string[] },
        activity: { status: 'complete', missing: [] as string[] },
      },
      freshness: MEASURED_FRESHNESS,
    },
    snapshot: {
      ok: true,
      result: {
        data: {
          work_id: 'T-102',
          contract_hash: 'c'.repeat(64),
          context_complete: true,
          next_step: '等许宁复核；通过后林悦的页面联调改用第 3 版',
          runtime: { state: 'claimed', work_version: '1' },
          visible_contract: {
            goals: ['订单从查询到提交在一个页面完成'],
            acceptance: [
              '提交成功返回订单编号',
              '提交失败时返回可读的错误原因',
              '同一请求重复提交不会重复下单',
            ],
          },
          acceptance_evidence: {
            status: 'provided' as const,
            basis: 'contract_examples_and_current_candidate',
            candidate_digest: 'a'.repeat(64),
            candidate_current: true,
            review_state: 'open',
            review_current_candidate: true,
            facts_truncated: false,
            items: [
              {
                criterion: '提交成功返回订单编号',
                status: 'supported' as const,
                reason: 'bound_checks_passed_and_review_approved',
                bound_checks: ['提交成功'],
                checks: [{ check: '提交成功', outcome: 'passed' as const, run_id: 'check-t-102-1' }],
                examples_count: 1,
                positive_examples: 1,
                negative_examples: 0,
              },
              {
                criterion: '提交失败时返回可读的错误原因',
                status: 'supported' as const,
                reason: 'bound_checks_passed_and_review_approved',
                bound_checks: ['错误分支'],
                checks: [{ check: '错误分支', outcome: 'passed' as const, run_id: 'check-t-102-2' }],
                examples_count: 1,
                positive_examples: 1,
                negative_examples: 0,
              },
              {
                criterion: '同一请求重复提交不会重复下单',
                status: 'waiting' as const,
                reason: 'awaiting_review_approval',
                bound_checks: ['重复提交'],
                checks: [{ check: '重复提交', outcome: 'passed' as const, run_id: 'check-t-102-3' }],
                examples_count: 1,
                positive_examples: 1,
                negative_examples: 0,
              },
            ],
          },
          observation: {
            observed_at_unix_ms: NOW_UNIX_MS,
            responsibility: {
              owner: 'M-003',
              owner_person_id: 'M-003',
              owner_name: '陈宇',
              independent_reviewer: { person_id: 'M-004', display_name: '许宁' },
            },
            client: { product: 'Codex' },
            progress: {
              phase: 'ready_for_review',
              summary: '第 3 版已提交复核，补上了超时重发的处理。',
              completed: ['补重复提交的判断', '补错误码对照表'],
              blockers: ['在等复核'],
              tests: [
                { name: '提交成功', outcome: 'passed' },
                { name: '错误分支', outcome: 'passed' },
                { name: '重复提交', outcome: 'passed' },
              ],
              artifacts: [{ label: '接口第 3 版说明' }, { label: '测试报告' }],
              origin: { actor_id: 'M-003-agent', actor_name: 'Codex（陈宇的 Agent）' },
              provenance: 'caller_declared',
              reported_at_unix_ms: 1794932400000,
              contract_matches_current: true,
              stale: false,
              next_action: '等许宁复核；通过后林悦的页面联调改用第 3 版',
            },
            collaboration: {
              delivery_required: true,
              candidate: { current: true, required_checks: ['单元测试', '接口约定检查'] },
              review: { id: 'round-t-102-1', state: 'open', current_candidate: true },
              integration: null as { state: string; current_binding: boolean } | null,
              publication: null,
              verification: [
                { check: '单元测试', outcome: 'passed' },
                { check: '接口约定检查', outcome: 'passed' },
              ],
            },
            structured_waits: {
              status: 'not_provided' as 'provided' | 'not_provided' | 'unknown',
              items: null as Array<{
                waiting_on: { kind: 'person' | 'group' | 'external_party'; name: string };
                what: string;
                since_unix_ms: number;
                release_condition: string;
              }> | null,
              provenance: 'caller_declared' as const,
              grants_authority: false as const,
              checkpoint_id: 'checkpoint-t-102',
              reported_at_unix_ms: 1794932400000,
              contract_matches_current: true,
            },
          },
        },
      },
      freshness: MEASURED_FRESHNESS,
    },
    reviewRounds: {
      ok: true,
      result: {
        data: {
          work_id: 'T-102',
          items: [
            {
              round_id: 'round-t-102-1',
              round_index: 1,
              round_position: 1,
              readable: true,
              state: 'open',
              contract_matches_current: true,
              reviewer_role: null as string | null,
              reviewer_actor_id: null as string | null,
              reviewer_person_id: null as string | null,
              decided_at_unix_ms: null as number | null,
              reason_first_line: null as string | null,
            },
          ],
          total: 1,
          next_cursor: null,
          as_of: { at_unix_ms: NOW_UNIX_MS },
        },
      },
      freshness: MEASURED_FRESHNESS,
    },
    reviewInspect: {
      ok: true,
      result: {
        data: {
          review: {
            decisions: [],
            author_actor_id: 'M-003-agent',
            author_person_id: 'M-003',
            artifact_digest: 'a'.repeat(64),
            contract_hash: 'c'.repeat(64),
            bundle_hash: 'b'.repeat(64),
          },
        },
      },
      freshness: MEASURED_FRESHNESS,
    },
    dependencies: {
      ok: true,
      result: {
        data: { work_id: 'T-102', items: [], total: 0, next_cursor: null, as_of: { at_unix_ms: NOW_UNIX_MS } },
      },
      freshness: MEASURED_FRESHNESS,
    },
    handoffs: {
      ok: true,
      result: {
        data: { items: [], total: 0, next_cursor: null, as_of: { at_unix_ms: NOW_UNIX_MS } },
      },
      freshness: MEASURED_FRESHNESS,
    },
    activity: {
      ok: true,
      data: {
        items: [
          {
            id: 'EV-008',
            actor_id: 'M-003-agent',
            client_id: 'M-003-codex',
            actor: { person_id: 'M-003', display_name: '陈宇' },
            client: { display_name: 'Codex' },
            action: 'delivery.submit_and_request_review',
            work_id: 'T-102',
            result: 'committed',
            created_at_unix_ms: 1794932400000,
            credential_id: null,
          },
          {
            id: 'EV-007',
            actor_id: 'M-003-agent',
            client_id: 'M-003-codex',
            actor: { person_id: 'M-003', display_name: '陈宇' },
            client: { display_name: 'Codex' },
            action: 'work.claim',
            work_id: 'T-102',
            result: 'committed',
            created_at_unix_ms: 1794909900000,
            credential_id: null,
          },
          {
            id: 'EV-006',
            actor_id: 'M-004-agent',
            client_id: null,
            actor: { person_id: 'M-004', display_name: '许宁' },
            client: { display_name: null },
            action: 'review.approve',
            work_id: 'T-102',
            result: 'committed',
            created_at_unix_ms: 1794751200000,
            credential_id: null,
          },
        ] as Array<Record<string, unknown>>,
        next_cursor: null,
        development_source: 'audit.development',
        chat_text_collected: false,
        tool_io_collected: false,
        request_retention: '30d',
        scope: 'project',
        unknown_means: 'not_observed',
      },
    },
    documents: {
      ok: true,
      result: {
        data: {
          work_id: 'T-102',
          items: [
            { document_position: 1, path: 'docs/P-001/DOC-001.md', name: '订单协作平台的目标', summary: '目标与成功标准', summary_kind: 'source_excerpt', sha256: 'a'.repeat(64), byte_length: 120, source_kind: 'registered_source', readable: true, status: 'available', reason: null },
            { document_position: 2, path: 'docs/P-001/DOC-002.md', name: '接口约定', summary: '11 月 12 日改过', summary_kind: 'source_excerpt', sha256: 'b'.repeat(64), byte_length: 140, source_kind: 'registered_source', readable: true, status: 'available', reason: null },
          ],
          total: 2,
          next_cursor: null,
          truncated: false,
          read_only: true,
          execution_authorized: false,
          scope: 'work',
          content_read_limit_bytes: 262144,
          as_of: { at_unix_ms: NOW_UNIX_MS, source_snapshot_id: 'snap-1', project_revision: '47' },
        },
      },
      freshness: MEASURED_FRESHNESS,
    },
    goals: {
      ok: true,
      result: {
        data: {
          items: [{ goal_id: 'GOAL-001', project_id: 'P-001', title: '订单从查询到提交在一个页面完成' }],
          total: 1,
          next_cursor: null,
        },
      },
      freshness: MEASURED_FRESHNESS,
    },
    lessons: {
      ok: true,
      result: {
        data: {
          items: [
            {
              lesson_id: 'L-003',
              project_id: 'P-001',
              title: '结果不明确时先查询再重试',
              stage: 'project',
              author: { person_id: 'M-003', display_name: '陈宇' },
              tasks: ['T-108', 'T-102', 'T-109'],
            },
          ],
          total: 1,
          next_cursor: null,
        },
      },
      freshness: MEASURED_FRESHNESS,
    },
  };
}

type FetchMockHandler = (url: string, body: Record<string, unknown> | null) => Promise<Response> | Response;

function jsonResponse(payload: unknown, status = 200) {
  return new Response(JSON.stringify(payload), {
    status,
    headers: { 'Content-Type': 'application/json' },
  });
}

function defaultHandler(fixtures: ReturnType<typeof canonicalFixtures>): FetchMockHandler {
  return (url, body) => {
    if (url.startsWith('/api/team/projects')) return jsonResponse(fixtures.projects);
    if (url.startsWith('/api/team/overview-model')) return jsonResponse(fixtures.overviewModel);
    if (url.startsWith('/api/team/overview')) return jsonResponse(fixtures.overview);
    if (url.startsWith('/api/team/activity')) return jsonResponse(fixtures.activity);
    if (url.startsWith('/api/team/read')) {
      const op = (body?.query as { op?: string } | undefined)?.op;
      if (op === 'work.snapshot') return jsonResponse(fixtures.snapshot);
      if (op === 'review.rounds') return jsonResponse(fixtures.reviewRounds);
      if (op === 'review.inspect') return jsonResponse(fixtures.reviewInspect);
      if (op === 'dependencies.list') return jsonResponse(fixtures.dependencies);
      if (op === 'handoff.list') return jsonResponse(fixtures.handoffs);
      if (op === 'documents.list') return jsonResponse(fixtures.documents);
      if (op === 'goals.list') return jsonResponse(fixtures.goals);
      if (op === 'lessons.list') return jsonResponse(fixtures.lessons);
    }
    return jsonResponse({ ok: false, error: { code: 'Unsupported', message: 'not mocked' } }, 501);
  };
}

function renderTaskPage(
  initialEntry = '/tasks/T-102?scope=P-001',
  handler?: FetchMockHandler,
) {
  const fixtures = canonicalFixtures();
  const mockFn = handler ?? defaultHandler(fixtures);
  const fetchSpy = vi.spyOn(globalThis, 'fetch').mockImplementation(async (input, init) => {
    const url = typeof input === 'string' ? input : input instanceof URL ? input.toString() : input.url;
    const body = typeof init?.body === 'string' ? JSON.parse(init.body) as Record<string, unknown> : null;
    return mockFn(url, body);
  });
  const queryClient = createAppQueryClient({ retry: false });
  const router = createMemoryRouter(
    [
      {
        path: '*',
        element: (
          <ScopeProvider>
            <DisplayTimeZoneProvider timeZone="UTC">
              <TaskPage />
            </DisplayTimeZoneProvider>
          </ScopeProvider>
        ),
      },
      {
        path: '/tasks/:task',
        element: (
          <ScopeProvider>
            <DisplayTimeZoneProvider timeZone="UTC">
              <TaskPage />
            </DisplayTimeZoneProvider>
          </ScopeProvider>
        ),
      },
      { path: '/people/:id', element: <div data-testid="dest-person">person</div> },
      { path: '/goals/:goal', element: <div data-testid="dest-goal">goal</div> },
      { path: '/projects/:project', element: <div data-testid="dest-project">project</div> },
      { path: '/documents/:document', element: <div data-testid="dest-doc">doc</div> },
      { path: '/lessons', element: <div data-testid="dest-lessons">lessons</div> },
      { path: '/inbox', element: <div data-testid="dest-inbox">inbox</div> },
    ],
    { initialEntries: [initialEntry] },
  );

  const view = render(
    <QueryClientProvider client={queryClient}>
      <RevocationProvider>
        <CraftThemeProvider>
          <RouterProvider router={router} />
        </CraftThemeProvider>
      </RevocationProvider>
    </QueryClientProvider>,
  );

  return { ...view, queryClient, router, fetchSpy, fixtures };
}

beforeEach(async () => {
  await i18n.changeLanguage('zh-CN');
});

afterEach(async () => {
  vi.restoreAllMocks();
  await i18n.changeLanguage('zh-CN');
});

async function waitForSettled(queryClient: ReturnType<typeof createAppQueryClient>) {
  await waitFor(() => {
    expect(queryClient.isFetching()).toBe(0);
  });
}

describe('TaskPage (AWR-PPO-050)', () => {
  it('renders header, left column sections in order, right column sections in order, and omits technical details', async () => {
    const { container, queryClient } = renderTaskPage();

    await waitFor(() => {
      expect(screen.getByTestId('task-title').textContent).toBe('接口第 3 版：提交与错误反馈');
      expect(screen.getByTestId('task-acceptance-count').textContent).toBe('3');
      expect(screen.getByTestId('task-lesson-row')).toBeTruthy();
    });
    await waitForSettled(queryClient);

    // Criterion 1: Header elements
    expect(screen.getByTestId('task-summary').textContent).toBe(
      '在等复核，已经 2 天。3 条完成标准里 2 条有依据，1 条等复核人确认。',
    );
    expect(within(screen.getByTestId('task-meta')).getByText('等复核')).toBeTruthy();
    expect(screen.getByTestId('task-owner-link').textContent).toContain('陈宇');
    expect(screen.getByTestId('task-agent-chip').textContent).toContain('Codex 在做');
    expect(screen.getByTestId('task-next-step').textContent).toBe('下一步：等许宁复核；通过后林悦的页面联调改用第 3 版');

    // Criterion 2: Left column section order
    const leftHeadings = [...screen.getByTestId('task-col-left').querySelectorAll('.obs-section-title')]
      .map((el) => el.textContent);
    expect(leftHeadings).toEqual(['完成标准', '最近一次报告', '交付与复核', '经过']);

    // Report must state self-reported disclaimer and display completed, blockers, tests, artifacts
    expect(screen.getByTestId('task-report-attribution').textContent).toContain('这是它自己说的');
    const reportText = screen.getByTestId('task-section-report').textContent ?? '';
    expect(reportText).toContain('做完了');
    expect(reportText).toContain('补重复提交的判断；补错误码对照表');
    expect(reportText).toContain('卡在');
    expect(reportText).toContain('在等复核');
    expect(reportText).toContain('测试');
    expect(reportText).toContain('提交成功 通过');
    expect(reportText).toContain('产出');
    expect(reportText).toContain('接口第 3 版说明、测试报告');

    // Delivery section must show unrecorded version (never guessed from round count), checks, integration, and review rounds
    const deliveryText = screen.getByTestId('task-section-delivery').textContent ?? '';
    expect(deliveryText).toContain('未记录');
    expect(deliveryText).toContain('单元测试 通过');
    expect(deliveryText).toContain('复核通过后才能合入');
    expect(deliveryText).toContain('第 1 轮');
    expect(deliveryText).toContain('许宁，独立复核');

    // Criterion 3: Right column section order
    const rightHeadings = [...screen.getByTestId('task-col-right').querySelectorAll('.obs-section-title')]
      .map((el) => el.textContent);
    expect(rightHeadings).toEqual(['现在卡在哪', '为什么做', '上下游', '要读的资料', '用到的经验']);

    // Criterion 4: No collapsed technical details and no raw copy controls (after full load)
    expect(container.querySelector('details')).toBeNull();
    expect(container.textContent).not.toContain('技术细节');
    expect(container.textContent).not.toContain('Technical details');
    expect(container.textContent).not.toContain('复制原始');
    expect(container.textContent).not.toContain('T-102');
    expect(container.textContent).not.toContain('P-001');
    expect(container.textContent).not.toContain('WS-002');
    expect(container.textContent).not.toContain('sha256');
    expect(findHumanLanguageViolations(container)).toEqual([]);

    // Freshness badge & coverage footer preserved
    expect(screen.getByTestId('freshness-badge')).toBeTruthy();
    expect(screen.getByTestId('task-coverage-summary').textContent).toContain('数据读取情况：');
  });

  it('shows "尚无逐条依据" when PPD-040 per-criterion evidence is absent', async () => {
    const fixtures = canonicalFixtures();
    delete (fixtures.snapshot.result.data as Record<string, unknown>).acceptance_evidence;

    const { container, queryClient } = renderTaskPage('/tasks/T-102?scope=P-001', defaultHandler(fixtures));
    await waitFor(() => {
      expect(screen.getByTestId('task-no-criterion-evidence-tag')).toBeTruthy();
    });
    await waitForSettled(queryClient);
    expect(screen.getByTestId('task-no-criterion-evidence-tag').textContent).toBe('尚无逐条依据');
    expect(screen.getByTestId('task-acceptance-note').textContent).toContain('尚无逐条依据');
    expect(screen.getAllByText('尚无依据')).toHaveLength(3);
    expect(within(screen.getByTestId('task-section-acceptance')).queryByText('有依据')).toBeNull();
    expect(findHumanLanguageViolations(container)).toEqual([]);
  });

  it('renders all PPD-040 criterion states (supported, failed, waiting, no_bound_check, not_started)', async () => {
    const fixtures = canonicalFixtures();
    (fixtures.snapshot.result.data as Record<string, unknown>).acceptance_evidence = {
      status: 'provided',
      basis: 'contract_examples_and_current_candidate',
      candidate_digest: 'a'.repeat(64),
      candidate_current: true,
      review_state: 'rejected',
      review_current_candidate: true,
      facts_truncated: false,
      items: [
        {
          criterion: '标准一',
          status: 'supported',
          reason: 'bound_checks_passed_and_review_approved',
          bound_checks: ['单元测试'],
          checks: [{ check: '单元测试', outcome: 'passed', run_id: 'r1' }],
          examples_count: 1,
          positive_examples: 1,
          negative_examples: 0,
        },
        {
          criterion: '标准二',
          status: 'failed',
          reason: 'review_returned',
          bound_checks: ['接口检查'],
          checks: [{ check: '接口检查', outcome: 'passed', run_id: 'r2' }],
          examples_count: 1,
          positive_examples: 1,
          negative_examples: 0,
        },
        {
          criterion: '标准三',
          status: 'waiting',
          reason: 'awaiting_bound_checks',
          bound_checks: ['压测'],
          checks: [{ check: '压测', outcome: 'not_recorded', run_id: null }],
          examples_count: 1,
          positive_examples: 1,
          negative_examples: 0,
        },
        {
          criterion: '标准四',
          status: 'no_bound_check',
          reason: 'no_bound_check',
          bound_checks: [],
          checks: [],
          examples_count: 1,
          positive_examples: 1,
          negative_examples: 0,
        },
        {
          criterion: '标准五',
          status: 'not_started',
          reason: 'work_not_started',
          bound_checks: ['回放检查'],
          checks: [{ check: '回放检查', outcome: 'not_recorded', run_id: null }],
          examples_count: 1,
          positive_examples: 1,
          negative_examples: 0,
        },
      ],
    };
    const { container, queryClient } = renderTaskPage('/tasks/T-102?scope=P-001', defaultHandler(fixtures));
    await waitFor(() => {
      expect(screen.getByTestId('task-acceptance-count').textContent).toBe('5');
    });
    await waitForSettled(queryClient);
    const acc = screen.getByTestId('task-section-acceptance');
    expect(acc.querySelectorAll('.obs-acceptance-item')).toHaveLength(5);
    expect(within(acc).getByText('有依据')).toBeTruthy();
    expect(within(acc).getByText('没通过')).toBeTruthy();
    expect(within(acc).getByText('在等')).toBeTruthy();
    expect(within(acc).getAllByText('还没有开始')).toHaveLength(2);
    expect(within(acc).getByText('复核已退回，需修改后重审')).toBeTruthy();
    expect(within(acc).getByText('未绑定检查，尚无逐条依据')).toBeTruthy();
    expect(findHumanLanguageViolations(container)).toEqual([]);
  });

  it('matches every displayed count to its detail item count and navigates on click', async () => {
    const user = userEvent.setup();
    const { router, queryClient } = renderTaskPage();

    await waitFor(() => {
      expect(screen.getByTestId('task-acceptance-count').textContent).toBe('3');
      expect(screen.getByTestId('task-lesson-row')).toBeTruthy();
    });
    await waitForSettled(queryClient);

    // Acceptance count === list items
    expect(screen.getByTestId('task-acceptance-count').textContent).toBe('3');
    expect(screen.getByTestId('task-section-acceptance').querySelectorAll('.obs-acceptance-item')).toHaveLength(3);

    // Timeline count === list items
    expect(screen.getByTestId('task-timeline-count').textContent).toBe('3');
    expect(screen.getByTestId('task-section-timeline').querySelectorAll('.obs-timeline-item')).toHaveLength(3);

    // Deps count === list items
    expect(screen.getByTestId('task-deps-count').textContent).toBe('1');
    expect(screen.getAllByTestId('task-dep-downstream')).toHaveLength(1);

    // Docs count === list items
    expect(screen.getByTestId('task-docs-count').textContent).toBe('2');
    expect(screen.getAllByTestId('task-doc-row')).toHaveLength(2);

    // Lessons count === list items
    expect(screen.getByTestId('task-lessons-count').textContent).toBe('1');
    expect(screen.getAllByTestId('task-lesson-row')).toHaveLength(1);

    // Behind count expands to exact 3 items and clicking navigates
    expect(screen.getByTestId('task-behind-toggle').textContent).toBe('后面压着 3 件（算上它自己）');
    await user.click(screen.getByTestId('task-behind-toggle'));
    const behindItems = screen.getAllByTestId('task-behind-item');
    expect(behindItems).toHaveLength(3);

    await user.click(behindItems[1]!);
    await waitFor(() => {
      expect(router.state.location.pathname).toBe('/tasks/T-103');
    });
  });

  it('navigates from owner, blocker person, goal, project, dependency, document, and lesson rows', async () => {
    const user = userEvent.setup();

    // Owner -> /people/M-003
    const ownerView = renderTaskPage();
    await waitFor(() => expect(screen.getByTestId('task-owner-link')).toBeTruthy());
    await user.click(screen.getByTestId('task-owner-link'));
    await waitFor(() => expect(screen.getByTestId('dest-person')).toBeTruthy());
    expect(ownerView.router.state.location.pathname).toBe('/people/M-003');
    ownerView.unmount();

    // Blocker person (许宁) -> /people/M-004
    const blockerView = renderTaskPage();
    await waitFor(() => expect(screen.getByTestId('task-blocker-person-link')).toBeTruthy());
    await user.click(screen.getByTestId('task-blocker-person-link'));
    await waitFor(() => expect(screen.getByTestId('dest-person')).toBeTruthy());
    expect(blockerView.router.state.location.pathname).toBe('/people/M-004');
    blockerView.unmount();

    // Goal -> /goals/GOAL-001
    const goalView = renderTaskPage();
    await waitFor(() => expect(screen.getByTestId('task-why-goal')).toBeTruthy());
    await user.click(screen.getByTestId('task-why-goal'));
    await waitFor(() => expect(screen.getByTestId('dest-goal')).toBeTruthy());
    expect(goalView.router.state.location.pathname).toBe('/goals/GOAL-001');
    goalView.unmount();

    // Project -> /projects/P-001
    const projView = renderTaskPage();
    await waitFor(() => expect(screen.getByTestId('task-why-project')).toBeTruthy());
    await user.click(screen.getByTestId('task-why-project'));
    await waitFor(() => expect(screen.getByTestId('dest-project')).toBeTruthy());
    expect(projView.router.state.location.pathname).toBe('/projects/P-001');
    projView.unmount();

    // Dependency -> /tasks/T-103
    const depView = renderTaskPage();
    await waitFor(() => expect(screen.getByTestId('task-dep-downstream')).toBeTruthy());
    await user.click(screen.getByTestId('task-dep-downstream'));
    await waitFor(() => expect(depView.router.state.location.pathname).toBe('/tasks/T-103'));
    depView.unmount();

    // Document -> /documents/...
    const docView = renderTaskPage();
    await waitFor(() => expect(screen.getAllByTestId('task-doc-row')).toHaveLength(2));
    await user.click(screen.getAllByTestId('task-doc-row')[0]!);
    await waitFor(() => expect(screen.getByTestId('dest-doc')).toBeTruthy());
    expect(docView.router.state.location.pathname).toMatch(/^\/documents\//);
    docView.unmount();

    // Lesson -> /lessons
    const lessonView = renderTaskPage();
    await waitFor(() => expect(screen.getByTestId('task-lesson-row')).toBeTruthy());
    await user.click(screen.getByTestId('task-lesson-row'));
    await waitFor(() => expect(screen.getByTestId('dest-lessons')).toBeTruthy());
    expect(lessonView.router.state.location.pathname).toBe('/lessons');
    lessonView.unmount();
  });

  it('shows contract mismatch warning, review.rounds details, and live dependency fallback when canonical helpers are absent', async () => {
    const fixtures = canonicalFixtures();
    fixtures.snapshot.result.data.observation.progress.contract_matches_current = false;
    delete (fixtures.snapshot.result.data as Record<string, unknown>).dependency_details;
    (fixtures.dependencies.result.data as { items: unknown[] }).items = [
      {
        work_id: 'T-102',
        work_title: '接口第 3 版：提交与错误反馈',
        provider_work_id: 'T-101',
        provider_title: '订单表结构升级',
        dependency_position: 1,
        version_policy: 'fixed_delivery',
        review_assurance: null,
        required_version: null,
        available_version: null,
        adopted_version: null,
        consumed_version: null,
        adoption_valid: true,
        validity: 'valid',
        reason: null,
        missing: {},
      },
      {
        work_id: 'T-103',
        work_title: '订单页面联调',
        provider_work_id: 'T-102',
        provider_title: '接口第 3 版：提交与错误反馈',
        dependency_position: 2,
        version_policy: 'fixed_delivery',
        review_assurance: null,
        required_version: null,
        available_version: null,
        adopted_version: null,
        consumed_version: null,
        adoption_valid: false,
        validity: 'unavailable',
        reason: null,
        missing: {},
      },
    ];
    fixtures.overview.works.push({
      key: 'T-101',
      title: '订单表结构升级',
      status: 'completed',
      owner_person: '陈宇',
      agent: null,
      next_step: null,
      detail_loaded: true,
      workstream_id: 'WS-002',
      contract_hash: 'e'.repeat(64),
    });
    fixtures.reviewRounds.result.data.items = [
      {
        round_id: 'round-t-102-1',
        round_index: 1,
        round_position: 1,
        readable: true,
        state: 'rejected',
        contract_matches_current: true,
        reviewer_role: 'independent_reviewer',
        reviewer_actor_id: 'xu-ning',
        reviewer_person_id: 'M-004',
        decided_at_unix_ms: NOW_UNIX_MS - DAY,
        reason_first_line: '缺少超时重发测试用例',
      },
    ];

    const { container, queryClient } = renderTaskPage('/tasks/T-102?scope=P-001', defaultHandler(fixtures));
    await waitFor(() => {
      expect(screen.getByTestId('task-report-contract-note')).toBeTruthy();
      expect(screen.getByTestId('task-deps-count').textContent).toBe('2');
    });
    await waitForSettled(queryClient);
    expect(screen.getByTestId('task-section-delivery').textContent).toContain('退回');
    expect(screen.getByTestId('task-section-delivery').textContent).toContain('缺少超时重发测试用例');
    expect(screen.getAllByTestId('task-dep-upstream')).toHaveLength(1);
    expect(screen.getAllByTestId('task-dep-downstream')).toHaveLength(1);
    expect(findHumanLanguageViolations(container)).toEqual([]);
  });

  it('handles non-blocked verified task cleanly', async () => {
    const verifiedFixtures = canonicalFixtures();
    verifiedFixtures.overviewModel.input.tasks.items[0]!.verifiedAtUnixMs = NOW_UNIX_MS - DAY;
    verifiedFixtures.overviewModel.input.tasks.items[0]!.integrated = true;
    verifiedFixtures.overviewModel.input.tasks.items[0]!.acceptance = [
      { evidence: [{ passed: true, current: true }] },
      { evidence: [{ passed: true, current: true }] },
      { evidence: [{ passed: true, current: true }] },
    ];
    verifiedFixtures.overviewModel.input.reviewRounds.items[0]!.state = 'approved';
    verifiedFixtures.overview.works[0]!.status = 'completed';
    verifiedFixtures.snapshot.result.data.runtime.state = 'completed';
    verifiedFixtures.snapshot.result.data.observation.collaboration.review = { id: 'round-t-102-1', state: 'approved', current_candidate: true };
    verifiedFixtures.snapshot.result.data.observation.collaboration.integration = { state: 'confirmed', current_binding: true };

    const v1 = renderTaskPage('/tasks/T-102?scope=P-001', defaultHandler(verifiedFixtures));
    await waitForSettled(v1.queryClient);
    expect(screen.getByTestId('task-blocker-empty')).toBeTruthy();
    expect(screen.getByTestId('task-summary').textContent).toContain('通过');
    expect(findHumanLanguageViolations(v1.container)).toEqual([]);
  });

  it('links open decision wait to inbox', async () => {
    const decisionFixtures = canonicalFixtures();
    decisionFixtures.overviewModel.input.reviewRounds.items = [];
    (decisionFixtures.overviewModel.input.decisions as { items: unknown[] }).items = [
      {
        id: 'DEC-001',
        taskId: 'T-102',
        state: 'open',
        current: true,
        question: '超时重发要不要设上限',
        allowedResponderIds: ['M-004'],
        createdAtUnixMs: NOW_UNIX_MS - DAY,
      },
    ];
    const user = userEvent.setup();
    const v2 = renderTaskPage('/tasks/T-102?scope=P-001', defaultHandler(decisionFixtures));
    await waitForSettled(v2.queryClient);
    expect(screen.getByTestId('task-blocker-inbox-link')).toBeTruthy();
    expect(findHumanLanguageViolations(v2.container)).toEqual([]);
    await user.click(screen.getByTestId('task-blocker-inbox-link'));
    await waitFor(() => expect(screen.getByTestId('dest-inbox')).toBeTruthy());
    expect(v2.router.state.location.pathname).toBe('/inbox');
  });

  it('links pending assignment handoff to inbox', async () => {
    const assignFixtures = canonicalFixtures();
    assignFixtures.overviewModel.input.reviewRounds.items = [];
    (assignFixtures.overviewModel.input.handoffs as { items: unknown[] }).items = [
      {
        id: 'HO-001',
        taskId: 'T-102',
        kind: 'responsibility',
        pending: true,
        fromPersonId: 'M-004',
        toPersonId: 'M-003',
        updatedAtUnixMs: NOW_UNIX_MS - DAY,
      },
    ];
    const user = userEvent.setup();
    const v3 = renderTaskPage('/tasks/T-102?scope=P-001', defaultHandler(assignFixtures));
    await waitForSettled(v3.queryClient);
    expect(screen.getByTestId('task-blocker-assign-link')).toBeTruthy();
    expect(findHumanLanguageViolations(v3.container)).toEqual([]);
    await user.click(screen.getByTestId('task-blocker-assign-link'));
    await waitFor(() => expect(screen.getByTestId('dest-inbox')).toBeTruthy());
  });

  it.each(['zh-CN', 'en-US'] as const)('renders all 7 states cleanly in %s with zero human-language violations', async (lang) => {
    await i18n.changeLanguage(lang);

    // 1. Loading
    const loadingView = renderTaskPage('/tasks/T-102?scope=P-001', () => new Promise(() => {}));
    expect(screen.getByTestId('task-loading')).toBeTruthy();
    expect(findHumanLanguageViolations(loadingView.container)).toEqual([]);
    loadingView.unmount();

    // 2. Empty (not found)
    const emptyFixtures = canonicalFixtures();
    emptyFixtures.overview.works = [];
    emptyFixtures.overviewModel.input.tasks.items = [];
    const emptyView = renderTaskPage('/tasks/T-999?scope=P-001', defaultHandler(emptyFixtures));
    await waitFor(() => expect(screen.getByTestId('task-empty')).toBeTruthy());
    await waitForSettled(emptyView.queryClient);
    expect(findHumanLanguageViolations(emptyView.container)).toEqual([]);
    emptyView.unmount();

    // 3. Failed
    const failedView = renderTaskPage('/tasks/T-102?scope=P-001', (url) => {
      if (url.startsWith('/api/team/projects')) return jsonResponse(canonicalFixtures().projects);
      return jsonResponse({ ok: false, error: { code: 'Unavailable', message: 'down' } }, 503);
    });
    await waitFor(() => expect(screen.getByTestId('task-failed')).toBeTruthy());
    await waitForSettled(failedView.queryClient);
    expect(screen.getByTestId('freshness-badge')).toBeTruthy();
    expect(findHumanLanguageViolations(failedView.container)).toEqual([]);
    failedView.unmount();

    // 4. Partial failure (snapshot fails while overview/model stay visible)
    const partialBase = canonicalFixtures();
    const partialView = renderTaskPage('/tasks/T-102?scope=P-001', (url, body) => {
      if (url.startsWith('/api/team/read') && (body?.query as { op?: string })?.op === 'work.snapshot') {
        return jsonResponse({ ok: false, error: { code: 'Unavailable', message: 'snapshot down' } }, 503);
      }
      return defaultHandler(partialBase)(url, body);
    });
    await waitFor(() => expect(screen.getByTestId('task-partial')).toBeTruthy());
    await waitForSettled(partialView.queryClient);
    expect(screen.getByTestId('task-title').textContent).toBe('接口第 3 版：提交与错误反馈');
    expect(findHumanLanguageViolations(partialView.container)).toEqual([]);
    partialView.unmount();

    // 5. Unknown outcome (incomparable ordering / incomplete facts)
    const unknownBase = canonicalFixtures();
    (unknownBase.overviewModel as Record<string, unknown>).ordering = 'incomparable';
    const unknownView = renderTaskPage('/tasks/T-102?scope=P-001', defaultHandler(unknownBase));
    await waitFor(() => expect(screen.getByTestId('task-unknown')).toBeTruthy());
    await waitForSettled(unknownView.queryClient);
    expect(findHumanLanguageViolations(unknownView.container)).toEqual([]);
    unknownView.unmount();

    // 6. Revoked (403 clears restricted content immediately)
    const revokedView = renderTaskPage();
    await waitFor(() => expect(screen.getByTestId('task-title')).toBeTruthy());
    revokeObservationReads(revokedView.queryClient);
    await waitFor(() => expect(screen.getByTestId('state-forbidden')).toBeTruthy());
    expect(screen.queryByTestId('task-title')).toBeNull();
    expect(findHumanLanguageViolations(revokedView.container)).toEqual([]);
    revokedView.unmount();

    // 7. Data (complete)
    const dataView = renderTaskPage();
    await waitFor(() => {
      expect(screen.getByTestId('task-acceptance-count')).toBeTruthy();
      expect(screen.getByTestId('task-lesson-row')).toBeTruthy();
    });
    await waitForSettled(dataView.queryClient);
    expect(findHumanLanguageViolations(dataView.container)).toEqual([]);
    dataView.unmount();
  });

  it('renders verbatim live-sample snapshot shape without undefined, null, NaN, or raw fields', async () => {
    const liveFixtures = canonicalFixtures();
    // Replace snapshot with verbatim live-sample structure (no canonical-only fields, raw ULIDs & hashes & refs)
    liveFixtures.snapshot = {
      ok: true,
      result: {
        data: {
          work_id: 'issue-filter-api',
          contract_hash: '295b63612f58425c516a25c6e4d5d777d1da6661cc11cce9fb93ffc3c9597c4d',
          context_complete: true,
          next_step: null,
          runtime: { state: 'claimed', work_version: '1' },
          visible_contract: {
            goals: ['Support issue filtering across open tickets'],
            acceptance: ['Filter by severity returns matching issues'],
          },
          required_specs: [
            { path: 'docs/support.md', sha256: 'c101fd5c70986de856c0bd30d2da271b71074408acbec05fabc9fa095e1b80ac' },
          ],
          observation: {
            observed_at_unix_ms: 1793513851470,
            checkpoint: {
              id: '01M6AVV25TR88DZ0GPNQ6F9X3Y',
              created_at_unix_ms: 1793513588923,
              contract_matches_current: true,
              next_action: 'Wait for independent review of round 01M6AVQ870N7MBRJ5MPCN79T55. Do not self-approve or merge to main.',
              open_loops: [
                'Review round 01M6AVQ870N7MBRJ5MPCN79T55 is open and acceptance_ready is false.',
                'Integration observation is pending; main remains a516342436e434d0072ef9c138f419f2f128caac.',
              ],
            },
            responsibility: {
              owner: 'lin',
              owner_person_id: 'lin',
              owner_name: 'Lin',
              independent_reviewer: null,
            },
            client: { product: 'Grok' },
            progress: {
              phase: 'ready_for_review',
              summary: 'Implemented filter endpoint and submitted round 01M6AVQ870N7MBRJ5MPCN79T55.',
              completed: [
                'Added severity filter',
                'Published refs/heads/issue-filter-api and submitted the report plus both manifest files.',
              ],
              blockers: ['Review round 01M6AVQ870N7MBRJ5MPCN79T55 is open'],
              tests: [{ name: 'local_git.manifest', outcome: 'passed' }],
              artifacts: [
                { label: 'development branch', reference: 'refs/heads/issue-filter-api@5f7eb2fe9c563e4d355aa64a527e0aef754116c1' },
                { label: 'review round', reference: '01M6AVQ870N7MBRJ5MPCN79T55' },
              ],
              origin: { actor_id: 'lin-agent-native-20261101T045929Z-ee9ddb44e6e7-4856fa', actor_name: 'Lin Agent' },
              provenance: 'caller_declared',
              reported_at_unix_ms: 1793513588923,
              contract_matches_current: true,
              stale: false,
              next_action: 'Wait for independent review of round 01M6AVQ870N7MBRJ5MPCN79T55.',
            },
            collaboration: {
              delivery_required: true,
              candidate: { current: true, required_checks: ['local_git.manifest'] },
              review: { id: '01M6AVQ870N7MBRJ5MPCN79T55', state: 'open', current_candidate: true },
              integration: null,
              publication: null,
              verification: [{ check: 'local_git.manifest', outcome: 'passed' }],
            },
          },
        } as unknown as typeof liveFixtures.snapshot.result.data,
      },
      freshness: MEASURED_FRESHNESS,
    };
    liveFixtures.overview.works = [{
      key: 'issue-filter-api',
      title: 'Issue Filter API',
      status: 'claimed',
      owner_person: 'Lin',
      agent: 'Grok',
      next_step: null,
      detail_loaded: true,
      workstream_id: '013FE5E5116E5A8987D377C479',
      contract_hash: '295b63612f58425c516a25c6e4d5d777d1da6661cc11cce9fb93ffc3c9597c4d',
    }];
    liveFixtures.overviewModel.input.tasks.items = [];
    liveFixtures.documents.result.data.items = [];

    const { container, queryClient } = renderTaskPage('/tasks/issue-filter-api?scope=P-001', (url, body) => {
      const op = (body?.query as { op?: string } | undefined)?.op;
      if (op === 'goals.list' || op === 'lessons.list') {
        return jsonResponse({ ok: false, error: { code: 'Unsupported', message: 'unsupported' } }, 501);
      }
      return defaultHandler(liveFixtures)(url, body);
    });

    await waitFor(() => {
      expect(screen.getByTestId('task-title').textContent).toBe('Issue Filter API');
      expect(screen.getByTestId('task-no-criterion-evidence-tag')).toBeTruthy();
      expect(screen.getByTestId('task-lessons-unsupported')).toBeTruthy();
    });
    await waitForSettled(queryClient);

    const text = container.textContent ?? '';
    expect(text).not.toMatch(/\b(undefined|null|NaN)\b/);
    expect(text).not.toContain('01M6AVQ870N7MBRJ5MPCN79T55');
    expect(text).not.toContain('295b63612f58425c');
    expect(text).not.toContain('a516342436e434d0072ef9c138f419f2f128caac');
    expect(text).not.toContain('5f7eb2fe9c563e4d355aa64a527e0aef754116c1');
    expect(text).not.toContain('caller_declared');
    expect(text).not.toContain('ready_for_review');
    expect(text).not.toContain('acceptance_ready');
    expect(findHumanLanguageViolations(container)).toEqual([]);
  });

  it('never submits any mutating action (read-only surface)', async () => {
    const { container, fetchSpy, queryClient } = renderTaskPage();
    await waitFor(() => {
      expect(screen.getByTestId('task-acceptance-count')).toBeTruthy();
      expect(screen.getByTestId('task-lesson-row')).toBeTruthy();
    });
    await waitForSettled(queryClient);

    // No action/submit calls ever issued
    const actionCalls = fetchSpy.mock.calls.filter(([url]) => String(url).includes('/api/team/action'));
    expect(actionCalls).toHaveLength(0);
    // No form submit or mutation controls
    expect(container.querySelector('form')).toBeNull();
    expect(screen.queryByRole('button', { name: /^(批准|认领|合入|提交复核|派发|Approve|Claim|Merge|Submit|Dispatch)$/i })).toBeNull();
  });

  // --- Supervisor fault-injection tests (Section 4) ---

  it('does not state 0 criteria awaiting reviewer confirmation when all criteria are supported', async () => {
    const fixtures = canonicalFixtures();
    fixtures.snapshot.result.data.acceptance_evidence.items = [
      {
        criterion: '提交成功返回订单编号',
        status: 'supported',
        reason: 'bound_checks_passed_and_review_approved',
        bound_checks: ['提交成功'],
        checks: [{ check: '提交成功', outcome: 'passed', run_id: 'check-1' }],
        examples_count: 1,
        positive_examples: 1,
        negative_examples: 0,
      },
      {
        criterion: '提交失败时返回可读的错误原因',
        status: 'supported',
        reason: 'bound_checks_passed_and_review_approved',
        bound_checks: ['错误分支'],
        checks: [{ check: '错误分支', outcome: 'passed', run_id: 'check-2' }],
        examples_count: 1,
        positive_examples: 1,
        negative_examples: 0,
      },
    ];
    const { queryClient } = renderTaskPage('/tasks/T-102?scope=P-001', defaultHandler(fixtures));
    await waitFor(() => {
      expect(screen.getByTestId('task-acceptance-count').textContent).toBe('2');
    });
    await waitForSettled(queryClient);

    const summary = screen.getByTestId('task-summary').textContent ?? '';
    expect(summary).toBe('在等复核，已经 2 天。');
    expect(summary).not.toContain('0 条等复核人确认');
  });

  it('renders unknown behind count instead of a number when dependency coverage is incomplete', async () => {
    const fixtures = canonicalFixtures();
    // Mark dependencies slice as partial so deriveOverview leaves behind.count = null while keeping known items.
    (fixtures.overviewModel.input.dependencies as Record<string, unknown>).coverage = 'partial';
    (fixtures.overviewModel.input.dependencies as Record<string, unknown>).reason = 'truncated';
    fixtures.overviewModel.coverage.dependencies = { status: 'partial', missing: ['truncated'] };

    const { queryClient } = renderTaskPage('/tasks/T-102?scope=P-001', defaultHandler(fixtures));
    await waitFor(() => {
      expect(screen.getByTestId('task-blocker-kind')).toBeTruthy();
    });
    await waitForSettled(queryClient);

    expect(screen.getByTestId('task-behind-unknown').textContent).toContain('后面压着几件暂时未知');
    expect(screen.queryByTestId('task-behind-toggle')).toBeNull();
    expect(screen.getByTestId('task-section-blocker').textContent).not.toMatch(/后面压着\s*\d+\s*件/);
  });

  it('filters lessons strictly to the current task and excludes unrelated task lessons', async () => {
    const fixtures = canonicalFixtures();
    fixtures.lessons.result.data.items.push({
      lesson_id: 'L-999',
      project_id: 'P-001',
      title: '只属于其他任务的经验',
      stage: 'project',
      author: { person_id: 'M-002', display_name: '林悦' },
      tasks: ['T-103', 'T-106'],
    });
    fixtures.lessons.result.data.total = 2;

    const { container, queryClient } = renderTaskPage('/tasks/T-102?scope=P-001', defaultHandler(fixtures));
    await waitFor(() => {
      expect(screen.getByTestId('task-lesson-row')).toBeTruthy();
    });
    await waitForSettled(queryClient);

    expect(screen.getByTestId('task-lessons-count').textContent).toBe('1');
    expect(screen.getAllByTestId('task-lesson-row')).toHaveLength(1);
    expect(screen.getByTestId('task-section-lessons').textContent).toContain('结果不明确时先查询再重试');
    expect(container.textContent).not.toContain('只属于其他任务的经验');
  });

  it('prioritizes open decision wait ahead of open review wait on the same task', async () => {
    const fixtures = canonicalFixtures();
    // Keep the open review round on T-102 AND add an open decision on T-102 simultaneously.
    (fixtures.overviewModel.input.decisions as { items: unknown[] }).items = [
      {
        id: 'DEC-001',
        taskId: 'T-102',
        state: 'open',
        current: true,
        question: '超时重发要不要设上限',
        allowedResponderIds: ['M-004'],
        createdAtUnixMs: NOW_UNIX_MS - DAY,
      },
    ];

    const { queryClient } = renderTaskPage('/tasks/T-102?scope=P-001', defaultHandler(fixtures));
    await waitFor(() => {
      expect(screen.getByTestId('task-blocker-kind')).toBeTruthy();
    });
    await waitForSettled(queryClient);

    expect(screen.getByTestId('task-blocker-kind').textContent).toBe('等一个决定');
    expect(screen.getByTestId('task-blocker-what').textContent).toContain('超时重发要不要设上限');
    expect(screen.getByTestId('task-blocker-inbox-link')).toBeTruthy();
  });

  // --- Product honesty tests (Section 2) ---

  it('does not fabricate a project goal link when the task has no linked goal', async () => {
    const fixtures = canonicalFixtures();
    fixtures.overviewModel.input.tasks.items[0]!.goalIds = [];
    fixtures.snapshot.result.data.visible_contract.goals = [];

    const { container, queryClient } = renderTaskPage('/tasks/T-102?scope=P-001', defaultHandler(fixtures));
    await waitFor(() => {
      expect(screen.getByTestId('task-why-goal')).toBeTruthy();
    });
    await waitForSettled(queryClient);

    const goalEl = screen.getByTestId('task-why-goal');
    expect(goalEl.tagName).toBe('DIV');
    expect(goalEl.textContent).toContain('尚未关联目标');
    expect(container.textContent).not.toContain('订单从查询到提交在一个页面完成');
  });

  it('treats lessons read failure as partial failure rather than empty and updates footer', async () => {
    const fixtures = canonicalFixtures();
    const { container, queryClient } = renderTaskPage('/tasks/T-102?scope=P-001', (url, body) => {
      if (url.startsWith('/api/team/read') && (body?.query as { op?: string })?.op === 'lessons.list') {
        return jsonResponse({ ok: false, error: { code: 'Unavailable', message: 'lessons down' } }, 503);
      }
      return defaultHandler(fixtures)(url, body);
    });

    await waitFor(() => {
      expect(screen.getByTestId('task-lessons-failed')).toBeTruthy();
    });
    await waitForSettled(queryClient);

    expect(screen.getByTestId('task-partial')).toBeTruthy();
    expect(screen.getByTestId('task-lessons-failed').textContent).toContain('这一块的数据暂时读取不到');
    expect(screen.queryByTestId('task-lessons-empty')).toBeNull();
    expect(container.textContent).not.toContain('这件任务没有带经验');
    const footerText = screen.getByTestId('task-coverage-summary').textContent ?? '';
    expect(footerText).toContain('部分任务记录没有读全');
    expect(footerText).not.toContain('已读取');
  });

  it('treats goals read failure as partial failure rather than unlinked goal and updates footer', async () => {
    const fixtures = canonicalFixtures();
    fixtures.overviewModel.input.tasks.items[0]!.goalIds = [];
    fixtures.snapshot.result.data.visible_contract.goals = [];

    const { container, queryClient } = renderTaskPage('/tasks/T-102?scope=P-001', (url, body) => {
      if (url.startsWith('/api/team/read') && (body?.query as { op?: string })?.op === 'goals.list') {
        return jsonResponse({ ok: false, error: { code: 'Unavailable', message: 'goals down' } }, 503);
      }
      return defaultHandler(fixtures)(url, body);
    });

    await waitFor(() => {
      expect(screen.getByTestId('task-why-goal-failed')).toBeTruthy();
    });
    await waitForSettled(queryClient);

    expect(screen.getByTestId('task-partial')).toBeTruthy();
    expect(screen.getByTestId('task-why-goal-failed').textContent).toContain('目标信息暂时读取不到');
    expect(container.textContent).not.toContain('尚未关联目标');
    const footerText = screen.getByTestId('task-coverage-summary').textContent ?? '';
    expect(footerText).toContain('部分任务记录没有读全');
    expect(footerText).not.toContain('已读取');
  });

  it('does not print complete read coverage when the task is not found', async () => {
    const fixtures = canonicalFixtures();
    fixtures.overview.works = [];
    fixtures.overviewModel.input.tasks.items = [];

    const { queryClient } = renderTaskPage('/tasks/T-999?scope=P-001', defaultHandler(fixtures));
    await waitFor(() => {
      expect(screen.getByTestId('task-empty')).toBeTruthy();
    });
    await waitForSettled(queryClient);

    const footerText = screen.getByTestId('task-coverage-summary').textContent ?? '';
    expect(footerText).toContain('未找到这件任务');
    expect(footerText).not.toContain('任务详情、报告、复核、依赖与动态已读取');
  });

  it('never guesses delivery version from review round count when version is unrecorded', async () => {
    const fixtures = canonicalFixtures();
    fixtures.reviewRounds.result.data.items.push({
      round_id: 'round-t-102-2',
      round_index: 2,
      round_position: 2,
      readable: true,
      state: 'open',
      contract_matches_current: true,
      reviewer_role: 'independent_reviewer',
      reviewer_actor_id: 'M-004-agent',
      reviewer_person_id: 'M-004',
      decided_at_unix_ms: null,
      reason_first_line: null,
    });

    const { queryClient } = renderTaskPage('/tasks/T-102?scope=P-001', defaultHandler(fixtures));
    await waitFor(() => {
      expect(screen.getByTestId('task-section-delivery')).toBeTruthy();
    });
    await waitForSettled(queryClient);

    const deliveryText = screen.getByTestId('task-section-delivery').textContent ?? '';
    expect(deliveryText).toContain('未记录');
    expect(deliveryText).not.toContain('第 2 版');
  });

  it('uses explicit wait kind rather than party name heuristics for external party styling and badges', async () => {
    // Case A: Internal member named "甲方" on a review wait is still an internal person link, not external.
    const internalNamedFixtures = canonicalFixtures();
    internalNamedFixtures.overview.members[2]!.display_name = '甲方';
    internalNamedFixtures.overviewModel.input.members.items[2]!.displayName = '甲方';
    internalNamedFixtures.snapshot.result.data.observation.responsibility.independent_reviewer = {
      person_id: 'M-004',
      display_name: '甲方',
    };
    const vInternal = renderTaskPage('/tasks/T-102?scope=P-001', defaultHandler(internalNamedFixtures));
    await waitFor(() => {
      expect(screen.getByTestId('task-blocker-person-link')).toBeTruthy();
    });
    await waitForSettled(vInternal.queryClient);
    const internalAvatar = screen.getByTestId('task-blocker-person-link').querySelector('.task-avatar-sm');
    expect(internalAvatar?.getAttribute('data-external')).toBeNull();
    expect(internalAvatar?.textContent).toBe('甲');
    vInternal.unmount();

    // Case B: Explicit external wait with a non-"甲方" name gets external badge and static non-link rendering.
    const externalNamedFixtures = canonicalFixtures();
    externalNamedFixtures.overviewModel.input.reviewRounds.items = [];
    (externalNamedFixtures.overviewModel.input.tasks.items[0] as Record<string, unknown>).externalWait = {
      party: '合作方账务组',
      what: '确认结算导出字段',
      sinceUnixMs: NOW_UNIX_MS - 4 * DAY,
    };
    const vExternal = renderTaskPage('/tasks/T-102?scope=P-001', defaultHandler(externalNamedFixtures));
    await waitFor(() => {
      expect(screen.getByTestId('task-blocker-person-static')).toBeTruthy();
    });
    await waitForSettled(vExternal.queryClient);
    expect(screen.queryByTestId('task-blocker-person-link')).toBeNull();
    const externalAvatar = screen.getByTestId('task-blocker-person-static').querySelector('.task-avatar-sm');
    expect(externalAvatar?.getAttribute('data-external')).toBe('true');
    expect(externalAvatar?.textContent).toBe('外');
    vExternal.unmount();
  });

  it('does not infer weekly commitment membership from commitment ID substrings', async () => {
    const { queryClient } = renderTaskPage();
    await waitFor(() => {
      expect(screen.getByTestId('task-why-project')).toBeTruthy();
    });
    await waitForSettled(queryClient);

    const projectRowText = screen.getByTestId('task-why-project').textContent ?? '';
    expect(projectRowText).not.toContain('在本周承诺里');
    expect(projectRowText).not.toContain('不在本周承诺里');
  });

  it('uses i18n copy for review wait without round-prefix rewriting or hardcoded inline strings', async () => {
    await i18n.changeLanguage('en-US');
    const { queryClient } = renderTaskPage();
    await waitFor(() => {
      expect(screen.getByTestId('task-blocker-what')).toBeTruthy();
    });
    await waitForSettled(queryClient);

    expect(screen.getByTestId('task-blocker-what').textContent).toBe('Waiting for review');
    expect(screen.getByTestId('task-blocker-what').textContent).not.toContain('复核');
  });

  it('sorts timeline by timestamp before marking the newest event as current and leaves undated events unmarked', async () => {
    const fixtures = canonicalFixtures();
    // Feed audit.development in ascending (oldest-first) order to prove current is chosen by timestamp, not array order.
    fixtures.activity.data.items = [
      {
        id: 'EV-OLD',
        actor_id: 'M-003-agent',
        client_id: null,
        actor: { person_id: 'M-003', display_name: '陈宇' },
        client: { display_name: null },
        action: 'work.claim',
        work_id: 'T-102',
        result: 'committed',
        created_at_unix_ms: NOW_UNIX_MS - 3 * DAY,
        credential_id: null,
      },
      {
        id: 'EV-MID',
        actor_id: 'M-004-agent',
        client_id: null,
        actor: { person_id: 'M-004', display_name: '许宁' },
        client: { display_name: null },
        action: 'review.approve',
        work_id: 'T-102',
        result: 'committed',
        created_at_unix_ms: NOW_UNIX_MS - 2 * DAY,
        credential_id: null,
      },
      {
        id: 'EV-NEW',
        actor_id: 'M-003-agent',
        client_id: null,
        actor: { person_id: 'M-003', display_name: '陈宇' },
        client: { display_name: null },
        action: 'delivery.submit_and_request_review',
        work_id: 'T-102',
        result: 'committed',
        created_at_unix_ms: NOW_UNIX_MS - 3_600_000,
        credential_id: null,
      },
    ];

    const vSorted = renderTaskPage('/tasks/T-102?scope=P-001', defaultHandler(fixtures));
    await waitFor(() => {
      expect(screen.getByTestId('task-timeline-count').textContent).toBe('3');
    });
    await waitForSettled(vSorted.queryClient);

    const rows = [...screen.getByTestId('task-section-timeline').querySelectorAll('.obs-timeline-item')];
    expect(rows).toHaveLength(3);
    expect(rows[0]?.getAttribute('data-tone')).toBe('current');
    expect(rows[0]?.textContent).toContain('提交');
    expect(rows[1]?.getAttribute('data-tone')).toBe('positive');
    expect(rows[2]?.getAttribute('data-tone')).toBe('default');
    vSorted.unmount();

    // Undated events must never be marked current.
    const undatedFixtures = canonicalFixtures();
    undatedFixtures.activity.data.items = [
      {
        id: 'EV-UNDATED',
        actor_id: 'M-003-agent',
        client_id: null,
        actor: { person_id: 'M-003', display_name: '陈宇' },
        client: { display_name: null },
        action: 'work.claim',
        work_id: 'T-102',
        result: 'committed',
        created_at_unix_ms: Number.NaN,
        credential_id: null,
      },
    ];
    const vUndated = renderTaskPage('/tasks/T-102?scope=P-001', defaultHandler(undatedFixtures));
    await waitFor(() => {
      expect(screen.getByTestId('task-timeline-count').textContent).toBe('1');
    });
    await waitForSettled(vUndated.queryClient);
    const undatedRow = screen.getByTestId('task-section-timeline').querySelector('.obs-timeline-item');
    expect(undatedRow?.getAttribute('data-tone')).toBe('default');
    vUndated.unmount();
  });

  it('never uses an unspaced responsibility.owner string as ownerPersonId for person links', async () => {
    const fixtures = canonicalFixtures();
    fixtures.overviewModel.input.tasks.items = [];
    fixtures.snapshot.result.data.observation.responsibility = {
      owner: '陈宇',
      owner_name: '陈宇',
      independent_reviewer: null,
    } as unknown as typeof fixtures.snapshot.result.data.observation.responsibility;

    const { queryClient } = renderTaskPage('/tasks/T-102?scope=P-001', defaultHandler(fixtures));
    await waitFor(() => {
      expect(screen.getByTestId('task-owner-name')).toBeTruthy();
    });
    await waitForSettled(queryClient);

    expect(screen.getByTestId('task-owner-name').textContent).toContain('陈宇');
    expect(screen.queryByTestId('task-owner-link')).toBeNull();
  });

  // --- Verbatim PPD-040 & PPD-050 shapes (Section 3) ---

  it('treats PPD-040 status "not_provided" as no per-criterion evidence', async () => {
    const fixtures = canonicalFixtures();
    (fixtures.snapshot.result.data as Record<string, unknown>).acceptance_evidence = {
      status: 'not_provided',
      basis: 'contract_examples_absent',
      candidate_digest: null,
      candidate_current: false,
      review_state: null,
      review_current_candidate: false,
      items: null,
    };

    const { container, queryClient } = renderTaskPage('/tasks/T-102?scope=P-001', defaultHandler(fixtures));
    await waitFor(() => {
      expect(screen.getByTestId('task-no-criterion-evidence-tag')).toBeTruthy();
    });
    await waitForSettled(queryClient);

    expect(screen.getByTestId('task-no-criterion-evidence-tag').textContent).toBe('尚无逐条依据');
    expect(findHumanLanguageViolations(container)).toEqual([]);
  });

  it('renders verbatim PPD-050 observation.structured_waits when model waits are absent', async () => {
    const fixtures = canonicalFixtures();
    fixtures.overviewModel.input.reviewRounds.items = [];
    fixtures.snapshot.result.data.observation.structured_waits = {
      status: 'provided',
      items: [
        {
          waiting_on: { kind: 'external_party', name: '合作方运维团队' },
          what: '确认结算导出字段清单',
          since_unix_ms: NOW_UNIX_MS - 3 * DAY,
          release_condition: '收到确认后的字段清单',
        },
      ],
      provenance: 'caller_declared',
      grants_authority: false,
      checkpoint_id: '01M83KMD31SZ2NX0Z01JQKJGGE',
      reported_at_unix_ms: NOW_UNIX_MS - DAY,
      contract_matches_current: true,
    };

    const { container, queryClient } = renderTaskPage('/tasks/T-102?scope=P-001', defaultHandler(fixtures));
    await waitFor(() => {
      expect(screen.getByTestId('task-blocker-kind')).toBeTruthy();
    });
    await waitForSettled(queryClient);

    expect(screen.getByTestId('task-blocker-kind').textContent).toBe('等项目外的一方');
    expect(screen.getByTestId('task-blocker-person-static').textContent).toContain('合作方运维团队');
    expect(screen.getByTestId('task-blocker-person-static').querySelector('.task-avatar-sm')?.getAttribute('data-external')).toBe('true');
    expect(screen.getByTestId('task-blocker-days').textContent).toContain('3 天');
    expect(screen.getByTestId('task-blocker-what').textContent).toContain('确认结算导出字段清单');
    expect(screen.getByTestId('task-behind-unknown')).toBeTruthy();
    expect(findHumanLanguageViolations(container)).toEqual([]);
  });

  it('deriveTaskPageModel reports null behindCount when overview derivation leaves behind.count unknown', () => {
    const fixtures = canonicalFixtures();
    (fixtures.overviewModel.input.dependencies as Record<string, unknown>).coverage = 'partial';
    (fixtures.overviewModel.input.dependencies as Record<string, unknown>).reason = 'truncated';
    const derived = deriveTaskPageModel({
      workId: 'T-102',
      projectKey: 'P-001',
      projectTitle: '订单协作平台',
      overview: fixtures.overview as unknown as Parameters<typeof deriveTaskPageModel>[0]['overview'],
      model: fixtures.overviewModel as unknown as Parameters<typeof deriveTaskPageModel>[0]['model'],
      snapshot: fixtures.snapshot.result.data as unknown as Parameters<typeof deriveTaskPageModel>[0]['snapshot'],
      language: 'zh-CN',
      nowUnixMs: NOW_UNIX_MS,
      timeZone: 'UTC',
      t: i18n.t.bind(i18n),
    });
    expect(derived.blocker.kind).toBe('wait');
    if (derived.blocker.kind === 'wait') {
      expect(derived.blocker.wait.behindCount).toBeNull();
      expect(derived.blocker.wait.behindItems.length).toBeGreaterThan(0);
    }
  });
});
