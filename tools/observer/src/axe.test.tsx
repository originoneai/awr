import { screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { axe, toHaveNoViolations } from 'jest-axe';
import { describe, expect, it } from 'vitest';
import { DEMO_PROJECTS, DEMO_WORK_NEXT, mockViewport, renderApp, stubFetch } from './test-utils';

expect.extend(toHaveNoViolations);

describe('accessibility', () => {
  it('My work has no axe violations', async () => {
    stubFetch({ '/api/team/projects': DEMO_PROJECTS, '/api/team/read': DEMO_WORK_NEXT });
    const { container } = renderApp('/workbench?scope=demo&work=W1');
    await screen.findByRole('link', { name: 'Build the filter API' });
    const results = await axe(container, {
      // jsdom has no layout; color-contrast cannot be evaluated meaningfully.
      rules: { 'color-contrast': { enabled: false } },
    });
    expect(results).toHaveNoViolations();
  });

  it('the mobile navigation drawer has no axe violations when open', async () => {
    const user = userEvent.setup();
    const viewport = mockViewport(375);
    try {
      stubFetch({ '/api/team/projects': DEMO_PROJECTS, '/api/team/read': DEMO_WORK_NEXT });
      renderApp('/workbench?scope=demo&work=W1');
      await screen.findByRole('link', { name: 'Build the filter API' });
      await user.click(screen.getByRole('button', { name: '打开导航' }));
      await screen.findByRole('dialog');
      // The drawer renders in a portal on body, so audit the whole document.
      const results = await axe(document.body, {
        rules: { 'color-contrast': { enabled: false } },
      });
      expect(results).toHaveNoViolations();
    } finally {
      viewport.restore();
    }
  });
});
