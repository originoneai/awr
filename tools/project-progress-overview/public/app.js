/** Start the three-map workspace and load its own modules for offline export. */
(function (root) {
  'use strict';
  const i18n = root.AWR_I18N;
  const $ = (id) => root.document.getElementById(id);
  const connection = $('connectionStatus');

  async function callApi(url) {
    try {
      const response = await fetch(url, { credentials: 'same-origin' });
      const result = await response.json();
      if (!result || typeof result.ok !== 'boolean') throw new Error(i18n.t('progress.invalid_response'));
      if (url === '/api/map/config') {
        const demo = result.error && result.error.code === 'DemoMode';
        if (!demo && !result.ok) throw new Error(result.error ? result.error.message : i18n.t('progress.invalid_response'));
        connection.textContent = i18n.t(demo ? 'progress.demo' : 'progress.connected');
        connection.dataset.kind = demo ? 'demo' : 'live';
      }
      return result;
    } catch (error) {
      if (url === '/api/map/config') {
        connection.textContent = i18n.t('progress.connection_failed', { message: error.message });
        connection.dataset.kind = 'error';
      }
      throw error;
    }
  }

  async function fetchAsset(url) {
    const response = await fetch(url, { credentials: 'same-origin' });
    if (!response.ok) throw new Error(i18n.t('progress.asset_failed'));
    return response.text();
  }

  const projectMap = root.AWR_PROJECT_MAP.ui.createProjectMap({ i18n, $, callApi, fetchSource: fetchAsset });

  connection.textContent = i18n.t('progress.connecting');
  projectMap.show();
})(typeof globalThis !== 'undefined' ? globalThis : this);
