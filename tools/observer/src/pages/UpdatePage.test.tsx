import { readFileSync } from 'node:fs';
import path from 'node:path';
import { act, fireEvent, screen, waitFor, within } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import i18n from '../i18n';
import { defaultVisibleText, findHumanLanguageViolations } from '../lib/human-language-guard';
import type {
  PeriodUpdateFactSnapshot,
  PeriodUpdateRecord,
  TeamPeriodUpdateResponse,
  TeamPeriodUpdatesResponse,
} from '../lib/types';
import { DEMO_OVERVIEW, DEMO_PROJECTS, renderApp } from '../test-utils';

const NOW_UNIX_MS = 1795089600000; // 2026-11-19T12:00:00Z
const TODAY_0930_UNIX_MS = 1795080600000; // 2026-11-19T09:30:00Z
const NOV_15_UNIX_MS = 1794744000000; // 2026-11-15T12:00:00Z
const NOV_08_UNIX_MS = 1794139200000; // 2026-11-08T12:00:00Z

const FRESH_AT_LIST = Date.now() - 30_000;
const FRESH_AT_INSPECT = Date.now() - 10_000;

function json(payload: unknown) {
  return new Response(JSON.stringify(payload), {
    status: 200,
    headers: { 'content-type': 'application/json' },
  });
}

function sampleUpdates(): PeriodUpdateRecord[] {
  return [
    {
      update_id: 'U-001',
      period_kind: 'week',
      period_key: '2026-W47',
      starts_on: '2026-11-16',
      ends_on: '2026-11-22',
      status: 'risk',
      summary: '接口第 3 版的复核拖了两天，页面联调在等。',
      author: { person_id: 'M-001', display_name: '周宁' },
      recorded_at_unix_ms: TODAY_0930_UNIX_MS,
      version: '1',
      supersedes_update_id: null,
      fact_snapshot_ref: 'F-001',
      current: true,
    },
    {
      update_id: 'U-002',
      period_kind: 'week',
      period_key: '2026-W46',
      starts_on: '2026-11-09',
      ends_on: '2026-11-15',
      status: 'ok',
      summary: '查询接口加缓存层后压测达标，按计划推进。',
      author: { person_id: 'M-001', display_name: '周宁' },
      recorded_at_unix_ms: NOV_15_UNIX_MS,
      version: '1',
      supersedes_update_id: null,
      fact_snapshot_ref: 'F-002',
      current: true,
    },
    {
      update_id: 'U-003',
      period_kind: 'week',
      period_key: '2026-W45',
      starts_on: '2026-11-02',
      ends_on: '2026-11-08',
      status: 'ok',
      summary: '完成拆解：14 件任务，其中 8 件进了本期承诺。',
      author: { person_id: 'M-001', display_name: '周宁' },
      recorded_at_unix_ms: NOV_08_UNIX_MS,
      version: '1',
      supersedes_update_id: null,
      fact_snapshot_ref: 'F-003',
      current: true,
    },
  ];
}

function sampleSnapshot(): PeriodUpdateFactSnapshot {
  return {
    verified: { count: 2, work_ids: ['T-107', 'T-108'] },
    completed_without_independent_review: { count: 0, work_ids: [] },
    waits_opened: {
      count: 6,
      kinds: [
        { kind: 'decision', count: 2, work_ids: ['T-104', 'T-105'] },
        { kind: 'dependency', count: 1, work_ids: ['T-103'] },
        { kind: 'review', count: 3, work_ids: ['T-102', 'T-109', 'T-110'] },
      ],
    },
    waits_cleared: { count: 0, kinds: [] },
    review_returned: { count: 1, work_ids: ['T-105'] },
    delivered_unverified: { count: 1, work_ids: ['T-114'] },
    decisions_made: { count: 0, work_ids: [] },
  };
}

function updatesListReply(
  projectId: string,
  items: PeriodUpdateRecord[],
  overrides: Partial<{
    next_cursor: string | null;
    truncated: boolean;
    at_unix_ms: number;
    coordinator_epoch: string;
  }> = {},
): TeamPeriodUpdatesResponse {
  const at = overrides.at_unix_ms ?? FRESH_AT_LIST;
  const epoch = overrides.coordinator_epoch ?? 'epoch-1';
  const nextCursor = overrides.next_cursor ?? null;
  const truncated = overrides.truncated ?? nextCursor !== null;
  return {
    ok: true,
    result: {
      protocol_version: 1,
      scope_id: 'main',
      coordinator_epoch: epoch,
      project_status: 'active',
      source_snapshot_id: 'snap-1',
      project_revision: '47',
      selection_basis: 'project',
      data: {
        project_id: projectId,
        items,
        next_cursor: nextCursor,
        truncated,
        execution_authorized: false,
        as_of: { at_unix_ms: NOW_UNIX_MS, project_revision: '47', source_snapshot_id: 'snap-1' },
      },
    } as TeamPeriodUpdatesResponse['result'],
    freshness: {
      report_received: { status: 'not_measured' },
      authoritative: { status: 'measured', at_unix_ms: at, project_revision: '47', source_snapshot_id: 'snap-1', coordinator_epoch: epoch },
      page_read: { status: 'measured', at_unix_ms: at },
    },
  };
}

