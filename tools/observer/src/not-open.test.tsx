import { act, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it } from 'vitest';
import i18n from './i18n';
import { defaultVisibleText } from './lib/human-language-guard';
import { DEMO_PROJECTS, renderApp, stubFetch } from './test-utils';

afterEach(async () => {
  await act(async () => { await i18n.changeLanguage('zh-CN'); });
});

describe('pages whose feature is not open yet', () => {
  it.each([
    ['/knowledge', '知识与技能'],
    ['/members', '成员与 Agent'],
    ['/settings', '管理设置'],
    ['/advanced', '高级工具'],
  ])('%s says once that it is not open, and does not read as an empty read', async (path, title) => {
    stubFetch({ '/api/team/projects': DEMO_PROJECTS });
    renderApp(path);
    const notOpen = await screen.findByTestId('state-not-open');
    const page = notOpen.parentElement as HTMLElement;
    expect(defaultVisibleText(page)).toBe(`${title}\n这里暂时没有内容\n该功能尚未开放`);
    expect(screen.queryByTestId('state-empty')).toBeNull();
    expect(screen.queryByText(/未观察到相关记录|尚未完成读取/)).toBeNull();
  });

  it('the inbox says it is not open and points to the "needs me" tab of the workbench, instead of reading as an empty inbox', async () => {
    const user = userEvent.setup();
    stubFetch({ '/api/team/projects': DEMO_PROJECTS });
    const { router } = renderApp('/inbox');
    const page = await screen.findByTestId('page-inbox');
    expect(defaultVisibleText(page)).toBe('待处理\n这里暂时没有内容\n该功能尚未开放\n需要你处理的事项，现在请在工作台的“\n需要我处理\n”里查看');
    expect(page.textContent).not.toMatch(/未观察到相关记录|尚未完成读取|暂无数据/);
    expect(screen.queryByTestId('state-empty')).toBeNull();
    const link = within(page).getByRole('link', { name: '需要我处理' });
    expect(link.getAttribute('href')).toBe('/workbench?tab=needs-me');
    await user.click(link);
    expect(router.state.location.pathname).toBe('/workbench');
    expect(router.state.location.search).toBe('?tab=needs-me');
  });

  it('says the same in English', async () => {
    await act(async () => { await i18n.changeLanguage('en-US'); });
    stubFetch({ '/api/team/projects': DEMO_PROJECTS });
    renderApp('/inbox');
    const page = await screen.findByTestId('page-inbox');
    expect(defaultVisibleText(page)).toBe('Inbox\nNothing here yet\nThis feature is not available yet\nFor now, find what needs you on the workbench, under "\nNeeds me\n"');
    expect(within(page).getByRole('link', { name: 'Needs me' }).getAttribute('href')).toBe('/workbench?tab=needs-me');
  });
});
