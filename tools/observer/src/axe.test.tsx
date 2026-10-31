import { screen } from '@testing-library/react';
import { axe, toHaveNoViolations } from 'jest-axe';
import { describe, expect, it } from 'vitest';
import { DEMO_PROJECTS, DEMO_WORK_NEXT, renderApp, stubFetch } from './test-utils';

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
});
