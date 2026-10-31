import { act, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import { DEMO_OVERVIEW, DEMO_PROJECTS, renderApp } from './test-utils';

function accountFetch() {
  const calls: Array<{ url: string; init?: RequestInit }> = [];
  const fetchMock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
    const url = String(input);
    calls.push({ url, init });
    const payload = url.includes('/api/team/projects')
      ? DEMO_PROJECTS
      : url.includes('/api/team/overview')
        ? DEMO_OVERVIEW
        : { ok: true };
    return new Response(JSON.stringify(payload), {
      status: 200,
      headers: { 'content-type': 'application/json' },
    });
  });
  vi.stubGlobal('fetch', fetchMock);
  return { calls, fetchMock };
}

function mutationCall(calls: Array<{ url: string; init?: RequestInit }>, path: string) {
  return calls.find((call) => call.url === path && call.init?.method === 'POST');
}

describe('Account mutations and connection entry', () => {
  it('signs in with the guarded login body and resumes read queries', async () => {
    const { calls } = accountFetch();
    const user = userEvent.setup();
    renderApp('/account?scope=demo');
    await screen.findByText(/Team mainline/);
    const projectsBefore = calls.filter((call) => call.url === '/api/team/projects').length;

    await user.type(screen.getByLabelText('访问令牌'), 'secret-token');
    await user.click(screen.getByRole('button', { name: /^登\s*录$/ }));

    await waitFor(() => {
      const call = mutationCall(calls, '/api/team/login');
      expect(call).toBeDefined();
      expect(call?.init?.credentials).toBe('same-origin');
      expect((call?.init?.headers as Record<string, string>)['x-awr-observer']).toBe('1');
      expect(call?.init?.body).toBe(JSON.stringify({ bearer: 'secret-token' }));
    });
    await waitFor(() => {
      expect(calls.filter((call) => call.url === '/api/team/projects').length).toBeGreaterThan(projectsBefore);
    });
  });

  it.each([
    { button: '退出登录', path: '/api/team/logout' },
    { button: '撤销全部连接', path: '/api/team/session/revoke' },
  ])('clears and stops restricted reads after $button succeeds', async ({ button, path }) => {
    const { calls } = accountFetch();
    const user = userEvent.setup();
    const { queryClient, router } = renderApp('/account?scope=demo');
    await screen.findByText(/Team mainline/);

    await user.click(screen.getByRole('button', { name: button }));

    await waitFor(() => {
      const call = mutationCall(calls, path);
      expect(call).toBeDefined();
      expect(call?.init?.credentials).toBe('same-origin');
      expect((call?.init?.headers as Record<string, string>)['x-awr-observer']).toBe('1');
      expect(call?.init?.body).toBe('{}');
      expect(screen.queryByText(/Team mainline/)).toBeNull();
      expect(queryClient.getQueryCache().getAll().every((query) => query.state.data === undefined)).toBe(true);
    });
    await act(() => router.navigate('/projects'));
    expect(await screen.findByTestId('state-forbidden')).toBeTruthy();
  });

  it('resumes forbidden reads after a successful login', async () => {
    const { calls } = accountFetch();
    const user = userEvent.setup();
    const { router } = renderApp('/account?scope=demo');
    await screen.findByText(/Team mainline/);
    await user.click(screen.getByRole('button', { name: '退出登录' }));
    await waitFor(() => expect(screen.queryByText(/Team mainline/)).toBeNull());

    await act(() => router.navigate('/projects'));
    expect(await screen.findByTestId('state-forbidden')).toBeTruthy();
    await act(() => router.navigate('/account?scope=demo'));
    await user.type(screen.getByLabelText('访问令牌'), 'new-token');
    const readsBeforeLogin = calls.filter((call) => call.url === '/api/team/projects').length;
    await user.click(screen.getByRole('button', { name: /^登\s*录$/ }));

    await screen.findByText(/Team mainline/);
    expect(calls.filter((call) => call.url === '/api/team/projects').length).toBeGreaterThan(readsBeforeLogin);
    await act(() => router.navigate('/projects'));
    expect(await screen.findByTestId('page-projects')).toBeTruthy();
    expect(screen.queryByTestId('state-forbidden')).toBeNull();
  });

  it('links to the agent-connect placeholder without calling the project access proxy', async () => {
    const { calls } = accountFetch();
    renderApp('/account');
    const link = await screen.findByRole('link', { name: '连接我的 Agent' });
    expect(link.getAttribute('href')).toBe('/agent-connect');
    expect(calls.some((call) => call.url.includes('/api/team/access'))).toBe(false);
    expect(calls.some((call) => call.init?.body?.toString().includes('demo'))).toBe(false);
  });
});
