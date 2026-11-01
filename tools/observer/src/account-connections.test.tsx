import { screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { axe, toHaveNoViolations } from 'jest-axe';
import { describe, expect, it, vi } from 'vitest';
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
  it('shows the identity and effective permissions from the overview identity, with no extra identity read', async () => {
    const { calls } = setupAccount();
    renderApp('/agent-connect?scope=demo');
    expect(await screen.findByText('lin-agent-x')).toBeTruthy();
    expect(screen.getByText('lin-grok-x')).toBeTruthy();
    expect(screen.getByText('开发者')).toBeTruthy();
    // Effective permissions use localized display; exact service values remain in tooltips.
    expect(screen.getByText('观察者')).toBeTruthy();
    expect(screen.getByText('work.read')).toBeTruthy();
    expect(screen.getAllByText('否').length).toBeGreaterThanOrEqual(2);
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
    expect(document.body.textContent).toContain('当前不可用');
    expect(screen.queryByRole('button', { name: /撤销独立连接|撤销代理连接/ })).toBeNull();
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
    expect(card.textContent).toContain('创建时间');
    expect(card.textContent).toContain('过期时间');
    expect(card.textContent).toContain('最近活动');
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
    await user.click(screen.getByRole('button', { name: '撤销本凭据的全部 Web 会话' }));
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
    renderApp('/account?scope=demo');
    const boundary = await screen.findByTestId('revocation-boundary');
    expect(boundary.textContent).toContain('进程');
    expect(boundary.textContent).toContain('重启即失');
    expect(boundary.textContent).toContain('多实例不共享');
    expect(boundary.textContent).toContain('不支持重启后持久撤销');
  });

  it('the resume entry is read-only (no claim command fires)', async () => {
    const { calls } = setupAccount();
    const user = userEvent.setup();
    renderApp('/account?scope=demo');
    const resumeLink = await screen.findByRole('link', { name: /恢复我的工作|只读恢复/ });
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
  it('shows manage-members and read-audit as distinct rows with their own values', async () => {
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
    const card = await screen.findByTestId('connect-capabilities');
    const rows = card.querySelectorAll('.ant-descriptions-item');
    const textOf = (label: string) => Array.from(rows).find((row) => row.textContent?.includes(label))?.textContent;
    await waitFor(() => expect(textOf('可管理成员')).toContain('是'));
    expect(textOf('可管理成员')).not.toContain('否');
    expect(textOf('可读项目审计')).toContain('否');
    expect(textOf('可读项目审计')).not.toContain('是');
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
});
