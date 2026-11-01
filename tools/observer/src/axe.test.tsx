import { axe, toHaveNoViolations } from 'jest-axe';
import { describe, expect, it } from 'vitest';
import { DEMO_OVERVIEW, DEMO_PROJECTS, renderApp, stubFetch } from './test-utils';

expect.extend(toHaveNoViolations);

describe('accessibility', () => {
  it('AppShell has no axe violations', async () => {
    stubFetch({ '/api/team/projects': DEMO_PROJECTS, '/api/team/overview': DEMO_OVERVIEW });
    const { container } = renderApp('/workbench');
    const results = await axe(container, {
      // jsdom has no layout; color-contrast cannot be evaluated meaningfully.
      rules: { 'color-contrast': { enabled: false } },
    });
    expect(results).toHaveNoViolations();
  });
});
