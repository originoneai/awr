import 'i18next';
import type zhCN from './zh-CN';

// Typed translation keys: t('nav.workbench') etc. are checked at compile time.
declare module 'i18next' {
  interface CustomTypeOptions {
    defaultNS: 'translation';
    resources: {
      translation: typeof zhCN;
    };
    returnNull: false;
  }
}
