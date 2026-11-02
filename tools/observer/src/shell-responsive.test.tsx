import { act, fireEvent, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it } from 'vitest';
import i18n from './i18n';
import { DEMO_OVERVIEW, DEMO_PROJECTS, mockViewport, renderApp, stubFetch } from './test-utils';

let narrowViewport: ReturnType<typeof mockViewport> | null = null;

function setupNarrow(path = '/workbench') {
  stubFetch({ '/api/team/projects': DEMO_PROJECTS, '/api/team/overview': DEMO_OVERVIEW });
  narrowViewport = mockViewport(375);
  return renderApp(path);
}

afterEach(async () => {
  await i18n.changeLanguage('zh-CN');
  // Restore here rather than in each test: a failing assertion must not leak the narrow
  // matchMedia into later test files.
  narrowViewport?.restore();
  narrowViewport = null;
});

describe('responsive navigation below lg', () => {
  it('offers the header entry instead of the sider and the zero-width trigger', async () => {
    setupNarrow();
    const entry = await screen.findByRole('button', { name: '打开导航' });
    expect(entry.tagName).toBe('BUTTON');
    expect(document.querySelector('.ant-layout-sider')).toBeNull();
    expect(document.querySelector('.ant-layout-sider-zero-width-trigger')).toBeNull();
  });

  it('opens the drawer with every navigation item and closes it after choosing a destination', async () => {
    const user = userEvent.setup();
    const { router } = setupNarrow();
    await user.click(await screen.findByRole('button', { name: '打开导航' }));

    const dialog = await screen.findByRole('dialog');
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
      expect(dialog.textContent).toContain(label);
    }
    expect(await screen.findByRole('button', { name: '关闭导航' })).toBeTruthy();

    await user.click(screen.getByRole('link', { name: '账户与连接' }));
    await waitFor(() => {
      expect(document.querySelector('.ant-drawer-open')).toBeNull();
      expect(screen.queryByRole('dialog')).toBeNull();
    });
    expect(router.state.location.pathname).toBe('/account');
  });

  it('returns focus to the entry button when the drawer closes via Escape', async () => {
    const user = userEvent.setup();
    setupNarrow();
    const entry = await screen.findByRole('button', { name: '打开导航' });
    await user.click(entry);
    await screen.findByRole('dialog');

    fireEvent.keyDown(document.activeElement ?? document.body, { key: 'Escape' });
    await waitFor(() => {
      expect(document.querySelector('.ant-drawer-open')).toBeNull();
    });
    await waitFor(() => {
      expect(document.activeElement).toBe(entry);
    });
  });

  it('keeps the desktop shell at lg and above', async () => {
    stubFetch({ '/api/team/projects': DEMO_PROJECTS, '/api/team/overview': DEMO_OVERVIEW });
    renderApp('/workbench');
    await screen.findByAltText('AWR');
    expect(screen.queryByRole('button', { name: '打开导航' })).toBeNull();
    expect(document.querySelector('.ant-layout-sider')).not.toBeNull();
  });

  it('closes an open drawer when the viewport grows past lg and does not reopen it when shrinking', async () => {
    const user = userEvent.setup();
    narrowViewport = mockViewport(375);
    stubFetch({ '/api/team/projects': DEMO_PROJECTS, '/api/team/overview': DEMO_OVERVIEW });
    renderApp('/workbench');
    await user.click(await screen.findByRole('button', { name: '打开导航' }));
    await screen.findByRole('dialog');
    const { setWidth } = narrowViewport;

    await act(async () => {
      setWidth(1341);
    });
    await waitFor(() => {
      expect(document.querySelector('.ant-layout-sider')).not.toBeNull();
      expect(screen.queryByRole('button', { name: '打开导航' })).toBeNull();
      expect(document.querySelector('.ant-drawer-open')).toBeNull();
    });

    await act(async () => {
      setWidth(375);
    });
    await waitFor(() => {
      expect(screen.getByRole('button', { name: '打开导航' })).toBeTruthy();
    });
    expect(document.querySelector('.ant-drawer-open')).toBeNull();
  });

  it('labels the mobile entry in the selected language', async () => {
    const user = userEvent.setup();
    setupNarrow();
    await user.click(screen.getByTestId('language-switcher'));
    await user.click(await screen.findByText('English'));
    expect(await screen.findByRole('button', { name: 'Open navigation' })).toBeTruthy();
    await user.click(screen.getByRole('button', { name: 'Open navigation' }));
    expect(await screen.findByRole('button', { name: 'Close navigation' })).toBeTruthy();
  });
});
