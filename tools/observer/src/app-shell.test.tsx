import { screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it } from 'vitest';
import i18n from './i18n';
import { DEMO_OVERVIEW, DEMO_PROJECTS, renderApp, stubFetch } from './test-utils';

afterEach(async () => {
  await i18n.changeLanguage('zh-CN');
});

describe('AppShell navigation', () => {
  it('renders every nav item', () => {
    stubFetch({ '/api/team/projects': DEMO_PROJECTS, '/api/team/overview': DEMO_OVERVIEW });
    renderApp('/workbench');
    for (const label of [
      '工作台',
      '项目',
      '待处理',
      '知识与技能',
      '成员与 Agent',
      '活动与审计',
      '管理设置',
      '账户与连接',
      '高级工具',
    ]) {
      expect(screen.getByRole('menuitem', { name: new RegExp(label) })).toBeTruthy();
    }
  });

  it('renders the placeholder logo and neutral scope wording', () => {
    stubFetch({ '/api/team/projects': DEMO_PROJECTS, '/api/team/overview': DEMO_OVERVIEW });
    renderApp('/workbench');
    expect(screen.getByAltText('AWR')).toBeTruthy();
    expect(screen.getByText('全部授权范围')).toBeTruthy();
    expect(screen.queryByText('示例组织')).toBeNull();
  });

  it('places the theme switch at the bottom of the desktop sidebar', () => {
    stubFetch({ '/api/team/projects': DEMO_PROJECTS, '/api/team/overview': DEMO_OVERVIEW });
    renderApp('/workbench');
    const inner = document.querySelector('.shell-sider-inner');
    expect(inner?.lastElementChild?.classList.contains('shell-theme')).toBe(true);
    expect(inner?.lastElementChild?.querySelector('[data-testid="theme-toggle"]')).not.toBeNull();
  });

  it('navigates through a real menu link', async () => {
    stubFetch({ '/api/team/projects': DEMO_PROJECTS, '/api/team/overview': DEMO_OVERVIEW });
    const user = userEvent.setup();
    const { router } = renderApp('/workbench');
    await user.click(screen.getByRole('link', { name: '项目' }));
    expect(router.state.location.pathname).toBe('/projects');
    await waitFor(() => expect(screen.getByTestId('page-projects')).toBeTruthy());
  });

  it('keeps inert notification and identity entries disabled', () => {
    stubFetch({ '/api/team/projects': DEMO_PROJECTS, '/api/team/overview': DEMO_OVERVIEW });
    renderApp('/workbench');
    for (const name of ['通知', '范围身份']) {
      const button = screen.getByRole('button', { name });
      expect(button.tagName).toBe('BUTTON');
      expect(button).toHaveProperty('disabled', true);
    }
  });

  it('marks the active menu item from the route', () => {
    stubFetch({ '/api/team/projects': DEMO_PROJECTS, '/api/team/overview': DEMO_OVERVIEW });
    renderApp('/inbox');
    const selected = document.querySelector('.ant-menu-item-selected');
    expect(selected?.textContent).toContain('待处理');
  });

  it('offers a language switcher in the top bar', () => {
    stubFetch({ '/api/team/projects': DEMO_PROJECTS, '/api/team/overview': DEMO_OVERVIEW });
    renderApp('/workbench');
    expect(screen.getByTestId('language-switcher')).toBeTruthy();
  });

  it('preserves the selected project when opening the inbox from a project page', async () => {
    stubFetch({ '/api/team/projects': DEMO_PROJECTS, '/api/team/overview': DEMO_OVERVIEW,
      '/api/team/read': { ok: true, result: { queries: [] } } });
    const user = userEvent.setup();
    const { router } = renderApp('/projects/demo');
    await waitFor(() => expect(screen.getByTestId('scope-switcher').textContent).toBe('Team mainline'));
    const inbox = screen.getByRole('link', { name: '待处理' });
    expect(inbox.getAttribute('href')).toBe('/inbox?scope=demo');
    await user.click(inbox);
    expect(router.state.location.pathname).toBe('/inbox');
    expect(router.state.location.search).toBe('?scope=demo');
  });

  it('uses the registered project key when display metadata is unusable', async () => {
    stubFetch({ '/api/team/projects': { ...DEMO_PROJECTS, projects: [{ key: 'private-key', title: 'client_obs_658' }] },
      '/api/team/overview': DEMO_OVERVIEW });
    const user = userEvent.setup();
    renderApp('/projects/private-key');
    await waitFor(() => expect(screen.getByTestId('scope-switcher').textContent).toBe('private-key'));
    await user.click(screen.getByTestId('scope-switcher'));
    expect(screen.getByRole('menuitem', { name: 'private-key' })).toBeTruthy();
    expect(screen.queryByText('client_obs_658')).toBeNull();
  });
});
