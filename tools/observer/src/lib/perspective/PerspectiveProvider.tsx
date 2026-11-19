import { createContext, useCallback, useContext, useEffect, useMemo, type ReactNode } from 'react';
import { usePerspectiveSelection } from './usePerspectiveSelection';
import type { PerspectiveFacts, PerspectiveId, PerspectiveLimitation } from './types';

interface PerspectiveContextValue {
  selected: PerspectiveId;
  available: PerspectiveId[];
  limitations: PerspectiveLimitation[];
  select: (next: PerspectiveId) => void;
}

const PerspectiveContext = createContext<PerspectiveContextValue | null>(null);

interface PerspectiveProviderProps {
  facts: PerspectiveFacts;
  /** The URL value is a request, never an authorization fact. */
  requestedPerspective?: string | null;
  /** Reflects an accepted selection into the URL without writing to a service. */
  onChange?: (next: PerspectiveId) => void;
  children: ReactNode;
}

export function PerspectiveProvider({
  facts,
  requestedPerspective = null,
  onChange,
  children,
}: PerspectiveProviderProps) {
  const { available, limitations, selected, select: persistSelection } = usePerspectiveSelection(facts, requestedPerspective);

  const select = useCallback((next: PerspectiveId) => {
    if (persistSelection(next)) onChange?.(next);
  }, [onChange, persistSelection]);

  // The safe fallback is already visible in this render; clean the stale request afterwards.
  useEffect(() => {
    if (requestedPerspective !== null && requestedPerspective !== selected) onChange?.(selected);
  }, [onChange, requestedPerspective, selected]);

  const value = useMemo(() => ({ selected, available, limitations, select }), [available, limitations, selected, select]);
  return <PerspectiveContext.Provider value={value}>{children}</PerspectiveContext.Provider>;
}

export function usePerspective(): PerspectiveContextValue {
  const value = useContext(PerspectiveContext);
  if (!value) throw new Error('usePerspective must be used within PerspectiveProvider');
  return value;
}
