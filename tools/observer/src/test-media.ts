// jsdom has no layout engine, so media queries never match. These helpers evaluate the width
// conditions of a query at a given viewport width so tests can render the shell at 375 or 1341
// and let the responsive code observe the breakpoint the way a real browser does.

/** All `(min|max)-width: Npx` conditions of a query, as [kind, px] pairs. */
export function widthConditions(query: string): Array<[('min' | 'max'), number]> {
  return [...query.matchAll(/\((min|max)-width:\s*([\d.]+)px\)/g)].map((match) => (
    [match[1] as 'min' | 'max', Number(match[2])] as ['min' | 'max', number]
  ));
}

/** Whether a media query matches at a viewport width. Non-width conditions count as unmatched. */
export function matchesAtWidth(query: string, viewportWidth: number): boolean {
  const conditions = widthConditions(query);
  if (conditions.length === 0) return false;
  return conditions.every(([kind, px]) => (kind === 'min' ? viewportWidth >= px : viewportWidth <= px));
}

type MediaQueryListener = (event: { matches: boolean; media: string }) => void;

/**
 * A matchMedia replacement that answers width queries for `initialWidth` and can be moved to
 * another width later (dispatching `change` to registered listeners, as a browser does on resize).
 */
export function createMatchMedia(initialWidth: number) {
  const listeners = new Map<string, Set<MediaQueryListener>>();
  let viewportWidth = initialWidth;
  const evaluate = (query: string) => matchesAtWidth(query, viewportWidth);

  const matchMedia = (query: string) => {
    let forQuery = listeners.get(query);
    if (!forQuery) {
      forQuery = new Set();
      listeners.set(query, forQuery);
    }
    const current = forQuery;
    return {
      get matches() {
        return evaluate(query);
      },
      media: query,
      onchange: null,
      addListener: (listener: MediaQueryListener) => current.add(listener),
      removeListener: (listener: MediaQueryListener) => current.delete(listener),
      addEventListener: (type: string, listener: unknown) => {
        if (type === 'change') current.add(listener as MediaQueryListener);
      },
      removeEventListener: (type: string, listener: unknown) => {
        if (type === 'change') current.delete(listener as MediaQueryListener);
      },
      dispatchEvent: () => false,
    } as unknown as MediaQueryList;
  };

  const setWidth = (nextWidth: number) => {
    viewportWidth = nextWidth;
    for (const [query, forQuery] of listeners) {
      const event = { matches: evaluate(query), media: query };
      for (const listener of forQuery) listener(event);
    }
  };

  return { matchMedia, setWidth };
}
