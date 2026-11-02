import { act, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { axe, toHaveNoViolations } from 'jest-axe';
import { describe, expect, it, vi } from 'vitest';
import i18n from './i18n';
import { defaultVisibleText, findForbiddenVisibleText } from './lib/human-language-guard';
import {
  DEMO_OVERVIEW,
  DEMO_PROJECTS,
  demoWorkSnapshot,
  renderApp,
} from './test-utils';

expect.extend(toHaveNoViolations);

const FULL_SESSION_ID = 'ws_secret_full_session_id';

const OVERVIEW = {
  ...DEMO_OVERVIEW,
  identity: {
    actor_id: 'lin-agent-x',
    client_id: 'lin-grok-x',
    role: 'developer',
    business_roles: ['observer'],
    can_manage_members: false,
    can_read_project_audit: false,
    membership_action_ceiling: ['work.read'],
  },
  mcp_url: 'http://127.0.0.1:9000/v1/projects/demo/mcp',
  interaction_mode: 'mcp',
  works: [{ key: 'W1', title: 'Task one', workstream_id: 'WS1', status: 'unknown', owner_person: null, agent: null, next_step: null, detail_loaded: false, contract_hash: 'c1' }],
};

const PROJECTS = {
  ...DEMO_PROJECTS,
  session: {
    session_id: FULL_SESSION_ID,
    created_at_ms: 1761830000000,
    expires_at_ms: 1761860000000,
    last_seen_ms: 1761835000000,
    ok: true,
    projects: ['demo'],
  },
};

function setupAccount() {
  const calls: Array<{ url: string; init?: RequestInit }> = [];
  const fetchMock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
    const url = String(input);
    calls.push({ url, init });
    if (url.includes('/api/team/projects')) return jsonResponse(PROJECTS);
    if (url.includes('/api/team/overview')) return jsonResponse(OVERVIEW);
    if (url.includes('/api/team/session/revoke') || url.includes('/api/team/logout')) return jsonResponse({ ok: true });
    if (url.includes('/api/team/read')) {
      const request = JSON.parse(String(init?.body)) as { query: { op: string; work_id?: string } };
      if (request.query.op === 'work.snapshot') return jsonResponse(demoWorkSnapshot(request.query.work_id ?? ''));
    }
    return jsonResponse({ ok: false, error: { code: 'NoRoute', message: url } });
  });
  vi.stubGlobal('fetch', fetchMock);
  return { calls, fetchMock };
}

function jsonResponse(payload: unknown) {
  return new Response(JSON.stringify(payload), {
    status: 200,
    headers: { 'content-type': 'application/json' },
  });
}

