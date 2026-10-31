import i18n from 'i18next';
import { initReactI18next } from 'react-i18next';
import enUS from './en-US';
import zhCN from './zh-CN';

export const SUPPORTED_LANGUAGES = ['zh-CN', 'en-US'] as const;
export type AppLanguage = (typeof SUPPORTED_LANGUAGES)[number];

export const resources = {
  'zh-CN': { translation: zhCN },
  'en-US': { translation: enUS },
} as const;

void i18n.use(initReactI18next).init({
  resources,
  lng: 'zh-CN',
  fallbackLng: 'en-US',
  interpolation: { escapeValue: false },
  returnNull: false,
});

export default i18n;