function updateInspectReply(
  projectId: string,
  update: PeriodUpdateRecord,
  snapshot: PeriodUpdateFactSnapshot | null,
  overrides: Partial<{
    hidden_work_count: number;
    at_unix_ms: number;
    coordinator_epoch: string;
    authoritativeStatus: 'measured' | 'stale' | 'not_measured' | 'failed';
  }> = {},
): TeamPeriodUpdateResponse {
  const at = overrides.at_unix_ms ?? FRESH_AT_INSPECT;
  const epoch = overrides.coordinator_epoch ?? 'epoch-1';
  const hidden = overrides.hidden_work_count ?? 0;
  const hasSnapshot = update.fact_snapshot_ref !== null && snapshot !== null;
  const authStatus = overrides.authoritativeStatus ?? 'measured';
  return {
    ok: true,
    result: {
      protocol_version: 1,
      scope_id: 'main',
      coordinator_epoch: epoch,
      project_status: 'active',
      source_snapshot_id: 'snap-1',
      project_revision: '47',
      selection_basis: 'project',
      data: {
        project_id: projectId,
        update,
        fact_snapshot: hasSnapshot ? snapshot : null,
        fact_snapshot_coverage: hasSnapshot ? { complete: hidden === 0, hidden_work_count: hidden } : null,
        execution_authorized: false,
        as_of: { at_unix_ms: NOW_UNIX_MS, project_revision: '47', source_snapshot_id: 'snap-1' },
      },
    } as TeamPeriodUpdateResponse['result'],
    freshness: {
      report_received: { status: 'not_measured' },
      authoritative: {
        status: authStatus,
        ...(authStatus === 'measured' || authStatus === 'stale' ? { at_unix_ms: at } : {}),
        project_revision: '47',
        source_snapshot_id: 'snap-1',
        coordinator_epoch: epoch,
      },
      page_read: { status: 'measured', at_unix_ms: at },
    },
  };
}

function sampleOverview(project = 'P-001') {
  return {
    ...DEMO_OVERVIEW,
    project,
    works: [
      { key: 'T-102', title: '接口第 3 版：提交与错误反馈', status: 'review', owner_person: null, agent: null, next_step: null, detail_loaded: true, workstream_id: 'WS-002', contract_hash: 'h1' },
      { key: 'T-103', title: '订单页面联调', status: 'waiting', owner_person: null, agent: null, next_step: null, detail_loaded: true, workstream_id: 'WS-001', contract_hash: 'h2' },
      { key: 'T-104', title: '空状态检查', status: 'stalled', owner_person: null, agent: null, next_step: null, detail_loaded: true, workstream_id: 'WS-001', contract_hash: 'h3' },
      { key: 'T-105', title: '登录异常处理', status: 'returned', owner_person: null, agent: null, next_step: null, detail_loaded: true, workstream_id: 'WS-002', contract_hash: 'h4' },
      { key: 'T-107', title: '订单列表分页', status: 'verified', owner_person: null, agent: null, next_step: null, detail_loaded: true, workstream_id: 'WS-001', contract_hash: 'h5' },
      { key: 'T-108', title: '提交按钮防重复', status: 'verified', owner_person: null, agent: null, next_step: null, detail_loaded: true, workstream_id: 'WS-001', contract_hash: 'h6' },
      { key: 'T-109', title: '提交失败的错误码表', status: 'review', owner_person: null, agent: null, next_step: null, detail_loaded: true, workstream_id: 'WS-002', contract_hash: 'h7' },
      { key: 'T-110', title: '订单状态机说明', status: 'review', owner_person: null, agent: null, next_step: null, detail_loaded: true, workstream_id: 'WS-002', contract_hash: 'h8' },
      { key: 'T-114', title: '错误提示文案', status: 'claimed', owner_person: null, agent: null, next_step: null, detail_loaded: true, workstream_id: 'WS-001', contract_hash: 'h9' },
    ],
    freshness: {
      report_received: { status: 'not_measured' },
      authoritative: { status: 'measured', at_unix_ms: FRESH_AT_INSPECT },
      page_read: { status: 'measured', at_unix_ms: FRESH_AT_INSPECT },
    },
  };
}

function sampleProjects() {
  return {
    ...DEMO_PROJECTS,
    projects: [
      { key: 'P-001', title: '订单协作平台', role: 'lead' },
      { key: 'P-002', title: '统一通知服务', role: 'reader' },
    ],
  };
}

function shellReply() {
  return {
    ok: true,
    source: 'demo',
    organizationName: '示例组织',
    displayTimeZone: 'UTC',
    perspective: null,
  };
}

function installMock(handler: (url: string, init?: RequestInit) => Response | Promise<Response>) {
  const fetchMock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
    const url = String(input);
    if (url.includes('/api/team/shell')) return json(shellReply());
    return handler(url, init);
  });
  vi.stubGlobal('fetch', fetchMock);
  return fetchMock;
}

afterEach(async () => {
  vi.unstubAllGlobals();
  await act(async () => {
    await i18n.changeLanguage('zh-CN');
  });
});

