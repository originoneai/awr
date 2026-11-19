import { ConfigProvider } from 'antd';
import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useLayoutEffect,
  useMemo,
  useState,
  type ReactNode,
} from 'react';
import { applyCraftTheme, createThemeConfig, type CraftThemeMode } from '../theme';

export const THEME_STORAGE_KEY = 'awr-observer-theme';
const SYSTEM_THEME_QUERY = '(prefers-color-scheme: dark)';
type ThemePreference = CraftThemeMode | 'system';

interface CraftThemeContextValue {
  mode: CraftThemeMode;
  toggle: () => void;
}

const CraftThemeContext = createContext<CraftThemeContextValue | null>(null);

function readPreference(): ThemePreference {
  if (typeof window === 'undefined') return 'system';
  try {
    const saved = window.localStorage.getItem(THEME_STORAGE_KEY);
    return saved === 'light' || saved === 'dark' ? saved : 'system';
  } catch {
    return 'system';
  }
}

function readSystemTheme(): CraftThemeMode {
  if (typeof window === 'undefined' || typeof window.matchMedia !== 'function') return 'light';
  return window.matchMedia(SYSTEM_THEME_QUERY).matches ? 'dark' : 'light';
}

export function useCraftTheme() {
  const value = useContext(CraftThemeContext);
  if (!value) throw new Error('useCraftTheme must be used within CraftThemeProvider');
  return value;
}

export function CraftThemeProvider({ children }: { children: ReactNode }) {
  const [preference, setPreference] = useState<ThemePreference>(readPreference);
  const [systemTheme, setSystemTheme] = useState<CraftThemeMode>(readSystemTheme);
  const mode = preference === 'system' ? systemTheme : preference;

  useEffect(() => {
    if (typeof window.matchMedia !== 'function') return;
    const media = window.matchMedia(SYSTEM_THEME_QUERY);
    const update = () => setSystemTheme(media.matches ? 'dark' : 'light');
    update();
    if (typeof media.addEventListener === 'function') {
      media.addEventListener('change', update);
      return () => media.removeEventListener('change', update);
    }
    media.addListener(update);
    return () => media.removeListener(update);
  }, []);

  // The root variables and antd algorithm commit together, before the paint of a toggle.
  useLayoutEffect(() => {
    applyCraftTheme(document.documentElement, mode);
  }, [mode]);

  const toggle = useCallback(() => {
    const next = mode === 'dark' ? 'light' : 'dark';
    setPreference(next);
    try {
      window.localStorage.setItem(THEME_STORAGE_KEY, next);
    } catch {
      // The in-memory preference still works when storage is blocked or full.
    }
  }, [mode]);

  const context = useMemo(() => ({ mode, toggle }), [mode, toggle]);
  const config = useMemo(() => createThemeConfig(mode), [mode]);

  return (
    <CraftThemeContext.Provider value={context}>
      <ConfigProvider theme={config}>
        {children}
      </ConfigProvider>
    </CraftThemeContext.Provider>
  );
}
