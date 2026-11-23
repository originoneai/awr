import { screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';

// Prove the data router mounts ScrollRestoration without depending on jsdom scroll APIs.
vi.mock('react-router', async (importOriginal) => {
  const mod = await importOriginal<typeof import('react-router')>();
  return {
    ...mod,
    ScrollRestoration: () => <div data-testid="scroll-restoration" />,
  };
});

describe('router configuration', () => {
  it('is a data router created with createBrowserRouter', async () => {
    const { router } = await import('./router');
    expect(typeof router.navigate).toBe('function');
    expect(typeof router.fetch).toBe('function');
    expect(router.state.location.pathname).toBeDefined();
  });

  it('mounts ScrollRestoration in the root layout and redirects the root to workbench', async () => {
    const { renderApp, stubFetch, DEMO_PROJECTS, DEMO_OVERVIEW } = await import('./test-utils');
    stubFetch({ '/api/team/projects': DEMO_PROJECTS, '/api/team/overview': DEMO_OVERVIEW });
    const { router } = renderApp('/');
    expect(screen.getByTestId('scroll-restoration')).toBeTruthy();
    expect(router.state.location.pathname).toBe('/workbench');
  });

  it.each([
    ['/lessons', 'page-lessons', '经验'],
    ['/tasks/synthetic-task', 'page-task', '任务'],
    ['/members/synthetic-member', 'page-person', '成员'],
    ['/goals/synthetic-goal', 'page-goal', '目标'],
    ['/documents/synthetic-document', 'page-document', '资料'],
    ['/updates/synthetic-update', 'page-update', '一期更新'],
    ['/takeover', 'page-takeover', '接管结果'],
    ['/takeover/synthetic-result', 'page-takeover', '接管结果'],
  ])('routes %s to its readable pending page', async (path, testId, title) => {
    const { renderApp, stubFetch, DEMO_PROJECTS } = await import('./test-utils');
    stubFetch({ '/api/team/projects': DEMO_PROJECTS });
    renderApp(path);
    const page = await screen.findByTestId(testId);
    expect(page.textContent).toContain(title);
    expect(page.textContent).not.toContain('synthetic-');
  });

  it.each([
    ['overview', '概览'],
    ['plan', '工作计划'],
    ['deps', '依赖'],
    ['acceptance', '成果与验收'],
    ['knowledge', '项目知识'],
  ])('routes the project %s page', async (tab, title) => {
    const { renderApp, stubFetch, DEMO_PROJECTS } = await import('./test-utils');
    stubFetch({ '/api/team/projects': DEMO_PROJECTS });
    renderApp(`/projects/demo?tab=${tab}`);
    await screen.findByTestId('page-project');
    expect(document.querySelector('.ant-tabs-tab-active')?.textContent).toContain(title);
  });

  it.each([
    ['/home?scope=demo', '/workbench'],
    ['/knowledge?scope=demo', '/lessons'],
    ['/advanced?scope=demo', '/settings'],
  ])('preserves the scope when legacy route %s redirects', async (path, destination) => {
    const { renderApp, stubFetch, DEMO_PROJECTS } = await import('./test-utils');
    stubFetch({ '/api/team/projects': DEMO_PROJECTS });
    const { router } = renderApp(path);
    expect(router.state.location.pathname).toBe(destination);
    expect(router.state.location.search).toBe('?scope=demo');
  });
});