describe('agent-connect page (AWR-OBS-008)', () => {
  it('says in words what the connection may do, from the overview identity only, and keeps the ids behind the technical details', async () => {
    const { calls } = setupAccount();
    renderApp('/agent-connect?scope=demo');
    const card = await screen.findByTestId('connect-capabilities');
    await waitFor(() => expect(screen.getByTestId('connect-allowed').textContent).toBe('查看授权的任务'));
    expect(screen.getByText('开发者')).toBeTruthy();
    expect(screen.getByText('观察者')).toBeTruthy();
    expect(screen.getByTestId('connect-not-included').textContent).toBe('批准计划、做出评审结论、管理项目成员与权限、查看项目活动与审计记录');
    expect(findForbiddenVisibleText(defaultVisibleText(card))).toEqual([]);
    for (const hidden of ['lin-agent-x', 'lin-grok-x', 'work.read']) expect(defaultVisibleText(card)).not.toContain(hidden);
    await userEvent.setup().click(within(card).getByText('技术细节'));
    for (const exact of ['lin-agent-x', 'lin-grok-x', 'work.read']) expect(card.textContent).toContain(exact);
    // Identity comes from the overview only: no work.next read for it.
    expect(calls.filter((call) => String(call.init?.body ?? '').includes('work.next')).length).toBe(0);
  });

  it('shows no self-reported agent name in the identity card', async () => {
    setupAccount();
    renderApp('/agent-connect?scope=demo');
    await screen.findByTestId('connect-capabilities');
    // The real snapshot has no actor_name under observation.client
    // ({capabilities, model, product, version}); the viewer's own
    // self-reported name has no read source and must not appear.
    expect(document.querySelector('[data-testid="connect-capabilities"]')?.textContent).not.toContain('Lin Agent');
  });

  it('the account page does not poll a work.snapshot for a self-reported name', async () => {
    const { calls } = setupAccount();
    renderApp('/account?scope=demo');
    await screen.findByTestId('account-session-card');
    // There is no viewer-self-reported-name read source, so no such poll.
    expect(calls.filter((call) => String(call.init?.body ?? '').includes('work.snapshot')).length).toBe(0);
  });

  it('shows the MCP connect guide with a credential row that displays no credential', async () => {
    setupAccount();
    renderApp('/agent-connect?scope=demo');
    expect(await screen.findByText('http://127.0.0.1:9000/v1/projects/demo/mcp')).toBeTruthy();
    expect(screen.getByTestId('credential-row').textContent).toContain('不展示凭据');
    expect(document.body.textContent).not.toContain(FULL_SESSION_ID);
  });

  it('the copy button copies exactly the MCP address', async () => {
    setupAccount();
    const user = userEvent.setup();
    const written: string[] = [];
    Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText: async (text: string) => { written.push(text); } } });
    renderApp('/agent-connect?scope=demo');
    await user.click(await screen.findByRole('button', { name: '复制完整连接说明' }));
    await waitFor(() => expect(written.length).toBeGreaterThan(0));
    expect(written[0]).toBe('MCP: http://127.0.0.1:9000/v1/projects/demo/mcp');
    expect(written[0]).not.toContain(FULL_SESSION_ID);
  });

  it('declares independent-connection revocation unavailable without a fake button', async () => {
    setupAccount();
    renderApp('/agent-connect?scope=demo');
    await screen.findByTestId('page-agent-connect');
    expect((await screen.findByTestId('connect-revocation-note')).textContent).toBe('暂时不能单独让某个 Agent 的连接失效；需要时请联系管理员。');
    expect(screen.queryByRole('button', { name: /撤销独立连接|撤销代理连接|失效/ })).toBeNull();
  });

  it('passes axe on the agent-connect page', async () => {
    setupAccount();
    const { container } = renderApp('/agent-connect?scope=demo');
    await screen.findByTestId('connect-capabilities');
    expect(await axe(container)).toHaveNoViolations();
  });
});

