import { act, screen } from '@testing-library/react';
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

  it('says the same in English', async () => {
    await act(async () => { await i18n.changeLanguage('en-US'); });
    stubFetch({ '/api/team/projects': DEMO_PROJECTS });
    renderApp('/knowledge');
    const notOpen = await screen.findByTestId('state-not-open');
    expect(defaultVisibleText(notOpen.parentElement as HTMLElement)).toContain('This feature is not available yet');
  });
});
