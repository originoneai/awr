import { act, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';
import i18n from './i18n';
import { findHumanLanguageViolations } from './lib/human-language-guard';
import { DEMO_PROJECTS, mockViewport, renderApp } from './test-utils';

const DIRECTORY = {
  configured: true, organization: 'Example company', departments: ['Engineering'], updated_at_ms: 1794600000000,
  members: [
    { name: 'Taylor', available: true, departments: ['Engineering'] },
    { name: 'Taylor', available: true, departments: ['Research'] },
    { name: null, available: false, departments: [] },
  ],
};

function setup() {
  let response: unknown = { ok: true, data: DIRECTORY };
  const reads: string[] = [];
  const fetch = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
    const path = String(input);
    if (path.includes('/api/team/projects')) return Response.json(DEMO_PROJECTS);
    if (path === '/api/team/directory') {
      reads.push(String(init?.body));
      return Response.json(response);
    }
    return Response.json({ ok: false, error: { code: 'NoRoute' } });
  });
  vi.stubGlobal('fetch', fetch);
  return { reads, setResponse: (value: unknown) => { response = value; } };
}

afterEach(async () => {
  await act(async () => { await i18n.changeLanguage('zh-CN'); });
});

describe('project member observation', () => {
  it('starts with permitted project choices and reads only the chosen project', async () => {
    const { reads } = setup();
    renderApp('/members');
    expect(await screen.findByText('Team mainline')).toBeTruthy();
    expect(reads).toEqual([]);
    await userEvent.setup().click(screen.getAllByRole('button', { name: '查看成员' })[0]);
    expect(await screen.findByText('Example company')).toBeTruthy();
    expect(reads).toEqual([JSON.stringify({ project: 'demo' })]);
    expect(screen.queryByTestId('state-not-open')).toBeNull();
  });

  it.each(['zh-CN', 'en-US'])('keeps names, grouping and connection entries readable in %s', async (language) => {
    setup();
    await act(async () => { await i18n.changeLanguage(language); });
    renderApp('/members?scope=demo');
    expect(await screen.findByText('Example company')).toBeTruthy();
    expect(screen.getAllByText('Taylor')).toHaveLength(2);
    const page = screen.getByTestId('page-members');
    expect(findHumanLanguageViolations(page)).toEqual([]);
    expect(within(page).getByRole('link', { name: language === 'zh-CN' ? '查看我的连接' : 'View my connections' }).getAttribute('href')).toBe('/account?scope=demo');
    expect(page.querySelector('details')).toBeNull();
    expect(page.querySelector('[data-testid="technical-details"]')).toBeNull();
    expect(within(page).queryByRole('button', { name: /认领|指派|批准|Assign|Claim|Approve/ })).toBeNull();
  });

  it('filters members without merging people who share a name', async () => {
    setup();
    renderApp('/members?scope=demo');
    await screen.findByText('Example company');
    const search = screen.getByRole('textbox', { name: '搜索成员或部门' });
    await userEvent.setup().type(search, 'Research');
    expect(screen.getAllByText('Taylor')).toHaveLength(1);
    await userEvent.setup().clear(search);
    expect(screen.getAllByText('Taylor')).toHaveLength(2);
  });

  it.each(['zh-CN', 'en-US'].flatMap((language) => [
    [language, { ok: true, data: { ...DIRECTORY, configured: false, organization: null, departments: [], members: [], updated_at_ms: null } }, 'members.notRegistered'],
    [language, { ok: true, data: { ...DIRECTORY, members: [] } }, 'members.empty'],
    [language, { ok: false, error: { code: 'Unsupported' } }, 'members.notEnabled'],
    [language, { ok: false, error: { code: 'BadGateway', message: 'PrivateError request_id=private' } }, 'members.readFailed'],
  ] as const))('distinguishes empty and failed reads with product language in %s', async (language, response, message) => {
    const { setResponse } = setup();
    await act(async () => { await i18n.changeLanguage(language); });
    setResponse(response);
    renderApp('/members?scope=demo');
    expect(await screen.findByText(i18n.t(message))).toBeTruthy();
    expect(document.body.textContent).not.toMatch(/PrivateError|request_id|private/);
    expect(findHumanLanguageViolations(screen.getByTestId('page-members'))).toEqual([]);
  });

  it('clears stale member details after denial and keeps the project selector available', async () => {
    const { setResponse } = setup();
    const { router } = renderApp('/members?scope=demo');
    await screen.findByText('Example company');
    setResponse({ ok: false, error: { code: 'Forbidden' } });
    await userEvent.setup().click(screen.getByRole('button', { name: '刷新成员' }));
    expect(await screen.findByTestId('state-forbidden')).toBeTruthy();
    expect(screen.queryByText('Example company')).toBeNull();
    await act(async () => { await router.navigate('/members'); });
    expect(await screen.findByText('Team mainline')).toBeTruthy();
  });

  it('can refresh again after project directory access has been restored', async () => {
    const { setResponse } = setup();
    setResponse({ ok: false, error: { code: 'Forbidden' } });
    renderApp('/members?scope=demo');
    await screen.findByTestId('state-forbidden');
    setResponse({ ok: true, data: DIRECTORY });
    await userEvent.setup().click(screen.getByRole('button', { name: '刷新成员' }));
    expect(await screen.findByText('Example company')).toBeTruthy();
    expect(screen.queryByTestId('state-forbidden')).toBeNull();
  });

  it('rechecks a remounted project and does not show another project while reading', async () => {
    const { setResponse, reads } = setup();
    const { router } = renderApp('/members?scope=demo');
    await screen.findByText('Example company');
    setResponse({ ok: true, data: { ...DIRECTORY, organization: 'Documentation team' } });
    await act(async () => { await router.navigate('/members?scope=docs'); });
    expect(await screen.findByText('Documentation team')).toBeTruthy();
    expect(screen.queryByText('Example company')).toBeNull();
    expect(reads.map((value) => JSON.parse(value).project)).toEqual(['demo', 'docs']);
    setResponse({ ok: false, error: { code: 'Forbidden' } });
    await act(async () => { await router.navigate('/members?scope=demo'); });
    await waitFor(() => expect(screen.queryByText('Example company')).toBeNull());
    expect(await screen.findByTestId('state-forbidden')).toBeTruthy();
  });

  it('uses a readable member list on a phone', async () => {
    const viewport = mockViewport(390);
    try {
      setup();
      renderApp('/members?scope=demo');
      await screen.findByText('Example company');
      const page = screen.getByTestId('page-members');
      expect(within(page).queryByTestId('member-table')).toBeNull();
      expect(within(page).getByTestId('member-list')).toBeTruthy();
      expect(screen.getAllByText('Taylor')).toHaveLength(2);
      expect(findHumanLanguageViolations(page)).toEqual([]);
    } finally {
      viewport.restore();
    }
  });
});