describe('account page (AWR-OBS-008)', () => {
  it('shows session created and expires as distinct rows, never the full session id', async () => {
    setupAccount();
    renderApp('/account?scope=demo');
    await screen.findByTestId('account-session-card');
    expect(document.body.textContent).not.toContain(FULL_SESSION_ID);
    expect(document.body.textContent).not.toContain('secret-token');
    // The created and expires rows are distinct values, not one another.
    const card = document.querySelector('[data-testid="account-session-card"]')!;
    await waitFor(() => expect(card.textContent).not.toContain('未测量'));
    // Both rows are real, distinct times, not one another.
    expect(card.textContent).toContain('2025/10/30');
    const times = card.textContent.match(/\d{4}\/\d{2}\/\d{2} \d{2}:\d{2}:\d{2}/g) ?? [];
    expect(times.length).toBeGreaterThanOrEqual(3);
    expect(new Set(times).size).toBe(times.length);
    expect(card.textContent).toContain('登录时间');
    expect(card.textContent).toContain('到期时间');
    expect(card.textContent).toContain('最近使用');
  });

  it('offers no "all connections" wording or duplicate revoke-connections button', async () => {
    setupAccount();
    renderApp('/account?scope=demo');
    await screen.findByTestId('account-session-card');
    expect(document.body.textContent).not.toContain('全部连接');
    expect(document.body.textContent).not.toContain('all connections');
    expect(screen.queryByRole('button', { name: '撤销全部连接' })).toBeNull();
    expect(screen.queryByRole('button', { name: '撤销连接' })).toBeNull();
  });

  it('a successful all_mine revocation puts the page into the revoked state and silences reads', async () => {
    const { calls } = setupAccount();
    const user = userEvent.setup();
    renderApp('/account?scope=demo');
    await screen.findByTestId('account-session-card');
    await user.click(screen.getByRole('button', { name: '退出本设备上的所有登录' }));
    await waitFor(() => {
      const call = calls.find((entry) => entry.url === '/api/team/session/revoke' && entry.init?.method === 'POST');
      expect(call).toBeTruthy();
      expect(JSON.parse(String(call?.init?.body))).toEqual({ all_mine: true });
    });
    // After the success, the page is revoked: forbidden state, no more reads.
    expect(await screen.findByTestId('state-forbidden')).toBeTruthy();
    const settled = calls.length;
    await new Promise((resolve) => setTimeout(resolve, 1200));
    expect(calls.length).toBe(settled);
  });

  it('states the revocation boundary without claiming restart or multi-instance revocation', async () => {
    setupAccount();
    const user = userEvent.setup();
    renderApp('/account?scope=demo');
    await user.click(await screen.findByText('技术细节'));
    const boundary = await screen.findByTestId('technical-detail-Revocation boundary');
    expect(boundary.textContent).toContain('进程');
    expect(boundary.textContent).toContain('重启即失');
    expect(boundary.textContent).toContain('多实例不共享');
    expect(boundary.textContent).toContain('不支持重启后持久撤销');
  });

  it('the resume entry is read-only (no claim command fires)', async () => {
    const { calls } = setupAccount();
    const user = userEvent.setup();
    renderApp('/account?scope=demo');
    const resumeLink = await screen.findByRole('link', { name: '继续查看我的工作' });
    expect(resumeLink.getAttribute('href')).toContain('/workbench');
    await user.click(resumeLink);
    expect(calls.filter((call) => String(call.init?.body ?? '').includes('"claim')).length).toBe(0);
  });

  it('a revoked session shows the forbidden state on the account page', async () => {
    const calls: Array<{ url: string; init?: RequestInit }> = [];
    const fetchMock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = String(input);
      calls.push({ url, init });
      return new Response(JSON.stringify({ ok: false, error: { code: 'Unauthenticated', message: 'revoked' } }), {
        status: 200,
        headers: { 'content-type': 'application/json' },
      });
    });
    vi.stubGlobal('fetch', fetchMock);
    renderApp('/account?scope=demo');
    expect(await screen.findByTestId('state-forbidden')).toBeTruthy();
    const settled = calls.length;
    await new Promise((resolve) => setTimeout(resolve, 1200));
    expect(calls.length).toBe(settled);
  });
});

