import i18n from 'i18next';
import LanguageDetector from 'i18next-browser-languagedetector';
import { initReactI18next } from 'react-i18next';
import enUS from './en-US';
import zhCN from './zh-CN';

export const SUPPORTED_LANGUAGES = ['zh-CN', 'en-US'] as const;
export type AppLanguage = (typeof SUPPORTED_LANGUAGES)[number];

export const resources = {
  'zh-CN': { translation: zhCN },
  'en-US': { translation: enUS },
} as const;

void i18n.use(LanguageDetector).use(initReactI18next).init({
  resources,
  fallbackLng: 'zh-CN',
  supportedLngs: SUPPORTED_LANGUAGES,
  detection: { order: ['localStorage'], caches: ['localStorage'] },
  interpolation: { escapeValue: false },
  returnNull: false,
});

export default i18n;
