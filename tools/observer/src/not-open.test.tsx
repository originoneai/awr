import { act, screen, within } from '@testing-library/react';
import { afterEach, describe, expect, it } from 'vitest';
import i18n from './i18n';
import { defaultVisibleText } from './lib/human-language-guard';
import { DEMO_PROJECTS, renderApp, stubFetch } from './test-utils';

afterEach(async () => {
  await act(async () => { await i18n.changeLanguage('zh-CN'); });
});

describe('pages whose feature is not open yet', () => {
  it.each([
    ['/knowledge', '经验', '/lessons'],
    ['/settings', '管理设置', '/settings'],
    ['/advanced', '管理设置', '/settings'],
  ])('%s says once that it is not open, and does not read as an empty read', async (path, title, destination) => {
    stubFetch({ '/api/team/projects': DEMO_PROJECTS });
    const { router } = renderApp(path);
    const notOpen = await screen.findByTestId('state-not-open');
    const page = notOpen.parentElement as HTMLElement;
    expect(defaultVisibleText(page)).toBe(`${title}\n这里暂时没有内容\n该功能尚未开放`);
    expect(router.state.location.pathname).toBe(destination);
    expect(screen.queryByTestId('state-empty')).toBeNull();
    expect(screen.queryByText(/未观察到相关记录|尚未完成读取/)).toBeNull();
  });

  it('the inbox asks for a project before observing questions', async () => {
    stubFetch({ '/api/team/projects': DEMO_PROJECTS });
    renderApp('/inbox');
    const page = await screen.findByTestId('page-inbox');
    expect(defaultVisibleText(page)).toContain('选择一个项目，查看它的问题与答复。');
    expect(page.textContent).not.toMatch(/未观察到相关记录|尚未完成读取|暂无数据/);
    expect(screen.queryByTestId('state-empty')).toBeNull();
    expect(within(page).getByRole('link', { name: '选择项目' }).getAttribute('href')).toBe('/projects');
  });

  it('says the same in English', async () => {
    await act(async () => { await i18n.changeLanguage('en-US'); });
    stubFetch({ '/api/team/projects': DEMO_PROJECTS });
    renderApp('/inbox');
    const page = await screen.findByTestId('page-inbox');
    expect(defaultVisibleText(page)).toContain('Choose a project to see its questions and answers.');
    expect(within(page).getByRole('link', { name: 'Choose a project' }).getAttribute('href')).toBe('/projects');
  });
});
