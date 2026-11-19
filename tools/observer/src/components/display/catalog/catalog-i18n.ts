import i18next from 'i18next';
import { initReactI18next, useTranslation } from 'react-i18next';
import enUS from '../../../i18n/en-US';
import zhCN from '../../../i18n/zh-CN';
import { catalogMessages } from './catalog-messages';

/**
 * A development-only i18next instance for the catalog pages. It layers the catalog-only
 * copy over the production vocabulary so components resolve their product words (status
 * labels, legends, hover meanings) from the same place the real pages will, while the
 * catalog's own example copy stays out of src/i18n and out of production builds.
 */
function withCatalogOverlay<T extends { display: Record<string, unknown> }>(base: T, overlay: unknown): T {
  return { ...base, display: { ...base.display, catalog: overlay } };
}

function detectedLanguage(): string {
  try {
    return localStorage.getItem('i18nextLng') ?? 'zh-CN';
  } catch {
    return 'zh-CN';
  }
}

export const catalogI18n = i18next.createInstance();

/** Dev-only translate with widened keys; see useCatalogTranslation for why. */
export function catalogT(key: string, options?: Record<string, unknown>): string {
  const translate = catalogI18n.t as (key: string, options?: Record<string, unknown>) => string;
  return translate(key, options);
}

void catalogI18n.use(initReactI18next).init({
  resources: {
    'zh-CN': { translation: withCatalogOverlay(zhCN, catalogMessages['zh-CN']) },
    'en-US': { translation: withCatalogOverlay(enUS, catalogMessages['en-US']) },
  },
  lng: detectedLanguage(),
  fallbackLng: 'zh-CN',
  supportedLngs: ['zh-CN', 'en-US'],
  interpolation: { escapeValue: false },
  react: { useSuspense: false },
});

/**
 * The catalog hook: product words and example copy resolve against the dev instance.
 * The production resource type deliberately does not know the catalog subtree, so this
 * dev-only hook widens the key to a plain string; production code keeps its strict keys.
 */
export interface CatalogTranslation {
  t: (key: string, options?: Record<string, unknown>) => string;
}

export function useCatalogTranslation(): CatalogTranslation {
  return useTranslation(undefined, { i18n: catalogI18n }) as unknown as CatalogTranslation;
}
