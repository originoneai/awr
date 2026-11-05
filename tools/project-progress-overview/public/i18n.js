/** Local, dependency-free messages for the Project Progress Overview UI. Source content is never translated. */
(function (root) {
  'use strict';

  const commonJS = typeof module !== 'undefined' && module.exports;
  const catalogs = commonJS
    ? { en: require('./locales/en.js'), 'zh-CN': require('./locales/zh-CN.js') }
    : root.AWR_LOCALES;
  const storageKey = 'awr-project-progress-overview-language';
  const locales = Object.freeze(['en', 'zh-CN']);

  function supported(value) {
    if (typeof value !== 'string') return null;
    if (/^zh(?:-|$)/i.test(value)) return 'zh-CN';
    if (/^en(?:-|$)/i.test(value)) return 'en';
    return null;
  }

  function detectLocale() {
    // Explicit URL selection also works when browser storage is unavailable.
    try {
      const selected = supported(new URL(root.location.href).searchParams.get('lang'));
      if (selected) return selected;
    } catch (_) { /* Node tests may not provide a browser URL. */ }
    try {
      const saved = supported(root.localStorage.getItem(storageKey) || root.localStorage.getItem('awr-inspector-language'));
      if (saved) return saved;
    } catch (_) { /* Storage can be blocked by browser policy. */ }
    const languages = root.navigator && (root.navigator.languages || [root.navigator.language]);
    for (const language of languages || []) {
      const candidate = supported(language);
      if (candidate) return candidate;
    }
    return 'en';
  }

  let locale = commonJS ? 'en' : detectLocale();

  function t(key, params = {}) {
    const has = (catalog, name) => Object.prototype.hasOwnProperty.call(catalog || {}, name);
    const pattern = has(catalogs[locale], key) ? catalogs[locale][key]
      : has(catalogs.en, key) ? catalogs.en[key] : key;
    // A callback preserves literal replacement characters in project-supplied values.
    return pattern.replace(/\{([A-Za-z0-9_]+)\}/g, (placeholder, name) =>
      Object.prototype.hasOwnProperty.call(params, name) ? String(params[name]) : placeholder);
  }

  function setLocale(value) {
    locale = supported(value) || 'en';
    try { root.localStorage.setItem(storageKey, locale); } catch (_) { /* URL remains the fallback. */ }
    return locale;
  }

  function apply(document) {
    document.documentElement.lang = locale;
    for (const element of document.querySelectorAll('[data-i18n]')) {
      element.textContent = t(element.getAttribute('data-i18n'));
    }
    for (const attribute of ['title', 'aria-label', 'placeholder']) {
      for (const element of document.querySelectorAll(`[data-i18n-${attribute}]`)) {
        element.setAttribute(attribute, t(element.getAttribute(`data-i18n-${attribute}`)));
      }
    }
  }

  const api = { t, setLocale, detectLocale, apply, locales, get locale() { return locale; } };
  if (commonJS) module.exports = api;
  else {
    root.AWR_I18N = api;
    apply(root.document);
    const picker = root.document.getElementById('languageSelect');
    picker.value = locale;
    picker.addEventListener('change', () => {
      const selected = setLocale(picker.value);
      // Reload to rebuild map labels and demo fixtures in the selected language.
      // Keep the current view and all existing query parameters.
      const url = new URL(root.location.href);
      url.searchParams.set('lang', selected);
      root.location.assign(url.href);
    });
  }
})(typeof globalThis !== 'undefined' ? globalThis : this);
