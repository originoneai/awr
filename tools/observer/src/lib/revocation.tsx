import { useQueryClient } from '@tanstack/react-query';
import {
  createContext,
  useCallback,
  useContext,
  useMemo,
  useSyncExternalStore,
  type ReactNode,
} from 'react';
import {
  isObservationReadRevoked,
  resumeObservationReads,
  revokeObservationReads,
  subscribeToObservationRevocation,
} from './queries';

interface RevocationValue {
  revoked: boolean;
  revoke: () => void;
  resume: () => void;
}

const RevocationContext = createContext<RevocationValue | null>(null);

export function RevocationProvider({ children }: { children: ReactNode }) {
  const queryClient = useQueryClient();
  const subscribe = useCallback(
    (listener: () => void) => subscribeToObservationRevocation(queryClient, listener),
    [queryClient],
  );
  const revoked = useSyncExternalStore(
    subscribe,
    () => isObservationReadRevoked(queryClient),
    () => false,
  );
  const revoke = useCallback(() => revokeObservationReads(queryClient), [queryClient]);
  const resume = useCallback(() => resumeObservationReads(queryClient), [queryClient]);
  const value = useMemo(() => ({ revoked, revoke, resume }), [revoked, revoke, resume]);
  return <RevocationContext.Provider value={value}>{children}</RevocationContext.Provider>;
}

export function useRevocation() {
  const value = useContext(RevocationContext);
  if (!value) throw new Error('useRevocation must be used inside RevocationProvider');
  return value;
}
