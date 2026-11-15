import { act, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import i18n from './i18n';
import { defaultVisibleText, findForbiddenVisibleText } from './lib/human-language-guard';
import { DEMO_OVERVIEW, DEMO_PROJECTS, mockViewport, renderApp } from './test-utils';

const REFERENCE = `agent_${'a'.repeat(64)}`;
const rows = [
  { connection_id: REFERENCE, kind: 'agent', client_name: 'Grok Build', state: 'active',
    created_at_ms: 1794700000000, expires_at_ms: 1794786400000, can_revoke: true, revocation_scope: 'connection_family' },
  { connection_id: `browser_${'b'.repeat(64)}`, kind: 'browser', is_current: true, state: 'active',
    created_at_ms: 1794700000000, expires_at_ms: 1794786400000, can_revoke: true, revocation_scope: 'connection' },
  { connection_id: `access_${'c'.repeat(64)}`, kind: 'shared_access', state: 'active',
    expires_at_ms: null, can_revoke: true, revocation_scope: 'shared_account_access' },
];

function setup(options: { unconfirmed?: boolean; currentEnds?: boolean; denied?: boolean; paged?: boolean } = {}) {
  let items = structuredClone(rows);
  const calls: Array<{ url: string; body: Record<string, unknown> }> = [];
  vi.stubGlobal('fetch', vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
    const url = String(input);
    const body = init?.body ? JSON.parse(String(init.body)) : {};
    calls.push({ url, body });
    let response: unknown = { ok: false, error: { code: 'NoRoute' } };
    if (url.includes('/api/team/projects')) response = DEMO_PROJECTS;
    if (url.includes('/api/team/overview')) response = DEMO_OVERVIEW;
    if (url === '/api/team/connections/list') response = options.denied
      ? { ok: false, error: { code: 'Forbidden', message: 'private_protocol_error sha256:secret' } }
      : { ok: true, data: { items, next_cursor: options.paged && !body.cursor ? REFERENCE : null } };
    if (url === '/api/team/connections/revoke') {
      if (options.unconfirmed) throw new Error('private_transport_failure /private/internal/path');
      items = items.map((item) => item.connection_id === body.connection_id ? { ...item, state: 'revoked' } : item);
      response = { ok: true, data: { connection_id: body.connection_id, state: 'revoked', ends_current: options.currentEnds === true } };
    }
    return new Response(JSON.stringify(response), { status: 200, headers: { 'content-type': 'application/json' } });
  }));
  return calls;
}

