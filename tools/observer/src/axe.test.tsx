import { screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { axe, toHaveNoViolations } from 'jest-axe';
import { describe, expect, it } from 'vitest';
import { DEMO_PROJECTS, mockViewport, renderApp, stubFetch } from './test-utils';
import { demoOverviewModelResponse } from './test-fixtures/overview-model';

expect.extend(toHaveNoViolations);

describe('accessibility', () => {
  it('My work has no axe violations', async () => {
    stubFetch({ '/api/team/projects': DEMO_PROJECTS, '/api/team/overview-model': demoOverviewModelResponse() });
    const { container } = renderApp('/workbench?scope=demo&work=W1');
    await screen.findByTestId('workbench-frame');
    const results = await axe(container, {
      // jsdom has no layout; color-contrast cannot be evaluated meaningfully.
      rules: { 'color-contrast': { enabled: false } },
    });
    expect(results).toHaveNoViolations();
  });

  it('the mobile shell and open avatar menu have no axe violations', async () => {
    const user = userEvent.setup();
    const viewport = mockViewport(375);
    try {
      stubFetch({ '/api/team/projects': DEMO_PROJECTS, '/api/team/overview-model': demoOverviewModelResponse() });
      renderApp('/workbench?scope=demo&work=W1');
      await screen.findByTestId('workbench-frame');
      await user.click(screen.getByTestId('avatar-menu-trigger'));
      await screen.findByRole('link', { name: '账户与连接' });
      // The menu renders in a portal on body, so audit the whole document.
      const results = await axe(document.body, {
        rules: { 'color-contrast': { enabled: false } },
      });
      expect(results).toHaveNoViolations();
    } finally {
      viewport.restore();
    }
  });
});
