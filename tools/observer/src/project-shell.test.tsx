import { screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it } from 'vitest';
import { defaultVisibleText } from './lib/human-language-guard';
import { demoOverviewModelResponse } from './test-fixtures/overview-model';
import { DEMO_OVERVIEW, DEMO_PROJECTS, renderApp, stubFetch } from './test-utils';

const TAB_LABELS = ['概览', '工作计划', '依赖', '成果与验收', '项目知识'];

const RICH_PROJECTS = {
  ...DEMO_PROJECTS,
  projects: [{
    key: 'demo',
    title: 'Team mainline',
    role: 'admin',
    leadPersonId: 'M-001',
    groupName: '应用小组',
  }],
};

function setup(path: string) {
  const model = demoOverviewModelResponse();
  model.input.members = {
    coverage: 'complete',
    items: [{ personId: 'M-001', displayName: '周宁' }],
  };
  stubFetch({
    '/api/team/projects': RICH_PROJECTS,
    '/api/team/overview-model': {
      ...model,
      read_at_unix_ms: Date.now(),
      periodUpdates: {
        coverage: 'complete',
        items: [{
          id: 'U-1',
          periodLabel: '第 47 周',
          startsOn: '2026-11-16',
          endsOn: '2026-11-22',
          status: 'risk',
          summary: '接口复核拖了两天。',
          authorPersonId: 'M-001',
          recordedAtUnixMs: model.input.options.nowUnixMs - 60_000,
        }],
      },
    },
    '/api/team/overview': DEMO_OVERVIEW,
    '/api/team/graph': {
      ok: true, project: 'demo', read_at_unix_ms: 1790000000000,
      as_of: { project_revision: '12', source_snapshot_id: 'source-12', coordinator_epoch: 'current' },
      workstreams: [{ id: 'development', title: 'Development' }],
      items: [{
        work_id: 'synthetic-api', external_key: 'API-001', title: 'Application API',
        workstream_id: 'development', state: 'in_progress', owner: null,
        last_event_at: null, claims: [], dependencies_complete: true, required_dependencies: [],
      }],
    },
  });
  return renderApp(path);
}

describe('ProjectShell', () => {
  it('renders the prototype header with status tag, lead link, group, task count and workstream count without internal keys', async () => {
    const user = userEvent.setup();
    const { router } = setup('/projects/demo');
    await waitFor(() => expect(screen.getByTestId('project-title').textContent).toBe('Team mainline'));
    await waitFor(() => expect(screen.getByTestId('project-lead-link').textContent).toContain('周宁'));

    const header = screen.getByTestId('project-header');
    const visible = defaultVisibleText(header);
    expect(visible).toContain('Team mainline');
    expect(visible).toContain('有风险');
    expect(visible).toContain('负责人');
    expect(visible).toContain('周宁');
    expect(visible).toContain('应用小组');
    expect(visible).toMatch(/\d+\s*件任务/);
    expect(visible).toMatch(/\d+\s*条工作线/);
    expect(visible).not.toMatch(/项目键|demo|M-001/);
    expect(document.querySelector('[data-testid="technical-details"]')).toBeNull();
    expect(screen.queryByTestId('technical-project-key')).toBeNull();

    // Back button returns to /projects.
    expect(screen.getByTestId('project-back').textContent).toContain('全部项目');
    await user.click(screen.getByTestId('project-lead-link'));
    expect(router.state.location.pathname).toBe('/people/M-001');
  });

  it('renders the five tabs in exact order', async () => {
    setup('/projects/demo');
    await waitFor(() => expect(screen.getByRole('tab', { name: '概览' })).toBeTruthy());
    const tabs = screen.getAllByRole('tab').map((el) => el.textContent?.trim());
    expect(tabs).toEqual(TAB_LABELS);
  });

  it('keeps the active tab in the URL search params and clears overview detail params on switch', async () => {
    const user = userEvent.setup();
    const { router } = setup('/projects/demo?tab=overview&detail=verified');
    await waitFor(() => expect(screen.getByRole('tab', { name: '工作计划' })).toBeTruthy());
    await user.click(screen.getByRole('tab', { name: '工作计划' }));
    expect(router.state.location.search).toContain('tab=plan');
    expect(router.state.location.search).not.toContain('detail=');
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

  it('renders all three shared maps and opens readable work details from the dependencies tab', async () => {
    const user = userEvent.setup();
    const { queryClient } = setup('/projects/demo?tab=deps');
    const body = await screen.findByTestId('project-map-drawing');
    await waitFor(() => expect(body.querySelectorAll('svg').length).toBe(3));
    const query = queryClient.getQueryCache().find({ queryKey: ['team', 'graph', 'demo'] })!;
    expect(query.observers.map((observer) => observer.options.refetchInterval)).toEqual([false]);
    expect(defaultVisibleText(body)).toContain('Application API');
    await waitFor(() => expect(defaultVisibleText(body)).toContain('Team mainline'));
    expect(defaultVisibleText(body)).toContain('Development');
    expect(defaultVisibleText(body)).not.toContain('demo');
    await user.click(body.querySelector('.pm-card')!);
    const drawer = body.querySelector('.pm-drawer')!;
    expect(defaultVisibleText(drawer)).not.toMatch(/in_progress|synthetic-api|node\.milestone/);
    expect(within(drawer as HTMLElement).queryByRole('button', { name: '复制给 Agent' })).toBeNull();
    expect(within(drawer as HTMLElement).getByRole('button', { name: '在图中定位' })).toBeTruthy();
    expect(drawer.querySelector('details[data-testid="technical-details"][open]')).toBeNull();
    expect(screen.queryByTestId('state-empty')).toBeNull();
    expect(screen.queryByTestId('project-deps-not-open')).toBeNull();
  });
});
