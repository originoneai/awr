import enUS from 'antd/locale/en_US';
import zhCN from 'antd/locale/zh_CN';
import { act, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import i18n from './i18n';
import { antdLocale } from './RootLayout';
import { DEMO_OVERVIEW, DEMO_PROJECTS, renderApp, stubFetch } from './test-utils';
import { themeConfig } from './theme';

function setup(path = '/workbench') {
  stubFetch({ '/api/team/projects': DEMO_PROJECTS, '/api/team/overview': DEMO_OVERVIEW });
  return renderApp(path);
}

beforeEach(() => {
  window.__CSP_NONCE__ = 'test-csp-nonce';
});

afterEach(async () => {
  await i18n.changeLanguage('zh-CN');
  window.localStorage.removeItem('i18nextLng');
});

describe('prototype shell behavior', () => {
  it('uses a full-height sider with the AWR asset above a content-only header', () => {
    const { container } = setup();
    const sider = container.querySelector('.ant-layout-sider');
    const innerLayout = sider?.nextElementSibling;
    expect(sider?.querySelector('[data-testid="awr-brand"]')).not.toBeNull();
    expect(innerLayout?.classList.contains('ant-layout')).toBe(true);
    expect(innerLayout?.querySelector(':scope > .ant-layout-header')).not.toBeNull();
    expect(innerLayout?.querySelector(':scope > .ant-layout-content')).not.toBeNull();
    expect(container.querySelector(':scope > .ant-layout > .ant-layout-header')).toBeNull();
    expect(themeConfig.token?.colorBgLayout).toBe('#F3F5F9');
    expect(themeConfig.components?.Menu?.itemHeight).toBe(36);
  });

  it('renders real NavLinks and derives project selection from nested routes', async () => {
    const { router } = setup('/workbench');
    const projectsLink = screen.getByRole('link', { name: '项目' });
    expect(projectsLink.getAttribute('href')).toBe('/projects');
    await act(() => router.navigate('/projects/demo'));
    await waitFor(() => {
      expect(document.querySelector('.ant-menu-item-selected')?.textContent).toContain('项目');
    });
  });

  it('stores scope in the URL and agrees with the project route', async () => {
    const user = userEvent.setup();
    const { router } = setup('/workbench');
    await user.click(screen.getByTestId('scope-switcher'));
    await user.click(await screen.findByText('Docs stream'));
    expect(router.state.location.search).toContain('scope=docs');
    expect(screen.getByTestId('scope-switcher').textContent).toContain('Docs stream');

    await act(() => router.navigate('/projects/demo?tab=plan'));
    await waitFor(() => expect(screen.getByTestId('scope-switcher').textContent).toContain('Team mainline'));
    expect(router.state.location.pathname).toBe('/projects/demo');

    await user.click(screen.getByTestId('scope-switcher'));
    await user.click(await screen.findByText('全部授权范围'));
    expect(router.state.location.pathname).toBe('/projects');
    expect(router.state.location.search).not.toContain('scope=');
    expect(screen.getByTestId('scope-switcher').textContent).toContain('全部授权范围');
  });

  it('has no fake organization, notification dot, or constant role text', () => {
    setup();
    expect(screen.queryByText('示例组织')).toBeNull();
    expect(screen.queryByText('观察者')).toBeNull();
    expect(document.querySelector('.ant-badge-dot')).toBeNull();
    expect(screen.getByTestId('scope-switcher').textContent).toContain('全部授权范围');
  });

  it('keeps future controls disabled while the enabled perspective falls back safely', async () => {
    const user = userEvent.setup();
    setup('/projects/demo');
    const search = screen.getByRole('button', { name: /搜索/ });
    const notifications = screen.getByRole('button', { name: '待处理' });
    const settings = await screen.findByRole('button', { name: '项目设置' });
    for (const control of [search, settings]) expect(control).toHaveProperty('disabled', true);
    expect(screen.queryByTestId('perspective-switcher')).toBeNull();
    expect(document.querySelector('.shell-header .shell-perspective')).toBeNull();
    expect(notifications).toHaveProperty('disabled', false);
    expect(screen.getByTestId('current-member-avatar')).toBeTruthy();
    await user.hover(search);
    expect((await screen.findAllByText('该功能尚未开放')).length).toBeGreaterThan(0);
  });

  it('keeps the selected language key and persists the choice', async () => {
    setup();
    await act(async () => { await i18n.changeLanguage('en-US'); });
    await waitFor(() => expect(window.localStorage.getItem('i18nextLng')).toBe('en-US'));
    expect(screen.getByRole('menuitem', { name: /Workbench/ })).toBeTruthy();
    expect(screen.queryByText('数据来源')).toBeNull();
    expect(screen.queryByText('以谁的身份看')).toBeNull();
  });

  it('switches the navigation at the prototype breakpoint', () => {
    const media = vi.fn((query: string) => ({
      matches: false,
      media: query,
      onchange: null,
      addListener: () => {},
      removeListener: () => {},
      addEventListener: () => {},
      removeEventListener: () => {},
      dispatchEvent: () => false,
    })) as unknown as typeof window.matchMedia;
    window.matchMedia = media;
    setup();
    expect(vi.mocked(media).mock.calls.some(([query]) => query.includes('max-width: 860px'))).toBe(true);
  });

  it('uses the narrow-screen layout without fixed-width content', () => {
    setup();
    const header = document.querySelector('.ant-layout-header');
    const content = document.querySelector('.ant-layout-content');
    expect(header?.classList.contains('shell-header')).toBe(true);
    expect(content?.classList.contains('shell-content')).toBe(true);
    expect(document.querySelector('.shell-subtitle')).toBeNull();
    expect(getComputedStyle(document.documentElement).overflowX).not.toBe('scroll');
  });
});

describe('theme and antd integration', () => {
  it('uses the approved indicator accent', () => {
    expect(themeConfig.token?.colorPrimary).toBe('#1D4ED8');
  });

  it('maps Chinese and English to the corresponding antd locales', () => {
    expect(antdLocale('zh-CN')).toBe(zhCN);
    expect(antdLocale('en-US')).toBe(enUS);
    expect(antdLocale('fr-FR')).toBe(enUS);
  });

  it('passes the served CSP nonce to styles injected by antd', async () => {
    window.__CSP_NONCE__ = 'test-csp-nonce';
    setup();
    await waitFor(() => expect(document.querySelectorAll('style[data-css-hash]').length).toBeGreaterThan(0));
    const styles = Array.from(document.querySelectorAll('style[data-css-hash]'));
    for (const style of styles) expect(style.getAttribute('nonce')).toBe('test-csp-nonce');
  });
});
