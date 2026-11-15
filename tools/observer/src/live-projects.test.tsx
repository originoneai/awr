import { act, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it } from 'vitest';
import i18n from './i18n';
import { DEMO_OVERVIEW, renderApp, stubFetch } from './test-utils';

afterEach(async () => { await act(async () => { await i18n.changeLanguage('zh-CN'); }); });

describe('live project discovery without display metadata', () => {
  it.each(['zh-CN', 'en-US'])('distinguishes untitled projects consistently in the list, menu and selected headers in %s', async (language) => {
    await i18n.changeLanguage(language);
    const user = userEvent.setup();
    stubFetch({ '/api/team/projects': { ok: true, projects: [{ key: 'first-private' }, { key: 'second-private', title: 'client_obs_658' }] },
      '/api/team/overview': DEMO_OVERVIEW });
    const { router } = renderApp('/projects');
    const first = i18n.t('shell.unnamedProject', { number: 1 });
    const second = i18n.t('shell.unnamedProject', { number: 2 });
    expect(await screen.findByRole('link', { name: first })).toBeTruthy();
    const chosen = await screen.findByRole('link', { name: second });
    expect(chosen.getAttribute('href')).toBe('/projects/second-private');
    await user.click(chosen);
    await waitFor(() => expect(router.state.location.pathname).toBe('/projects/second-private'));
    expect(screen.getByTestId('scope-switcher').textContent).toBe(second);
    expect(screen.getByTestId('project-title').textContent).toBe(second);
    await user.click(screen.getByTestId('scope-switcher'));
    expect(await screen.findByRole('menuitem', { name: first })).toBeTruthy();
    expect(await screen.findByRole('menuitem', { name: second })).toBeTruthy();
    expect(document.body.textContent).not.toMatch(/first-private|second-private|client_obs_658/);
  });
  it.each([undefined, null, '   '])('opens a project whose title is %s', async (title) => {
    const user = userEvent.setup();
    stubFetch({
      '/api/team/projects': {
        ok: true,
        projects: [{ key: 'support-workspace', title, project_id: 'support-project' }],
      },
      '/api/team/overview': { ...DEMO_OVERVIEW, project: 'support-workspace' },
    });
    const { router } = renderApp('/projects');

    const projectLink = await screen.findByRole('link', { name: '项目 1 · 名称待补充' });
    expect(projectLink.getAttribute('href')).toBe('/projects/support-workspace');
    await user.click(projectLink);

    await waitFor(() => expect(router.state.location.pathname).toBe('/projects/support-workspace'));
    expect(screen.getByTestId('scope-switcher').textContent).toBe('项目 1 · 名称待补充');
    expect(document.body.textContent).not.toContain('undefined');
  });

  it('offers a key-only project in the scope menu', async () => {
    const user = userEvent.setup();
    stubFetch({
      '/api/team/projects': { ok: true, projects: [{ key: 'support-workspace' }] },
      '/api/team/overview': { ...DEMO_OVERVIEW, project: 'support-workspace' },
    });
    const { router } = renderApp('/workbench');
    await user.click(screen.getByTestId('scope-switcher'));
    await user.click(await screen.findByText('项目 1 · 名称待补充'));

    await waitFor(() => expect(router.state.location.search).toBe('?scope=support-workspace'));
    expect(screen.getByTestId('scope-switcher').textContent).toBe('项目 1 · 名称待补充');
  });
});
