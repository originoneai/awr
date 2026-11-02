import { cleanup } from '@testing-library/react';
import { configure } from '@testing-library/dom';
import { afterEach, vi } from 'vitest';
import { createMatchMedia } from './test-media';

// CI runners can take over 1s to hydrate the heaviest acceptance evidence panel.
configure({ asyncUtilTimeout: 5000 });

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

// jsdom lacks APIs antd 6 relies on.
if (typeof window !== 'undefined') {
  // jsdom's matchMedia never matches (no layout engine), which would force every responsive
  // component onto its narrow layout. Tests default to a desktop viewport; a test that needs a
  // narrow one installs its own via mockViewport (test-utils).
  const desktop = createMatchMedia(1440);
  window.matchMedia = desktop.matchMedia as unknown as typeof window.matchMedia;
  if (!window.ResizeObserver) {
    class ResizeObserverStub {
      observe() {}
      unobserve() {}
      disconnect() {}
    }
    window.ResizeObserver = ResizeObserverStub as unknown as typeof ResizeObserver;
  }
}