describe('agent-connect and account review round 2', () => {
  it('lists what the member may and may not do from the two flags as well as from the ceiling', async () => {
    const custom = {
      ...OVERVIEW,
      identity: { ...OVERVIEW.identity, can_manage_members: true, can_read_project_audit: false },
    };
    const fetchMock = vi.fn(async (input: RequestInfo | URL) => {
      const url = String(input);
      if (url.includes('/api/team/projects')) return jsonResponse(PROJECTS);
      if (url.includes('/api/team/overview')) return jsonResponse(custom);
      return jsonResponse({ ok: false, error: { code: 'NoRoute' } });
    });
    vi.stubGlobal('fetch', fetchMock);
    renderApp('/agent-connect?scope=demo');
    await screen.findByTestId('connect-capabilities');
    await waitFor(() => expect(screen.getByTestId('connect-allowed').textContent).toBe('查看授权的任务、管理项目成员与权限'));
    expect(screen.getByTestId('connect-not-included').textContent).toBe('批准计划、做出评审结论、查看项目活动与审计记录');
  });

  it.each([
    ['an operation nobody has words for is counted, never shown', { membership_action_ceiling: ['work.read', 'future.thing'] }, '查看授权的任务、另有 1 项其他操作'],
    ['an empty ceiling says nothing is allowed', { membership_action_ceiling: [] }, '没有授权任何操作'],
    ['the read-audit flag adds its operation', { can_read_project_audit: true }, '查看授权的任务、查看项目活动与审计记录'],
    ['a missing ceiling and flags say not provided', { membership_action_ceiling: undefined, can_manage_members: undefined, can_read_project_audit: undefined }, '未提供'],
  ])('%s', async (_name, identity, expected) => {
    const custom = { ...OVERVIEW, identity: { ...OVERVIEW.identity, ...identity } };
    vi.stubGlobal('fetch', vi.fn(async (input: RequestInfo | URL) => {
      const url = String(input);
      if (url.includes('/api/team/projects')) return jsonResponse(PROJECTS);
      if (url.includes('/api/team/overview')) return jsonResponse(custom);
      return jsonResponse({ ok: false, error: { code: 'NoRoute' } });
    }));
    renderApp('/agent-connect?scope=demo');
    await waitFor(() => expect(screen.getByTestId('connect-allowed').textContent).toBe(expected));
    expect(findForbiddenVisibleText(defaultVisibleText(screen.getByTestId('connect-capabilities')))).toEqual([]);
    if (expected === '未提供') expect(screen.queryByTestId('connect-not-included')).toBeNull();
  });

  it('reads in English with nothing for the guard to flag', async () => {
    setupAccount();
    renderApp('/agent-connect?scope=demo');
    await screen.findByTestId('connect-capabilities');
    await act(() => i18n.changeLanguage('en-US'));
    await waitFor(() => expect(screen.getByTestId('connect-allowed').textContent).toBe('View authorized work'));
    expect(screen.getByTestId('connect-not-included').textContent).toBe('Approve plans, Decide reviews, Manage project members and access, Read project activity and audit');
    expect(screen.getByTestId('connect-revocation-note').textContent).toContain('cannot be revoked on its own yet');
    expect(findForbiddenVisibleText(defaultVisibleText(screen.getByTestId('page-agent-connect')))).toEqual([]);
    await act(() => i18n.changeLanguage('zh-CN'));
  });

  it('the first copy button copies exactly the MCP address too', async () => {
    setupAccount();
    const user = userEvent.setup();
    const written: string[] = [];
    Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText: async (text: string) => { written.push(text); } } });
    renderApp('/agent-connect?scope=demo');
    await user.click(await screen.findByRole('button', { name: /^复\s*制$/ }));
    await waitFor(() => expect(written.length).toBe(1));
    expect(written[0]).toBe('http://127.0.0.1:9000/v1/projects/demo/mcp');
  });

  it('a revoked session shows the forbidden state on /agent-connect and silences reads', async () => {
    const calls: Array<{ url: string; init?: RequestInit }> = [];
    const fetchMock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = String(input);
      calls.push({ url, init });
      return jsonResponse({ ok: false, error: { code: 'Unauthenticated', message: 'revoked' } });
    });
    vi.stubGlobal('fetch', fetchMock);
    renderApp('/agent-connect?scope=demo');
    expect(await screen.findByTestId('state-forbidden')).toBeTruthy();
    const settled = calls.length;
    await new Promise((resolve) => setTimeout(resolve, 1200));
    expect(calls.length).toBe(settled);
  });

  it('no part of session.session_id appears in the account page DOM', async () => {
    setupAccount();
    renderApp('/account?scope=demo');
    // Wait for the session data to actually load before asserting: an
    // unloaded page would pass vacuously (the reviewer's fault survived).
    await screen.findAllByText(/2025/);
    expect(document.body.textContent).not.toContain(FULL_SESSION_ID);
    // Only distinctive fragments, not words that are legal in English UI.
    for (const part of ['secret_full', 'full_session', 'session_id', 'ws_secret']) {
      expect(document.body.textContent).not.toContain(part);
    }
  });

  it('the account page also declares independent revocation unavailable, with no single-connection revoke button', async () => {
    setupAccount();
    renderApp('/account?scope=demo');
    await screen.findByTestId('independent-revoke-unavailable');
    expect(screen.queryByRole('button', { name: /撤销独立连接|撤销代理连接|撤销单个连接/ })).toBeNull();
  });

  it('keeps account defaults human while preserving technical boundaries on demand', async () => {
    setupAccount();
    renderApp('/account?scope=demo');
    await screen.findAllByText(/2025/);
    expect(findForbiddenVisibleText(defaultVisibleText(document.body))).toEqual([]);

    const user = userEvent.setup();
    await user.click(screen.getByText('技术细节'));
    expect(document.body.textContent).toContain('OAuth');
    expect(document.body.textContent).toContain('client_id');
    expect(findForbiddenVisibleText(defaultVisibleText(document.body))).toEqual([]);
  });
});
