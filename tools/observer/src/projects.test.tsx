import { screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { DEMO_OVERVIEW, DEMO_PROJECTS, renderApp, stubFetch } from './test-utils';

describe('project navigation and reads', () => {
  it('encodes project keys in links without altering them', async () => {
    stubFetch({
      '/api/team/projects': {
        ...DEMO_PROJECTS,
        projects: [{ key: 'demo /?', title: 'Encoded project', role: 'reviewer' }],
      },
      '/api/team/overview': DEMO_OVERVIEW,
    });
    renderApp('/projects');
    const link = await screen.findByRole('link', { name: 'Encoded project' });
    expect(link.getAttribute('href')).toBe('/projects/demo%20%2F%3F');
  });

  it('keeps project tabs in their contractual order', async () => {
    stubFetch({ '/api/team/projects': DEMO_PROJECTS, '/api/team/overview': DEMO_OVERVIEW });
    renderApp('/projects/demo');
    await screen.findByRole('tab', { name: '概览' });
    expect(screen.getAllByRole('tab').map((tab) => tab.textContent)).toEqual([
      '概览',
      '工作计划',
      '协作依赖',
      '成果与验收',
      '项目知识',
    ]);
  });
});
