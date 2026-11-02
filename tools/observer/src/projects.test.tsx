import { screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { defaultVisibleText } from './lib/human-language-guard';
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

describe('project list in words', () => {
  it('names a project by its title and role and keeps its key out of the default view', async () => {
    stubFetch({ '/api/team/projects': DEMO_PROJECTS, '/api/team/overview': DEMO_OVERVIEW });
    renderApp('/projects');
    await screen.findByRole('link', { name: 'Team mainline' });
    const list = screen.getByTestId('page-projects');
    expect(defaultVisibleText(list)).toBe('项目\nTeam mainline\n项目管理员\nDocs stream\n复核者');
    expect(defaultVisibleText(list)).not.toMatch(/\bdemo\b|\bdocs\b/);
  });
});
