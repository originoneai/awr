import { act, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it } from 'vitest';
import i18n from './i18n';
import { DEMO_OVERVIEW, DEMO_PROJECTS, mockViewport, renderApp, stubFetch } from './test-utils';

let narrowViewport: ReturnType<typeof mockViewport> | null = null;

function setupNarrow(path = '/workbench', width = 375) {
  stubFetch({ '/api/team/projects': DEMO_PROJECTS, '/api/team/overview': DEMO_OVERVIEW });
  narrowViewport = mockViewport(width);
  return renderApp(path);
}

afterEach(async () => {
  await i18n.changeLanguage('zh-CN');
  narrowViewport?.restore();
  narrowViewport = null;
});

describe('responsive navigation at 860px', () => {
  it('uses a native select and top theme instead of the sider or a drawer', async () => {
    setupNarrow();
    const entry = await screen.findByRole<HTMLSelectElement>('combobox', { name: '页面' });
    expect(entry.tagName).toBe('SELECT');
    expect(document.querySelector('.ant-layout-sider')).toBeNull();
    expect(document.querySelector('.ant-layout-sider-zero-width-trigger')).toBeNull();
    expect(document.querySelector('.ant-drawer')).toBeNull();
    const theme = screen.getByTestId('mobile-theme-toggle');
    expect(theme.style.width).toBe('40px');
    expect(theme.style.height).toBe('40px');
    expect(screen.getByTestId('avatar-menu-trigger')).toBeTruthy();
  });

  it('navigates directly on selection and preserves the current project', async () => {
    const user = userEvent.setup();
    const { router } = setupNarrow('/workbench?scope=demo');
    const entry = await screen.findByRole<HTMLSelectElement>('combobox', { name: '页面' });
    await user.selectOptions(entry, '/lessons');
    await screen.findByTestId('page-lessons');
    expect(router.state.location.pathname).toBe('/lessons');
    expect(router.state.location.search).toBe('?scope=demo');
    expect(entry.value).toBe('/lessons');
    expect(document.querySelector('.ant-drawer')).toBeNull();
    expect(screen.getByTestId('mobile-theme-toggle')).toBeTruthy();
  });

  it('offers every destination in the native select and marks the current page', async () => {
    setupNarrow('/lessons');
    const entry = await screen.findByRole<HTMLSelectElement>('combobox', { name: '页面' });
    expect(entry.value).toBe('/lessons');
    expect(Array.from(entry.options).map((option) => option.textContent)).toEqual([
      '工作台', '项目', '待处理', '经验', '成员与 Agent', '活动与审计', '管理设置',
    ]);
  });

  it('keeps the desktop shell at 861px', async () => {
    stubFetch({ '/api/team/projects': DEMO_PROJECTS, '/api/team/overview': DEMO_OVERVIEW });
    narrowViewport = mockViewport(861);
    renderApp('/workbench');
    await screen.findByTestId('awr-brand');
    expect(screen.queryByRole('combobox', { name: '页面' })).toBeNull();
    expect(document.querySelector('.ant-layout-sider')).not.toBeNull();
  });

  it('uses the native page selector at exactly 860px', async () => {
    setupNarrow('/workbench', 860);
    expect(await screen.findByRole('combobox', { name: '页面' })).toBeTruthy();
    expect(document.querySelector('.ant-layout-sider')).toBeNull();
  });

  it('switches between the native selector and sider on resize without creating a drawer', async () => {
    setupNarrow('/workbench', 860);
    await screen.findByRole('combobox', { name: '页面' });
    const viewport = narrowViewport;
    expect(viewport).not.toBeNull();

    await act(async () => viewport?.setWidth(861));
    await waitFor(() => {
      expect(document.querySelector('.ant-layout-sider')).not.toBeNull();
      expect(screen.queryByRole('combobox', { name: '页面' })).toBeNull();
    });
    await act(async () => viewport?.setWidth(860));
    await screen.findByRole('combobox', { name: '页面' });
    expect(document.querySelector('.ant-drawer')).toBeNull();
  });

  it('opens real account and Agent connection destinations from the mobile avatar menu', async () => {
    const user = userEvent.setup();
    const { router } = setupNarrow('/workbench?scope=demo');
    await user.click(await screen.findByTestId('avatar-menu-trigger'));
    const account = await screen.findByRole('link', { name: '账户与连接' });
    expect(account.getAttribute('href')).toBe('/account?scope=demo');
    expect(screen.getAllByRole('link', { name: '连接我的 Agent' })
      .some((link) => link.getAttribute('href') === '/agent-connect?scope=demo')).toBe(true);
    await user.click(account);
    await waitFor(() => expect(router.state.location.pathname).toBe('/account'));
    expect(router.state.location.search).toBe('?scope=demo');
  });

  it('changes language from the mobile avatar menu', async () => {
    const user = userEvent.setup();
    setupNarrow();
    await user.click(await screen.findByTestId('avatar-menu-trigger'));
    await user.click(await screen.findByText('English'));
    await waitFor(() => expect(i18n.language).toBe('en-US'));
    const entry = await screen.findByRole<HTMLSelectElement>('combobox', { name: 'Page' });
    expect(Array.from(entry.options).map((option) => option.textContent)).toContain('Lessons');
  });

  it('wraps the top bar and removes fixed-width shell content on narrow screens', async () => {
    setupNarrow();
    await screen.findByRole('combobox', { name: '页面' });
    const header = document.querySelector<HTMLElement>('.shell-header');
    const content = document.querySelector<HTMLElement>('.shell-content');
    const mobile = screen.getByTestId('mobile-navigation');
    expect(header?.style.flexWrap).toBe('wrap');
    expect(header?.style.height).toBe('auto');
    expect(content?.style.padding).toBe('16px');
    expect(mobile.style.minWidth).toBe('0px');
    expect(getComputedStyle(document.documentElement).overflowX).not.toBe('scroll');
  });
});
