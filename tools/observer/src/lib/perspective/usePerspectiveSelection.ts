import { useCallback, useMemo, useState } from 'react';
import { derivePerspectives, resolvePerspective } from './rules';
import { isPerspectiveId, type PerspectiveFacts, type PerspectiveId } from './types';

export const PERSPECTIVE_STORAGE_KEY = 'awr-observer-perspective';

export function readRememberedPerspective(storage?: Storage): PerspectiveId | null {
  if (typeof window === 'undefined' && storage === undefined) return null;
  try {
    const value = (storage ?? window.localStorage).getItem(PERSPECTIVE_STORAGE_KEY);
    return isPerspectiveId(value) ? value : null;
  } catch {
    return null;
  }
}

export function writeRememberedPerspective(value: PerspectiveId, storage?: Storage): boolean {
  if (typeof window === 'undefined' && storage === undefined) return false;
  try {
    (storage ?? window.localStorage).setItem(PERSPECTIVE_STORAGE_KEY, value);
    return true;
  } catch {
    return false;
  }
}

export function usePerspectiveSelection(facts: PerspectiveFacts, requested: string | null) {
  const derivation = useMemo(() => derivePerspectives(facts), [facts]);
  const [remembered, setRemembered] = useState<PerspectiveId | null>(() => readRememberedPerspective());
  const selected = resolvePerspective(facts, requested, remembered);

  const select = useCallback((next: PerspectiveId): boolean => {
    // This second check keeps programmatic calls from persisting a height the facts do not allow.
    if (!derivation.available.includes(next)) return false;
    setRemembered(next);
    writeRememberedPerspective(next);
    return true;
  }, [derivation.available]);

  return { ...derivation, selected, select };
}