describe('connection management', () => {
  it('keeps the connection actions directly available in a narrow viewport', async () => {
    setup();
    const viewport = mockViewport(390);
    try {
      renderApp('/account?scope=demo');
      await screen.findByText('Grok Build');
      const card = screen.getByTestId('account-connections');
      expect(card.querySelector('.ant-table-container')).toBeNull();
      expect(within(card).getAllByRole('button', { name: '断开访问' })).toHaveLength(3);
      expect(defaultVisibleText(card)).not.toContain(REFERENCE);
    } finally {
      viewport.restore();
    }
  });

  it('starts at the first connection page when the selected project changes', async () => {
    const calls = setup({ paged: true });
    const user = userEvent.setup();
    const { router } = renderApp('/account?scope=demo');
    await screen.findByText('Grok Build');
    await user.click(await screen.findByRole('button', { name: '下一页' }));
    await waitFor(() => expect(calls.some((call) => call.body.cursor === REFERENCE)).toBe(true));
    await act(() => router.navigate('/account?scope=docs'));
    await waitFor(() => expect(calls.some((call) => call.url === '/api/team/connections/list' && call.body.project === 'docs')).toBe(true));
    const next = calls.find((call) => call.url === '/api/team/connections/list' && call.body.project === 'docs');
    expect(next?.body.cursor).toBeUndefined();
  });

  it('clears the previous connection list while a new sign-in is being verified', async () => {
    const releases: Array<() => void> = [];
    let changed = false;
    vi.stubGlobal('fetch', vi.fn(async (input: RequestInfo | URL) => {
      const url = String(input);
      const response = (body: unknown) => new Response(JSON.stringify(body), { status: 200, headers: { 'content-type': 'application/json' } });
      if (url.includes('/api/team/projects')) return response(DEMO_PROJECTS);
      if (url.includes('/api/team/overview')) return response(DEMO_OVERVIEW);
      if (url === '/api/team/login') { changed = true; return response({ ok: true }); }
      if (url === '/api/team/connections/list') {
        if (changed) return new Promise<Response>((resolve) => releases.push(() => resolve(response({ ok: true, data: { items: [], next_cursor: null } }))));
        return response({ ok: true, data: { items: rows, next_cursor: null } });
      }
      return response({ ok: false, error: { code: 'NoRoute' } });
    }));
    const user = userEvent.setup();
    renderApp('/account?scope=demo');
    await screen.findByText('Grok Build');
    await user.type(screen.getByLabelText('访问口令'), 'synthetic-test-passphrase');
    await user.click(screen.getByRole('button', { name: '登录' }));
    try {
      await waitFor(() => expect(releases.length).toBeGreaterThan(0));
      expect(screen.queryByText('Grok Build')).toBeNull();
    } finally {
      await act(() => releases.forEach((release) => release()));
    }
  });
  it.each(['zh-CN', 'en-US'])('shows product labels and hides every connection reference in %s', async (language) => {
    setup();
    await act(() => i18n.changeLanguage(language));
    renderApp('/account?scope=demo');
    await screen.findByText('Grok Build');
    const card = screen.getByTestId('account-connections');
    const visible = defaultVisibleText(card);
    expect(findForbiddenVisibleText(visible)).toEqual([]);
    for (const item of rows) expect(visible).not.toContain(item.connection_id);
    expect(visible).not.toContain('connection_family');
    expect(card.querySelector('details')).toBeNull();
    await act(() => i18n.changeLanguage('zh-CN'));
  });

  it('confirms and revokes exactly the selected connection before showing success', async () => {
    const calls = setup();
    const user = userEvent.setup();
    renderApp('/account?scope=demo');
    const row = (await screen.findByText('Grok Build')).closest('tr')!;
    await user.click(within(row).getByRole('button', { name: '断开访问' }));
    expect(calls.filter((call) => call.url.includes('/connections/revoke'))).toHaveLength(0);
    const dialog = await screen.findByRole('tooltip');
    await user.click(within(dialog).getByRole('button', { name: '断开访问' }));
    await waitFor(() => expect(calls.find((call) => call.url === '/api/team/connections/revoke')?.body).toEqual({ project: 'demo', connection_id: REFERENCE }));
    await waitFor(() => expect(within(row).getByText('已断开')).toBeTruthy());
    expect(screen.getByText('访问已断开')).toBeTruthy();
    expect(calls.some((call) => call.url.includes('/command'))).toBe(false);
  });

  it('explains the full impact of a shared account passphrase before revoking it', async () => {
    setup();
    const user = userEvent.setup();
    renderApp('/account?scope=demo');
    const row = (await screen.findByText('共用访问口令')).closest('tr')!;
    expect(within(row).getByText('影响使用此口令访问各项目的连接')).toBeTruthy();
    await user.click(within(row).getByRole('button', { name: '断开访问' }));
    expect(await screen.findByText('所有使用此口令的连接都将失效。请先确认影响范围。')).toBeTruthy();
  });

  it('keeps an uncertain outcome unconfirmed and hides raw transport diagnostics', async () => {
    setup({ unconfirmed: true });
    const user = userEvent.setup();
    renderApp('/account?scope=demo');
    const row = (await screen.findByText('Grok Build')).closest('tr')!;
    await user.click(within(row).getByRole('button', { name: '断开访问' }));
    await user.click(within(await screen.findByRole('tooltip')).getByRole('button', { name: '断开访问' }));
    expect(await screen.findByText('尚未确认是否断开，请刷新连接查看')).toBeTruthy();
    expect(screen.queryByText('访问已断开')).toBeNull();
    expect(document.body.textContent).not.toContain('private_transport_failure');
    expect(document.body.textContent).not.toContain('/private/internal/path');
    await waitFor(() => expect(within(row).getByText('可访问')).toBeTruthy());
  });

  it('clears the observation surface when the current connection ends', async () => {
    setup({ currentEnds: true });
    const user = userEvent.setup();
    renderApp('/account?scope=demo');
    const row = (await screen.findByText('当前浏览器')).closest('tr')!;
    await user.click(within(row).getByRole('button', { name: '断开访问' }));
    await user.click(within(await screen.findByRole('tooltip')).getByRole('button', { name: '断开访问' }));
    expect(await screen.findByTestId('state-forbidden')).toBeTruthy();
    expect(screen.queryByText('Grok Build')).toBeNull();
  });

  it('withholds the list and internal permission errors after access is revoked', async () => {
    setup({ denied: true });
    renderApp('/account?scope=demo');
    expect(await screen.findByTestId('state-forbidden')).toBeTruthy();
    expect(screen.queryByText('Grok Build')).toBeNull();
    expect(document.body.textContent).not.toContain('private_protocol_error');
  });
});
