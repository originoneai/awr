import { readFileSync } from 'node:fs';
import path from 'node:path';
import { act, render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { COMPACT_VIEWPORT_QUERY, useCompactViewport } from './viewport';

function ViewportProbe() {
  return <output data-testid="compact-probe">{useCompactViewport() ? 'compact' : 'wide'}</output>;
}

function mockMedia(initial: boolean) {
  let matches = initial;
  const listeners = new Set<() => void>();
  const media = {
    get matches() { return matches; },
    media: COMPACT_VIEWPORT_QUERY,
    onchange: null,
    addListener: (listener: () => void) => listeners.add(listener),
    removeListener: (listener: () => void) => listeners.delete(listener),
    addEventListener: (_type: string, listener: () => void) => listeners.add(listener),
    removeEventListener: (_type: string, listener: () => void) => listeners.delete(listener),
    dispatchEvent: () => false,
  };
  window.matchMedia = vi.fn().mockImplementation((query: string) => {
    if (query !== COMPACT_VIEWPORT_QUERY) throw new Error(`Unexpected media query: ${query}`);
    return media;
  });
  return {
    setMatches(next: boolean) {
      matches = next;
      for (const listener of listeners) listener();
    },
  };
}

const originalMatchMedia = window.matchMedia;

afterEach(() => {
  window.matchMedia = originalMatchMedia;
});

describe('shared compact viewport breakpoint', () => {
  it('is the prototype 860px max-width query', () => {
    expect(COMPACT_VIEWPORT_QUERY).toBe('(max-width: 860px)');
  });

  it('reports false while wide, then follows the media query across the breakpoint', () => {
    const media = mockMedia(false);
    render(<ViewportProbe />);
    expect(screen.getByTestId('compact-probe').textContent).toBe('wide');
    act(() => media.setMatches(true));
    expect(screen.getByTestId('compact-probe').textContent).toBe('compact');
    act(() => media.setMatches(false));
    expect(screen.getByTestId('compact-probe').textContent).toBe('wide');
  });

  it('stays wide when matchMedia is unavailable', () => {
    window.matchMedia = undefined as unknown as typeof window.matchMedia;
    render(<ViewportProbe />);
    expect(screen.getByTestId('compact-probe').textContent).toBe('wide');
  });

  it('pins the application shell and every display component to the one shared constant', () => {
    const read = (relative: string) => readFileSync(path.resolve(process.cwd(), relative), 'utf8');

    const shell = read('src/components/AppShell.tsx');
    expect(shell).toContain("from '../lib/viewport'");
    // The shell must not re-declare its own copy of the breakpoint.
    expect(shell).not.toContain('COMPACT_NAVIGATION_QUERY');
    expect(shell).not.toMatch(/max-width:\s*860px/);

    for (const relative of [
      'src/components/display/DataTable.tsx',
      'src/components/display/PageHeader.tsx',
      'src/components/display/DetailPanel.tsx',
      'src/components/display/KpiStrip.tsx',
      'src/components/display/CompletionBar.tsx',
      'src/components/display/Chain.tsx',
      'src/components/display/TodoRail.tsx',
      'src/components/display/catalog/ComponentCatalog.tsx',
    ]) {
      const source = read(relative);
      expect(source, relative).toContain('useCompactViewport');
      // antd's md breakpoint (768px) disagrees with the shell between 769px and 860px.
      expect(source, relative).not.toContain('useBreakpoint');
    }
  });
});
