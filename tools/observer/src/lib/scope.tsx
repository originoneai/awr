import { createContext, useContext, useMemo, useState, type ReactNode } from 'react';

interface ScopeValue {
  /** Selected project key, or null when the org-level placeholder is active. */
  projectKey: string | null;
  setProjectKey: (key: string | null) => void;
}

const ScopeContext = createContext<ScopeValue>({ projectKey: null, setProjectKey: () => {} });

export function ScopeProvider({ children }: { children: ReactNode }) {
  const [projectKey, setProjectKey] = useState<string | null>(null);
  const value = useMemo(() => ({ projectKey, setProjectKey }), [projectKey]);
  return <ScopeContext.Provider value={value}>{children}</ScopeContext.Provider>;
}

export function useScope() {
  return useContext(ScopeContext);
}