describe('UpdatePage — acceptance criteria 1, 2, 3, 5 (data state and clickable facts)', () => {
  it('renders header, facts table, program-vs-human note, rollup block, previous period with transition, and all updates', async () => {
    const updates = sampleUpdates();
    const snapshot = sampleSnapshot();
    installMock((url) => {
      if (url.includes('/api/team/projects')) return json(sampleProjects());
      if (url.includes('/api/team/updates?')) return json(updatesListReply('P-001', updates));
      if (url.includes('/api/team/update?')) {
        const id = new URL(url, 'http://synthetic.invalid').searchParams.get('id');
        const found = updates.find((item) => item.update_id === id) ?? updates[0];
        return json(updateInspectReply('P-001', found, snapshot));
      }
      if (url.includes('/api/team/overview')) return json(sampleOverview('P-001'));
      return json({ ok: false, error: { code: 'NoRoute', message: url } });
    });

    const { router } = renderApp('/updates/U-001?scope=P-001');

    // Acceptance #1: header title, sentence, status tag, range, author & time
    expect((await screen.findByTestId('update-title')).textContent).toContain('订单协作平台 · 第 47 周的更新');
    expect(screen.getByTestId('update-summary').textContent).toContain('接口第 3 版的复核拖了两天，页面联调在等。');
    expect(screen.getByTestId('update-meta').textContent).toContain('有风险');
    expect(screen.getByTestId('update-range').textContent).toContain('11 月 16 日到 22 日');
    expect(screen.getByTestId('update-author').textContent).toContain('周宁 今天 09:30 在自己的 Agent 里写的');

    // Acceptance #2: left column facts table + rollup block; right column previous + all updates
    const factsSection = screen.getByTestId('update-facts');
    expect(factsSection.textContent).toContain('这一期的事实');
    expect(within(factsSection).getByText('5')).toBeTruthy();
    expect(factsSection.textContent).toContain('数的是什么');
    expect(factsSection.textContent).toContain('多少');
    expect(factsSection.textContent).toContain('是哪些');
    expect(screen.getByTestId('update-fact-row-verified').textContent).toContain('通过核验');
    expect(screen.getByTestId('update-fact-row-verified').textContent).toContain('2 件');
    expect(screen.getByTestId('update-fact-row-verified').textContent).toContain('订单列表分页、提交按钮防重复');
    expect(screen.getByTestId('update-fact-row-waits_opened').textContent).toContain('6 个');
    expect(screen.getByTestId('update-fact-row-waits_opened').textContent).toContain('3 件等复核，2 个等决定，1 件等上游');
    expect(screen.getByTestId('update-fact-row-waits_cleared').textContent).toContain('0 个');
    expect(screen.getByTestId('update-fact-row-waits_cleared').textContent).toContain('没有');
    expect(screen.getByTestId('update-fact-row-review_returned').textContent).toContain('登录异常处理');
    expect(screen.getByTestId('update-fact-row-delivered_unverified').textContent).toContain('错误提示文案');

    // Acceptance #3: program counts vs lead sentence in Agent, platform does not write sentences
    expect(screen.getByTestId('update-facts-note').textContent).toContain(
      '这些数是程序数的。上面那句话，是负责人看着这些数，在自己的 Agent 里写好再记进来的。平台自己不写句子。',
    );

    const rollup = screen.getByTestId('update-rollup');
    expect(rollup.textContent).toContain('这句话往上怎么用');
    expect(rollup.textContent).toContain('部门那一页');
    expect(rollup.textContent).toContain('最高层那一页');

    const prevBlock = screen.getByTestId('update-previous');
    expect(prevBlock.textContent).toContain('上一期');
    expect(prevBlock.textContent).toContain('查询接口加缓存层后压测达标，按计划推进。');
    expect(prevBlock.textContent).toContain('第 46 周 · 正常 → 这一期：有风险');

    const allBlock = screen.getByTestId('update-all');
    expect(allBlock.textContent).toContain('全部更新');
    expect(within(allBlock).getAllByRole('button')).toHaveLength(3);
    expect(within(allBlock).getAllByRole('button')[0].getAttribute('aria-current')).toBe('true');

    // Freshness badge & coverage preserved
    expect(screen.getByTestId('freshness-badge')).toBeTruthy();
    expect(screen.getByTestId('update-coverage-summary').textContent).toContain('已读取本期更新与全部更新记录');

    // Product language clean: no internal ids or forbidden strings
    expect(findHumanLanguageViolations(screen.getByTestId('page-update'))).toEqual([]);
    const pageText = defaultVisibleText(screen.getByTestId('page-update'));
    for (const raw of ['U-001', 'P-001', 'M-001', 'F-001', 'T-107', 'T-108', 'undefined', 'null', 'NaN']) {
      expect(pageText).not.toContain(raw);
    }

    // Acceptance #5: clicking every count opens detail whose row count === number
    const expectedCounts: Array<[string, number]> = [
      ['verified', 2],
      ['waits_opened', 6],
      ['waits_cleared', 0],
      ['review_returned', 1],
      ['delivered_unverified', 1],
    ];
    for (const [key, expected] of expectedCounts) {
      fireEvent.click(screen.getByTestId(`update-fact-count-${key}`));
      const detail = screen.getByTestId('update-fact-detail');
      expect(Number(detail.getAttribute('data-count'))).toBe(expected);
      expect(detail.querySelectorAll('.obs-row-item')).toHaveLength(expected);
    }

    // Clicking a wait-kind entry opens detail with that exact kind count (3, 2, 1)
    fireEvent.click(screen.getByTestId('update-fact-kind-waits_opened-review'));
    const reviewDetail = screen.getByTestId('update-fact-detail');
    expect(Number(reviewDetail.getAttribute('data-count'))).toBe(3);
    expect(reviewDetail.querySelectorAll('.obs-row-item')).toHaveLength(3);

    // Clicking an item inside the detail navigates to its task route
    fireEvent.click(reviewDetail.querySelectorAll('.obs-row-control')[0]);
    expect(router.state.location.pathname).toBe('/tasks/T-102');
    expect(router.state.location.search).toBe('?scope=P-001');
  });

  it('clicks an inline task link in the facts table directly to that task and switches periods via previous / all updates', async () => {
    const updates = sampleUpdates();
    const snapshot = sampleSnapshot();
    installMock((url) => {
      if (url.includes('/api/team/projects')) return json(sampleProjects());
      if (url.includes('/api/team/updates?')) return json(updatesListReply('P-001', updates));
      if (url.includes('/api/team/update?')) {
        const id = new URL(url, 'http://synthetic.invalid').searchParams.get('id');
        const found = updates.find((item) => item.update_id === id) ?? updates[0];
        return json(updateInspectReply('P-001', found, snapshot));
      }
      if (url.includes('/api/team/overview')) return json(sampleOverview('P-001'));
      return json({ ok: false, error: { code: 'NoRoute', message: url } });
    });

    const { router } = renderApp('/updates/U-001?scope=P-001');
    await screen.findByTestId('update-title');

    // Click previous period (U-002): status unchanged from U-003 (ok -> ok)
    fireEvent.click(within(screen.getByTestId('update-previous')).getByRole('button'));
    await waitFor(() => {
      expect(screen.getByTestId('update-title').textContent).toContain('订单协作平台 · 第 46 周的更新');
    });
    expect(screen.getByTestId('update-previous').textContent).toContain('第 45 周 · 正常 · 这一期没有变');

    // Click earliest period (U-003): shows "这是第一期。"
    fireEvent.click(within(screen.getByTestId('update-previous')).getByRole('button'));
    await waitFor(() => {
      expect(screen.getByTestId('update-title').textContent).toContain('订单协作平台 · 第 45 周的更新');
    });
    expect(screen.getByTestId('update-prev-first').textContent).toContain('这是第一期。');

    // Click inline task link directly
    fireEvent.click(screen.getByTestId('update-fact-task-verified-T-107'));
    expect(router.state.location.pathname).toBe('/tasks/T-107');
    expect(router.state.location.search).toBe('?scope=P-001');
  });

  it('navigates back to the project overview via the back button', async () => {
    const updates = sampleUpdates();
    installMock((url) => {
      if (url.includes('/api/team/projects')) return json(sampleProjects());
      if (url.includes('/api/team/updates?')) return json(updatesListReply('P-001', updates));
      if (url.includes('/api/team/update?')) return json(updateInspectReply('P-001', updates[0], sampleSnapshot()));
      if (url.includes('/api/team/overview')) return json(sampleOverview('P-001'));
      return json({ ok: false, error: { code: 'NoRoute', message: url } });
    });

    const { router } = renderApp('/updates/U-001?scope=P-001');
    await screen.findByTestId('update-title');
    fireEvent.click(screen.getByTestId('update-back'));
    expect(router.state.location.pathname).toBe('/projects/P-001');
  });

  it('displays non-zero decisions_made and completed_without_independent_review rows with matching clickable detail counts', async () => {
    const updates = sampleUpdates();
    const extraSnapshot: PeriodUpdateFactSnapshot = {
      ...sampleSnapshot(),
      completed_without_independent_review: { count: 1, work_ids: ['T-114'] },
      decisions_made: { count: 2, work_ids: ['T-104', 'T-105'] },
    };
    installMock((url) => {
      if (url.includes('/api/team/projects')) return json(sampleProjects());
      if (url.includes('/api/team/updates?')) return json(updatesListReply('P-001', updates));
      if (url.includes('/api/team/update?')) return json(updateInspectReply('P-001', updates[0], extraSnapshot));
      if (url.includes('/api/team/overview')) return json(sampleOverview('P-001'));
      return json({ ok: false, error: { code: 'NoRoute', message: url } });
    });

    renderApp('/updates/U-001?scope=P-001');
    await screen.findByTestId('update-fact-row-decisions_made');
    expect(screen.getByTestId('update-fact-row-completed_without_independent_review').textContent).toContain('1 件');
    expect(screen.getByTestId('update-fact-row-decisions_made').textContent).toContain('2 个');
    expect(within(screen.getByTestId('update-facts')).getByText('7')).toBeTruthy();

    fireEvent.click(screen.getByTestId('update-fact-count-decisions_made'));
    const detail = screen.getByTestId('update-fact-detail');
    expect(Number(detail.getAttribute('data-count'))).toBe(2);
    expect(detail.querySelectorAll('.obs-row-item')).toHaveLength(2);
  });
});

