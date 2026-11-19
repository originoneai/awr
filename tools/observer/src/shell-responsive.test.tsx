import { act, fireEvent, screen, waitFor } from '@testing-library/react';
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
  it('offers the top page selector instead of the sider and keeps a 40px theme target', async () => {
    setupNarrow();
    const entry = await screen.findByRole('combobox', { name: '页面' });
    expect(entry).toBeTruthy();
    expect(document.querySelector('.ant-layout-sider')).toBeNull();
    expect(document.querySelector('.ant-layout-sider-zero-width-trigger')).toBeNull();
    const theme = screen.getByTestId('mobile-theme-toggle');
    expect(theme.style.width).toBe('40px');
    expect(theme.style.height).toBe('40px');
  });

  it('opens the existing drawer with every destination and bottom theme, then closes after navigation', async () => {
    const user = userEvent.setup();
    const { router } = setupNarrow();
    await user.click(await screen.findByRole('combobox', { name: '页面' }));

    const dialog = await screen.findByRole('dialog');
    for (const label of ['工作台', '项目', '待处理', '经验', '成员与 Agent', '活动与审计', '管理设置']) {
      expect(dialog.textContent).toContain(label);
    }
    expect(dialog.querySelector('.ant-drawer-body')?.lastElementChild?.classList.contains('shell-theme')).toBe(true);
    expect(dialog.querySelector('[data-testid="theme-toggle"]')).not.toBeNull();
    expect(await screen.findByRole('button', { name: '关闭导航' })).toBeTruthy();

    await user.click(screen.getByRole('link', { name: '经验' }));
    await waitFor(() => {
      expect(document.querySelector('.ant-drawer-open')).toBeNull();
      expect(screen.queryByRole('dialog')).toBeNull();
    });
    expect(router.state.location.pathname).toBe('/lessons');
  });

  it('returns focus to the page selector when the drawer closes via Escape', async () => {
    const user = userEvent.setup();
    setupNarrow();
    const entry = await screen.findByRole('combobox', { name: '页面' });
    await user.click(entry);
    await screen.findByRole('dialog');

    fireEvent.keyDown(document.activeElement ?? document.body, { key: 'Escape' });
    await waitFor(() => expect(document.querySelector('.ant-drawer-open')).toBeNull());
    await waitFor(() => expect(document.activeElement).toBe(entry));
  });

  it('keeps the desktop shell at 861px', async () => {
    stubFetch({ '/api/team/projects': DEMO_PROJECTS, '/api/team/overview': DEMO_OVERVIEW });
    narrowViewport = mockViewport(861);
    renderApp('/workbench');
    await screen.findByAltText('AWR');
    expect(screen.queryByRole('combobox', { name: '页面' })).toBeNull();
    expect(document.querySelector('.ant-layout-sider')).not.toBeNull();
  });

  it('uses the top page selector at exactly 860px', async () => {
    setupNarrow('/workbench', 860);
    expect(await screen.findByRole('combobox', { name: '页面' })).toBeTruthy();
    expect(document.querySelector('.ant-layout-sider')).toBeNull();
  });

  it('marks the current page in the drawer menu', async () => {
    const user = userEvent.setup();
    setupNarrow('/lessons');
    await user.click(await screen.findByRole('combobox', { name: '页面' }));
    const dialog = await screen.findByRole('dialog');
    const selected = dialog.querySelectorAll('.ant-menu-item-selected');
    expect(selected).toHaveLength(1);
    expect(selected[0]?.textContent).toContain('经验');
  });

  it('closes the drawer when the mask is tapped', async () => {
    const user = userEvent.setup();
    setupNarrow();
    const entry = await screen.findByRole('combobox', { name: '页面' });
    await user.click(entry);
    await screen.findByRole('dialog');
    const mask = document.querySelector('.ant-drawer-mask');
    expect(mask).not.toBeNull();
    await user.click(mask as HTMLElement);
    await waitFor(() => expect(document.querySelector('.ant-drawer-open')).toBeNull());
    await waitFor(() => expect(document.activeElement).toBe(entry));
  });

  it('closes an open drawer above 860px and does not reopen it when shrinking', async () => {
    const user = userEvent.setup();
    setupNarrow();
    await user.click(await screen.findByRole('combobox', { name: '页面' }));
    await screen.findByRole('dialog');
    const viewport = narrowViewport;
    expect(viewport).not.toBeNull();

    await act(async () => viewport?.setWidth(861));
    await waitFor(() => {
      expect(document.querySelector('.ant-layout-sider')).not.toBeNull();
      expect(screen.queryByRole('combobox', { name: '页面' })).toBeNull();
      expect(document.querySelector('.ant-drawer-open')).toBeNull();
    });

    await act(async () => viewport?.setWidth(860));
    await waitFor(() => expect(screen.getByRole('combobox', { name: '页面' })).toBeTruthy());
    expect(document.querySelector('.ant-drawer-open')).toBeNull();
  });

  it('labels the mobile page selector and drawer in the selected language', async () => {
    const user = userEvent.setup();
    setupNarrow();
    await act(async () => { await i18n.changeLanguage('en-US'); });
    await user.click(await screen.findByRole('combobox', { name: 'Page' }));
    expect(await screen.findByRole('button', { name: 'Close navigation' })).toBeTruthy();
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
