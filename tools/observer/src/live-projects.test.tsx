import { screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it } from 'vitest';
import { DEMO_OVERVIEW, renderApp, stubFetch } from './test-utils';

describe('live project discovery without display metadata', () => {
  it.each([undefined, null, '   '])('opens a project whose title is %s', async (title) => {
    const user = userEvent.setup();
    stubFetch({
      '/api/team/projects': {
        ok: true,
        projects: [{ key: 'support-workspace', title, project_id: 'support-project' }],
      },
      '/api/team/overview': { ...DEMO_OVERVIEW, project: 'support-workspace' },
    });
    const { router } = renderApp('/projects');

    const projectLink = await screen.findByRole('link', { name: 'support-workspace' });
    expect(projectLink.getAttribute('href')).toBe('/projects/support-workspace');
    await user.click(projectLink);

    await waitFor(() => expect(router.state.location.pathname).toBe('/projects/support-workspace'));
    expect(screen.getByTestId('scope-switcher').textContent).toBe('support-workspace');
    expect(document.body.textContent).not.toContain('undefined');
  });

  it('offers a key-only project in the scope menu', async () => {
    const user = userEvent.setup();
    stubFetch({
      '/api/team/projects': { ok: true, projects: [{ key: 'support-workspace' }] },
      '/api/team/overview': { ...DEMO_OVERVIEW, project: 'support-workspace' },
    });
    const { router } = renderApp('/workbench');
    await user.click(screen.getByTestId('scope-switcher'));
    await user.click(await screen.findByText('support-workspace'));

    await waitFor(() => expect(router.state.location.search).toBe('?scope=support-workspace'));
    expect(screen.getByTestId('scope-switcher').textContent).toBe('support-workspace');
  });
});
