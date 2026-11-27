/** Start the three-map workspace and load its own modules for offline export. */
(function (root) {
  'use strict';

  function createProjectSwitcher({ document: doc, i18n, currentKey, load, navigate }) {
    const select = doc && doc.getElementById('mapProjectSelect');
    let projects = [];
    const valid = (p) => p && typeof p.key === 'string' && /^[a-f0-9]{64}$/.test(p.key) && typeof p.name === 'string';
    const stateKey = (p) => (p.available === false ? 'hub.unavailable' : p.state === 'discovered' ? 'hub.pending' : p.state === 'attention' ? 'hub.attention' : 'map.project_connected');
    const selectedValue = () => (currentKey && projects.some((p) => p.key === currentKey) ? currentKey : '');

    function render() {
      const selected = selectedValue();
      select.textContent = '';
      const all = doc.createElement('option');
      all.value = '';
      all.textContent = i18n.t('map.project_all', { count: projects.length });
      all.selected = selected === '';
      select.appendChild(all);
      for (const project of projects) {
        const option = doc.createElement('option');
        option.value = project.key;
        option.textContent = i18n.t('map.project_option', { name: project.name, state: i18n.t(stateKey(project)) });
        if (project.available === false && project.key !== currentKey) option.disabled = true;
        option.selected = project.key === selected;
        select.appendChild(option);
      }
      select.value = selected;
      select.hidden = false;
    }

    async function refresh() {
      if (!select || !currentKey) {
        if (select) select.hidden = true;
        return false;
      }
      try {
        const res = await load();
        if (!res?.ok || !Array.isArray(res.data?.projects) || !res.data.projects.length || res.data.projects.some((p) => !valid(p))) {
          projects = [];
          select.hidden = true;
          return false;
        }
        projects = res.data.projects.slice().sort((a, b) =>
          (b.last_used_at || 0) - (a.last_used_at || 0)
          || (b.registered_at || 0) - (a.registered_at || 0)
          || ((a.root || '') < (b.root || '') ? -1 : (a.root || '') > (b.root || '') ? 1 : 0));
        render();
        return true;
      } catch (_) {
        projects = [];
        select.hidden = true;
        return false;
      }
    }

    if (select) {
      select.addEventListener('change', () => {
        const key = select.value;
        const lang = i18n.locale === 'zh-CN' ? 'zh-CN' : 'en';
        if (key === '') { navigate(`/?lang=${lang}`); return; }
        if (key === currentKey) return;
        const matched = projects.find((p) => p.key === key);
        if (!/^[a-f0-9]{64}$/.test(key) || (matched && matched.available === false)) {
          select.value = selectedValue();
          return;
        }
        const intake = matched && (matched.state || 'connected') !== 'connected' ? '&intake=1' : '';
        navigate(`/?project=${key}&lang=${lang}${intake}`);
      });
    }

    return { refresh };
  }

  if (typeof module !== 'undefined' && module.exports) { module.exports = { createProjectSwitcher }; return; }

  const i18n = root.AWR_I18N;
  const $ = (id) => root.document.getElementById(id);
  const connection = $('connectionStatus');
  const projectKey = new URL(root.location.href).searchParams.get('project');
  if (projectKey) {
    $('projectHome').hidden = false;
    $('projectHome').href = `/?lang=${encodeURIComponent(i18n.locale)}`;
  }

  async function callApi(url, options = {}) {
    try {
      const route = projectKey ? `/api/projects/${encodeURIComponent(projectKey)}${url.slice(4)}` : url;
      const response = await fetch(route, { credentials: 'same-origin', cache: 'no-store', ...options,
        headers: options.method === 'POST' ? { 'x-awr-project-progress-overview': '1' } : undefined });
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

  const projectSwitcher = createProjectSwitcher({
    document: root.document, i18n, currentKey: projectKey,
    load: async () => (await fetch('/api/projects', { credentials: 'same-origin', cache: 'no-store' })).json(),
    navigate: (href) => root.location.assign(href),
  });
  const projectMap = root.AWR_PROJECT_MAP.ui.createProjectMap({ i18n, $, callApi, fetchSource: fetchAsset });

  connection.textContent = i18n.t('progress.connecting');
  projectMap.show();
  projectSwitcher.refresh();
  root.addEventListener('pageshow', (event) => {
    if (event.persisted) {
      projectMap.refresh();
      projectSwitcher.refresh();
    }
  });
})(typeof globalThis !== 'undefined' ? globalThis : this);
