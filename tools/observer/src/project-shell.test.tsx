import { screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it } from 'vitest';
import { DEMO_OVERVIEW, DEMO_PROJECTS, renderApp, stubFetch } from './test-utils';

const TAB_LABELS = ['概览', '工作计划', '协作依赖', '成果与验收', '项目知识'];

function setup(path: string) {
  stubFetch({ '/api/team/projects': DEMO_PROJECTS, '/api/team/overview': DEMO_OVERVIEW });
  return renderApp(path);
}

describe('ProjectShell', () => {
  it('renders the project header with name, status tag, meta and settings entry', async () => {
    setup('/projects/demo');
    await screen.findByText('Team mainline');
    const header = screen.getByTestId('project-header');
    expect(header.textContent).toContain('Team mainline');
    expect(header.textContent).toContain('project_admin');
    expect(screen.getByRole('button', { name: /项目设置/ })).toBeTruthy();
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

  it('tab bodies render the shared pending presentation, not fake data', async () => {
    setup('/projects/demo?tab=plan');
    await waitFor(() => expect(screen.getAllByText(/未观察到相关记录/).length).toBeGreaterThan(0));
  });
});
