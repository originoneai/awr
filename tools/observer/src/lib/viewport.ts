import { useEffect, useState } from 'react';

/**
 * The prototype has one compact breakpoint (max-width: 860px): the shell collapses its
 * navigation and the display components tighten their insets. The application shell and
 * the shared display components must both consume this constant so the two can never
 * disagree between 769px and 860px. Pinned by lib/viewport.test.tsx.
 */
export const COMPACT_VIEWPORT_QUERY = '(max-width: 860px)';

export function useMediaQuery(query: string): boolean {
  const [matches, setMatches] = useState(() => (
    typeof window !== 'undefined' && typeof window.matchMedia === 'function'
      ? window.matchMedia(query).matches
      : false
  ));

  useEffect(() => {
    if (typeof window.matchMedia !== 'function') return;
    const media = window.matchMedia(query);
    const update = () => setMatches(media.matches);
    update();
    if (typeof media.addEventListener === 'function') {
      media.addEventListener('change', update);
      return () => media.removeEventListener('change', update);
    }
    media.addListener(update);
    return () => media.removeListener(update);
  }, [query]);

  return matches;
}

/** True below the single shared compact breakpoint; false on the desktop layout. */
export function useCompactViewport(): boolean {
  return useMediaQuery(COMPACT_VIEWPORT_QUERY);
}
