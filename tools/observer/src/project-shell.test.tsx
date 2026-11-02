import { screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it } from 'vitest';
import { defaultVisibleText } from './lib/human-language-guard';
import { DEMO_OVERVIEW, DEMO_PROJECTS, renderApp, stubFetch } from './test-utils';

const TAB_LABELS = ['概览', '工作计划', '协作依赖', '成果与验收', '项目知识'];

function setup(path: string) {
  stubFetch({ '/api/team/projects': DEMO_PROJECTS, '/api/team/overview': DEMO_OVERVIEW });
  return renderApp(path);
}

describe('ProjectShell', () => {
  it('renders the project header with name, role and settings entry, and keeps the project key behind the technical details', async () => {
    const user = userEvent.setup();
    setup('/projects/demo');
    await screen.findByText('Team mainline');
    const header = screen.getByTestId('project-header');
    expect(defaultVisibleText(header)).toBe('Team mainline\n项目管理员\n项目设置');
    expect(defaultVisibleText(header)).not.toMatch(/项目键|demo/);
    expect(screen.getByRole('button', { name: /项目设置/ })).toBeTruthy();
    await user.click(within(header).getByRole('button', { name: '技术细节' }));
    expect(within(header).getByTestId('technical-project-key').textContent).toContain('demo');
  });

  it('renders the five tabs', async () => {
    setup('/projects/demo');
    await waitFor(() => expect(screen.getByRole('tab', { name: /概览/ })).toBeTruthy());
    for (const label of TAB_LABELS) {
      expect(screen.getByRole('tab', { name: new RegExp(label) })).toBeTruthy();
    }
  });

  it('keeps the active tab in the URL search params', async () => {
    const user = userEvent.setup();
    const { router } = setup('/projects/demo');
    await waitFor(() => expect(screen.getByRole('tab', { name: /工作计划/ })).toBeTruthy());
    await user.click(screen.getByRole('tab', { name: /工作计划/ }));
    expect(router.state.location.search).toContain('tab=plan');
    await waitFor(() =>
      expect(document.querySelector('.ant-tabs-tab-active')?.textContent).toContain('工作计划'),
    );
  });

  it('restores the active tab from the URL', async () => {
    setup('/projects/demo?tab=knowledge');
    await waitFor(() =>
      expect(document.querySelector('.ant-tabs-tab-active')?.textContent).toContain('项目知识'),
    );
  });

  it('says the dependencies tab is not open yet, once, and does not read as an empty read', async () => {
    setup('/projects/demo?tab=deps');
    const body = await screen.findByTestId('project-deps-not-open');
    expect(defaultVisibleText(body)).toBe('这里暂时没有内容\n该功能尚未开放');
    expect(screen.queryByTestId('state-empty')).toBeNull();
    expect(screen.queryByText(/未观察到相关记录|尚未完成读取/)).toBeNull();
  });
});
