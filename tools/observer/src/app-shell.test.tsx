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

  it('renders the wordmark and scope dropdown placeholder', () => {
    stubFetch({ '/api/team/projects': DEMO_PROJECTS, '/api/team/overview': DEMO_OVERVIEW });
    renderApp('/workbench');
    expect(screen.getByText('AWR')).toBeTruthy();
    expect(screen.getByText('示例组织')).toBeTruthy();
  });

  it('navigates when a menu item is clicked', async () => {
    stubFetch({ '/api/team/projects': DEMO_PROJECTS, '/api/team/overview': DEMO_OVERVIEW });
    const user = userEvent.setup();
    const { router } = renderApp('/workbench');
    await user.click(screen.getByRole('menuitem', { name: /项目/ }));
    expect(router.state.location.pathname).toBe('/projects');
    await waitFor(() => expect(screen.getByTestId('page-projects')).toBeTruthy());
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
});
