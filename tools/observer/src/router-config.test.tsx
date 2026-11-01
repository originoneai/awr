import { screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';

// Prove the data router mounts ScrollRestoration without depending on jsdom
// scroll APIs: replace it with a marker and assert the marker renders.
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
    // Data routers expose navigate/fetchers/state; a plain object would not.
    expect(typeof router.navigate).toBe('function');
    expect(typeof router.fetch).toBe('function');
    expect(router.state.location.pathname).toBeDefined();
  });

  it('mounts ScrollRestoration in the root layout', async () => {
    const { renderApp, stubFetch, DEMO_PROJECTS, DEMO_OVERVIEW } = await import('./test-utils');
    stubFetch({ '/api/team/projects': DEMO_PROJECTS, '/api/team/overview': DEMO_OVERVIEW });
    renderApp('/workbench');
    expect(screen.getByTestId('scroll-restoration')).toBeTruthy();
  });

  it('redirects / to the workbench', async () => {
    const { renderApp, stubFetch, DEMO_PROJECTS, DEMO_OVERVIEW } = await import('./test-utils');
    stubFetch({ '/api/team/projects': DEMO_PROJECTS, '/api/team/overview': DEMO_OVERVIEW });
    const { router } = renderApp('/');
    expect(router.state.location.pathname).toBe('/workbench');
  });
});
