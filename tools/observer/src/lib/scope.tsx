import { createContext, useCallback, useContext, useMemo, type ReactNode } from 'react';
import { matchPath, useLocation, useNavigate, useSearchParams } from 'react-router';

interface ScopeValue {
  projectKey: string | null;
  setProjectKey: (key: string | null) => void;
}

const ScopeContext = createContext<ScopeValue>({ projectKey: null, setProjectKey: () => {} });

export function ScopeProvider({ children }: { children: ReactNode }) {
  const location = useLocation();
  const navigate = useNavigate();
  const [searchParams, setSearchParams] = useSearchParams();
  const projectMatch = matchPath({ path: '/projects/:project', end: true }, location.pathname);
  const routeProject = projectMatch?.params.project ?? null;
  const projectKey = routeProject ?? searchParams.get('scope');

  const setProjectKey = useCallback((key: string | null) => {
    const nextParams = new URLSearchParams(searchParams);
    if (key) nextParams.set('scope', key);
    else nextParams.delete('scope');
    if (projectMatch) {
      navigate({
        pathname: key ? `/projects/${encodeURIComponent(key)}` : '/projects',
        search: nextParams.toString(),
      });
    } else {
      setSearchParams(nextParams);
    }
  }, [navigate, projectMatch, searchParams, setSearchParams]);

  const value = useMemo(() => ({ projectKey, setProjectKey }), [projectKey, setProjectKey]);
  return <ScopeContext.Provider value={value}>{children}</ScopeContext.Provider>;
}

export function useScope() {
  return useContext(ScopeContext);
}