describe('UpdatePage — acceptance criterion 4 & 6 (all 7 states in zh-CN and en-US)', () => {
  it.each(['zh-CN', 'en-US'] as const)('1. loading state (%s) never shows calm empty copy', async (lang) => {
    await act(async () => { await i18n.changeLanguage(lang); });
    installMock((url) => {
      if (url.includes('/api/team/projects')) return json(sampleProjects());
      return new Promise(() => {});
    });

    renderApp('/updates/U-001?scope=P-001');
    const loading = await screen.findByTestId('update-loading');
    expect(loading.textContent).toContain(lang === 'zh-CN' ? '正在读取' : 'Loading');
    expect(screen.queryByTestId('update-empty')).toBeNull();
    expect(screen.queryByText(/还没有更新|no updates yet/i)).toBeNull();
  });

  it.each(['zh-CN', 'en-US'] as const)('2. empty project state (%s) explains recording via lead Agent', async (lang) => {
    await act(async () => { await i18n.changeLanguage(lang); });
    installMock((url) => {
      if (url.includes('/api/team/projects')) return json(sampleProjects());
      if (url.includes('/api/team/updates?')) return json(updatesListReply('P-001', []));
      if (url.includes('/api/team/overview')) return json(sampleOverview('P-001'));
      return json({ ok: false, error: { code: 'NoRoute', message: url } });
    });

    renderApp('/updates?scope=P-001');
    const empty = await screen.findByTestId('update-empty');
    if (lang === 'zh-CN') {
      expect(empty.textContent).toContain('这个项目还没有更新');
      expect(empty.textContent).toContain('负责人');
      expect(empty.textContent).toContain('Agent');
    } else {
      expect(empty.textContent).toContain('This project has no updates yet');
      expect(empty.textContent).toContain('project lead');
      expect(empty.textContent).toContain('Agent');
    }
    expect(screen.getByTestId('freshness-badge')).toBeTruthy();
    expect(findHumanLanguageViolations(screen.getByTestId('page-update'))).toEqual([]);
  });

  it.each(['zh-CN', 'en-US'] as const)('3. failed state (%s) never shows empty copy and keeps freshness badge failed', async (lang) => {
    await act(async () => { await i18n.changeLanguage(lang); });
    installMock((url) => {
      if (url.includes('/api/team/projects')) return json(sampleProjects());
      if (url.includes('/api/team/updates?')) return json({ ok: false, error: { code: 'ReadTimeout', message: 'timeout' } });
      if (url.includes('/api/team/update?')) return json({ ok: false, error: { code: 'ReadTimeout', message: 'timeout' } });
      if (url.includes('/api/team/overview')) return json({ ok: false, error: { code: 'ReadTimeout', message: 'timeout' } });
      return json({ ok: false, error: { code: 'NoRoute', message: url } });
    });

    renderApp('/updates/U-001?scope=P-001');
    await screen.findByTestId('update-failed');
    expect(screen.queryByTestId('update-empty')).toBeNull();
    expect(screen.getByTestId('freshness-authoritative').textContent).toContain(lang === 'zh-CN' ? '读取失败' : 'Read failed');
    expect(findHumanLanguageViolations(screen.getByTestId('page-update'))).toEqual([]);
  });

  it.each(['zh-CN', 'en-US'] as const)('4. partial failure when list fails (%s): keeps inspect facts, marks right column failed, freshness failed', async (lang) => {
    await act(async () => { await i18n.changeLanguage(lang); });
    const updates = sampleUpdates();
    installMock((url) => {
      if (url.includes('/api/team/projects')) return json(sampleProjects());
      if (url.includes('/api/team/updates?')) return json({ ok: false, error: { code: 'Unavailable', message: 'list down' } });
      if (url.includes('/api/team/update?')) return json(updateInspectReply('P-001', updates[0], sampleSnapshot()));
      if (url.includes('/api/team/overview')) return json(sampleOverview('P-001'));
      return json({ ok: false, error: { code: 'NoRoute', message: url } });
    });

    renderApp('/updates/U-001?scope=P-001');
    await screen.findByTestId('update-partial');
    expect(screen.getByTestId('update-fact-row-verified')).toBeTruthy();
    expect(screen.getByTestId('update-prev-failed')).toBeTruthy();
    expect(screen.getByTestId('update-all-failed')).toBeTruthy();
    expect(screen.queryByTestId('update-prev-first')).toBeNull();
    expect(screen.getByTestId('freshness-authoritative').textContent).toContain(lang === 'zh-CN' ? '读取失败' : 'Read failed');
    expect(findHumanLanguageViolations(screen.getByTestId('page-update'))).toEqual([]);
  });

  it.each(['zh-CN', 'en-US'] as const)('5. partial failure when inspect fails (%s): never fakes zero facts', async (lang) => {
    await act(async () => { await i18n.changeLanguage(lang); });
    const updates = sampleUpdates();
    installMock((url) => {
      if (url.includes('/api/team/projects')) return json(sampleProjects());
      if (url.includes('/api/team/updates?')) return json(updatesListReply('P-001', updates));
      if (url.includes('/api/team/update?')) return json({ ok: false, error: { code: 'Unavailable', message: 'inspect down' } });
      if (url.includes('/api/team/overview')) return json(sampleOverview('P-001'));
      return json({ ok: false, error: { code: 'NoRoute', message: url } });
    });

    renderApp('/updates/U-001?scope=P-001');
    await screen.findByTestId('update-partial');
    expect(screen.getByTestId('update-facts-failed')).toBeTruthy();
    expect(screen.queryByTestId('update-fact-row-verified')).toBeNull();
    expect(screen.getByTestId('freshness-authoritative').textContent).toContain(lang === 'zh-CN' ? '读取失败' : 'Read failed');
    expect(findHumanLanguageViolations(screen.getByTestId('page-update'))).toEqual([]);
  });

  it.each(['zh-CN', 'en-US'] as const)('6. revoked / forbidden state (%s) clears all update content immediately', async (lang) => {
    await act(async () => { await i18n.changeLanguage(lang); });
    installMock((url) => {
      if (url.includes('/api/team/projects')) return json(sampleProjects());
      return json({ ok: false, error: { code: 'Forbidden', message: 'revoked' } });
    });

    renderApp('/updates/U-001?scope=P-001');
    await screen.findByTestId('state-forbidden');
    expect(screen.queryByTestId('update-hero')).toBeNull();
    expect(screen.queryByTestId('update-facts')).toBeNull();
    expect(findHumanLanguageViolations(screen.getByTestId('page-update'))).toEqual([]);
  });

  it('clears the page immediately when only update inspect returns Forbidden even after updates list succeeded', async () => {
    const updates = sampleUpdates();
    installMock((url) => {
      if (url.includes('/api/team/projects')) return json(sampleProjects());
      if (url.includes('/api/team/updates?')) return json(updatesListReply('P-001', updates));
      if (url.includes('/api/team/update?')) return json({ ok: false, error: { code: 'Forbidden', message: 'revoked on inspect' } });
      if (url.includes('/api/team/overview')) return json(sampleOverview('P-001'));
      return json({ ok: false, error: { code: 'NoRoute', message: url } });
    });

    renderApp('/updates/U-001?scope=P-001');
    await screen.findByTestId('state-forbidden');
    expect(screen.queryByTestId('update-hero')).toBeNull();
    expect(screen.queryByTestId('update-facts')).toBeNull();
    expect(screen.queryByTestId('update-all')).toBeNull();
  });

  it('never mistakes a failed updates list read on /updates?scope=P-001 for an empty project', async () => {
    installMock((url) => {
      if (url.includes('/api/team/projects')) return json(sampleProjects());
      if (url.includes('/api/team/updates?')) return json({ ok: false, error: { code: 'Unavailable', message: 'list failed' } });
      if (url.includes('/api/team/overview')) return json(sampleOverview('P-001'));
      return json({ ok: false, error: { code: 'NoRoute', message: url } });
    });

    renderApp('/updates?scope=P-001');
    await screen.findByTestId('update-failed');
    expect(screen.queryByTestId('update-empty')).toBeNull();
    expect(screen.getByTestId('freshness-authoritative').textContent).toContain('读取失败');
  });

  it.each(['zh-CN', 'en-US'] as const)('7. unknown outcome (%s): null fact snapshot renders unknown notice, never zeros', async (lang) => {
    await act(async () => { await i18n.changeLanguage(lang); });
    const updateNoSnapshot: PeriodUpdateRecord = { ...sampleUpdates()[0], fact_snapshot_ref: null };
    installMock((url) => {
      if (url.includes('/api/team/projects')) return json(sampleProjects());
      if (url.includes('/api/team/updates?')) return json(updatesListReply('P-001', [updateNoSnapshot]));
      if (url.includes('/api/team/update?')) return json(updateInspectReply('P-001', updateNoSnapshot, null));
      if (url.includes('/api/team/overview')) return json(sampleOverview('P-001'));
      return json({ ok: false, error: { code: 'NoRoute', message: url } });
    });

    renderApp('/updates/U-001?scope=P-001');
    await screen.findByTestId('update-unknown');
    expect(screen.getByTestId('update-facts-unknown')).toBeTruthy();
    expect(screen.queryByTestId('update-fact-row-verified')).toBeNull();
    expect(screen.getByTestId('freshness-badge')).toBeTruthy();
    expect(findHumanLanguageViolations(screen.getByTestId('page-update'))).toEqual([]);
  });

  it('discloses truncated update list instead of claiming first period, and loads earlier page on click', async () => {
    const updates = sampleUpdates();
    installMock((url) => {
      if (url.includes('/api/team/projects')) return json(sampleProjects());
      if (url.includes('/api/team/updates?')) {
        const cursor = new URL(url, 'http://synthetic.invalid').searchParams.get('cursor');
        if (!cursor) return json(updatesListReply('P-001', [updates[0]], { next_cursor: 'cursor-page-2', truncated: true }));
        return json(updatesListReply('P-001', [updates[1], updates[2]], { next_cursor: null, truncated: false }));
      }
      if (url.includes('/api/team/update?')) return json(updateInspectReply('P-001', updates[0], sampleSnapshot()));
      if (url.includes('/api/team/overview')) return json(sampleOverview('P-001'));
      return json({ ok: false, error: { code: 'NoRoute', message: url } });
    });

    renderApp('/updates/U-001?scope=P-001');
    expect(await screen.findByTestId('update-prev-truncated')).toBeTruthy();
    expect(screen.queryByTestId('update-prev-first')).toBeNull();

    fireEvent.click(screen.getByTestId('update-load-more'));
    await waitFor(() => {
      expect(within(screen.getByTestId('update-all')).getAllByRole('button')).toHaveLength(3);
    });
    expect(screen.getByTestId('update-previous').textContent).toContain('查询接口加缓存层后压测达标，按计划推进。');
  });

  it('discloses hidden works when fact_snapshot_coverage is incomplete', async () => {
    const updates = sampleUpdates();
    installMock((url) => {
      if (url.includes('/api/team/projects')) return json(sampleProjects());
      if (url.includes('/api/team/updates?')) return json(updatesListReply('P-001', updates));
      if (url.includes('/api/team/update?')) return json(updateInspectReply('P-001', updates[0], sampleSnapshot(), { hidden_work_count: 3 }));
      if (url.includes('/api/team/overview')) return json(sampleOverview('P-001'));
      return json({ ok: false, error: { code: 'NoRoute', message: url } });
    });

    renderApp('/updates/U-001?scope=P-001');
    expect(await screen.findByTestId('update-partial')).toBeTruthy();
    expect(screen.getByTestId('update-facts-hidden').textContent).toContain('另有 3 件任务不在你的查看范围内');
  });

  it('never guesses the first project when scope is absent from the URL', async () => {
    const fetchMock = installMock((url) => {
      if (url.includes('/api/team/projects')) return json(sampleProjects());
      return json({ ok: false, error: { code: 'NoRoute', message: url } });
    });

    renderApp('/updates/U-001');
    const chooser = await screen.findByTestId('update-choose-project');
    expect(chooser.textContent).toContain('订单协作平台');
    expect(chooser.textContent).toContain('统一通知服务');
    expect(fetchMock.mock.calls.some(([input]) => String(input).includes('/api/team/update'))).toBe(false);
  });

  it('scrubs diagnostic payloads and raw identifiers in summary, author name, and task titles', async () => {
    const dirtyRecord: PeriodUpdateRecord = {
      ...sampleUpdates()[0],
      summary: 'payload={"token":"secret-123"} actor_id=client-01',
      author: { person_id: 'M-001', display_name: 'actor-999' },
    };
    const dirtyOverview = {
      ...sampleOverview('P-001'),
      works: [
        { key: 'T-107', title: 'client[secret]=leak-title', status: 'verified', owner_person: null, agent: null, next_step: null, detail_loaded: true, workstream_id: 'WS-001', contract_hash: 'h1' },
      ],
    };
    installMock((url) => {
      if (url.includes('/api/team/projects')) return json(sampleProjects());
      if (url.includes('/api/team/updates?')) return json(updatesListReply('P-001', [dirtyRecord]));
      if (url.includes('/api/team/update?')) return json(updateInspectReply('P-001', dirtyRecord, sampleSnapshot()));
      if (url.includes('/api/team/overview')) return json(dirtyOverview);
      return json({ ok: false, error: { code: 'NoRoute', message: url } });
    });

    renderApp('/updates/U-001?scope=P-001');
    await screen.findByTestId('update-title');
    const text = defaultVisibleText(screen.getByTestId('page-update'));
    expect(text).not.toMatch(/secret-123|actor-999|leak-title|payload|actor_id|T-107|T-108|M-001|U-001/);
    expect(screen.getByTestId('update-summary').textContent).toContain('更新说明暂时无法以产品语言显示');
    expect(screen.getByTestId('update-author').textContent).toContain('负责人');
    expect(findHumanLanguageViolations(screen.getByTestId('page-update'))).toEqual([]);
  });

  it('renders verbatim PPD-010/011/012 test fixtures without undefined, null, NaN or raw field names', async () => {
    const inspectFixture = JSON.parse(readFileSync(path.resolve(process.cwd(), 'test/fixtures/update-inspect.json'), 'utf8'));
    const listFixture = JSON.parse(readFileSync(path.resolve(process.cwd(), 'test/fixtures/updates-list.json'), 'utf8'));
    const projectId = inspectFixture.data.project_id as string;
    const updateId = inspectFixture.data.update.update_id as string;

    await act(async () => { await i18n.changeLanguage('en-US'); });
    installMock((url) => {
      if (url.includes('/api/team/projects')) {
        return json({ ...DEMO_PROJECTS, projects: [{ key: projectId, title: 'Reader project', role: 'reader' }] });
      }
      if (url.includes('/api/team/updates?')) {
        return json({
          ok: true,
          result: listFixture,
          freshness: {
            report_received: { status: 'not_measured' },
            authoritative: { status: 'measured', at_unix_ms: 1795121858518 },
            page_read: { status: 'measured', at_unix_ms: 1795121858518 },
          },
        });
      }
      if (url.includes('/api/team/update?')) {
        return json({
          ok: true,
          result: inspectFixture,
          freshness: {
            report_received: { status: 'not_measured' },
            authoritative: { status: 'measured', at_unix_ms: 1795121858534 },
            page_read: { status: 'measured', at_unix_ms: 1795121858534 },
          },
        });
      }
      if (url.includes('/api/team/overview')) {
        return json({
          ...DEMO_OVERVIEW,
          project: projectId,
          works: [{ key: 'a', title: 'First verified deliverable', status: 'verified', owner_person: null, agent: null, next_step: null, detail_loaded: true, workstream_id: 'ws-1', contract_hash: 'h' }],
        });
      }
      return json({ ok: false, error: { code: 'NoRoute', message: url } });
    });

    renderApp(`/updates/${encodeURIComponent(updateId)}?scope=${encodeURIComponent(projectId)}`);
    expect((await screen.findByTestId('update-title')).textContent).toContain('Reader project · Week 47 update');
    expect(screen.getByTestId('update-summary').textContent).toContain('The lead wrote this exact sentence.');
    expect(screen.getByTestId('update-superseded-tag').textContent).toContain('Superseded version');
    expect(screen.getByTestId('update-fact-row-verified').textContent).toContain('1 item');
    expect(screen.getByTestId('update-fact-row-verified').textContent).toContain('First verified deliverable');

    const visible = defaultVisibleText(screen.getByTestId('page-update'));
    for (const forbidden of [
      'undefined', 'null', 'NaN', 'fact_snapshot_ref', 'supersedes_update_id',
      'coordinator_epoch', 'source_snapshot_id', 'execution_authorized',
      '01M7TSKHVDC25RY7GZBC6K0ABK', '01M7TSKHVDB96RG4VWKDNK8G5P',
    ]) {
      expect(visible).not.toContain(forbidden);
    }
    expect(findHumanLanguageViolations(screen.getByTestId('page-update'))).toEqual([]);
  });

  it('keeps counts intact and marks partial failure when task-title overview fails, and never leaks raw project keys', async () => {
    const updates = sampleUpdates();
    installMock((url) => {
      if (url.includes('/api/team/projects')) {
        return json({ ...DEMO_PROJECTS, projects: [{ key: 'P-001', role: 'reader' }] });
      }
      if (url.includes('/api/team/updates?')) return json(updatesListReply('P-001', updates));
      if (url.includes('/api/team/update?')) return json(updateInspectReply('P-001', updates[0], sampleSnapshot()));
      if (url.includes('/api/team/overview')) return json({ ok: false, error: { code: 'Unavailable', message: 'overview down' } });
      return json({ ok: false, error: { code: 'NoRoute', message: url } });
    });

    renderApp('/updates/U-001?scope=P-001');
    await screen.findByTestId('update-partial');
    expect(screen.getByTestId('update-title').textContent).toContain('项目 1 · 名称待补充 · 第 47 周的更新');
    expect(screen.getByTestId('update-fact-row-verified').textContent).toContain('2 件');
    expect(screen.getByTestId('update-fact-row-verified').textContent).toContain('某件任务');
    expect(screen.getByTestId('freshness-authoritative').textContent).toContain('读取失败');
    const visible = defaultVisibleText(screen.getByTestId('page-update'));
    expect(visible).not.toMatch(/P-001|T-107|T-108|U-001/);
    expect(findHumanLanguageViolations(screen.getByTestId('page-update'))).toEqual([]);
  });

  it('shows unknown notice when a wait kind is unknown or authoritative ordering is incomparable', async () => {
    const updates = sampleUpdates();
    const unknownWaitSnapshot: PeriodUpdateFactSnapshot = {
      ...sampleSnapshot(),
      waits_opened: {
        count: 1,
        kinds: [{ kind: 'unknown', count: 1, work_ids: ['T-104'] }],
      },
    };
    installMock((url) => {
      if (url.includes('/api/team/projects')) return json(sampleProjects());
      if (url.includes('/api/team/updates?')) return json(updatesListReply('P-001', updates));
      if (url.includes('/api/team/update?')) return json(updateInspectReply('P-001', updates[0], unknownWaitSnapshot));
      if (url.includes('/api/team/overview')) return json(sampleOverview('P-001'));
      return json({ ok: false, error: { code: 'NoRoute', message: url } });
    });

    renderApp('/updates/U-001?scope=P-001');
    expect(await screen.findByTestId('update-unknown')).toBeTruthy();
    expect(screen.getByTestId('update-fact-kind-waits_opened-unknown').textContent).toContain('1 个原因未知的等待');
    fireEvent.click(screen.getByTestId('update-fact-kind-waits_opened-unknown'));
    const detail = screen.getByTestId('update-fact-detail');
    expect(Number(detail.getAttribute('data-count'))).toBe(1);
    expect(detail.querySelectorAll('.obs-row-item')).toHaveLength(1);
    expect(findHumanLanguageViolations(screen.getByTestId('page-update'))).toEqual([]);
  });
});
