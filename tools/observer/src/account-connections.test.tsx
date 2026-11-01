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
    business_roles: ['developer'],
    can_manage_members: false,
    can_read_project_audit: false,
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

function snapshotWithClient(workId: string) {
  const snapshot = demoWorkSnapshot(workId) as {
    result: { data: { observation: Record<string, unknown> } };
  };
  snapshot.result.data.observation.client = {
    actor_id: 'lin-agent-x',
    actor_name: 'Lin Agent',
    client_id: 'lin-grok-x',
    capabilities: { model: 'supported' },
  };
  return snapshot;
}

function setupAccount() {
  const calls: Array<{ url: string; init?: RequestInit }> = [];
  const fetchMock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
    const url = String(input);
    calls.push({ url, init });
    if (url.includes('/api/team/projects')) return jsonResponse(PROJECTS);
    if (url.includes('/api/team/overview')) return jsonResponse(OVERVIEW);
    if (url.includes('/api/team/read')) {
      const request = JSON.parse(String(init?.body)) as { query: { op: string; work_id?: string } };
      if (request.query.op === 'work.snapshot') return jsonResponse(snapshotWithClient(request.query.work_id ?? ''));
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

describe('account and connections (AWR-OBS-008)', () => {
  it('shows the identity attribution from the overview identity, with no extra identity read', async () => {
    const { calls } = setupAccount();
    renderApp('/account?scope=demo');
    expect(await screen.findByText('lin-agent-x')).toBeTruthy();
    expect(screen.getByText('lin-grok-x')).toBeTruthy();
    expect(screen.getByText('developer')).toBeTruthy();
    // Identity comes from the overview only: no work.next read for it.
    expect(calls.filter((call) => String(call.init?.body ?? '').includes('work.next')).length).toBe(0);
  });

  it('labels the agent name as client self-reported', async () => {
    setupAccount();
    renderApp('/account?scope=demo');
    expect(await screen.findByText(/Lin Agent/)).toBeTruthy();
    expect(screen.getByTestId('self-reported-agent-name').textContent).toContain('自报');
  });

  it('shows session times but never the full session id in the DOM', async () => {
    setupAccount();
    renderApp('/account?scope=demo');
    await screen.findByTestId('account-session-card');
    expect(document.body.textContent).not.toContain(FULL_SESSION_ID);
    expect(document.body.textContent).not.toContain('secret-token');
  });

  it('shows the MCP connect guide without any bearer value', async () => {
    setupAccount();
    renderApp('/account?scope=demo');
    expect(await screen.findByText('http://127.0.0.1:9000/v1/projects/demo/mcp')).toBeTruthy();
    expect(document.body.textContent).not.toContain(FULL_SESSION_ID);
  });

  it('declares independent-connection revocation unavailable without a fake button', async () => {
    setupAccount();
    renderApp('/account?scope=demo');
    expect(await screen.findByTestId('independent-revoke-unavailable')).toBeTruthy();
    expect(screen.queryByRole('button', { name: /撤销独立连接|撤销代理连接/ })).toBeNull();
  });

  it('offers revoking all web sessions of this credential with precise wording and body', async () => {
    const { calls } = setupAccount();
    const user = userEvent.setup();
    renderApp('/account?scope=demo');
    const button = await screen.findByRole('button', { name: '撤销本凭据的全部 Web 会话' });
    expect(button.textContent).not.toContain('所有连接');
    expect(button.textContent).not.toContain('所有 Agent');
    await user.click(button);
    await waitFor(() => {
      const call = calls.find((entry) => entry.url === '/api/team/session/revoke' && entry.init?.method === 'POST');
      expect(call).toBeTruthy();
      expect(JSON.parse(String(call?.init?.body))).toEqual({ all_mine: true });
    });
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

  it('passes axe on the account page', async () => {
    setupAccount();
    const { container } = renderApp('/account?scope=demo');
    await screen.findByTestId('account-session-card');
    expect(await axe(container)).toHaveNoViolations();
  });
});

describe('account revocation presentation', () => {
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
