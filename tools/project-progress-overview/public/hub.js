/** The local project catalog, rendered without reading project files in the browser. */
(function (root) {
  'use strict';
  const commonJS = typeof module !== 'undefined' && module.exports;

  function visibleProjects(projects, query = '') {
    const needle = query.trim().toLocaleLowerCase();
    return projects.filter((project) => `${project.name}\n${project.root}`.toLocaleLowerCase().includes(needle))
      .slice().sort((a, b) => b.last_used_at - a.last_used_at || (a.root < b.root ? -1 : a.root > b.root ? 1 : 0));
  }

  function projectHref(key, locale) {
    if (!/^[a-f0-9]{64}$/.test(key)) throw new Error('invalid local project key');
    return `/?project=${key}&lang=${locale === 'zh-CN' ? 'zh-CN' : 'en'}`;
  }

  function createHub({ document: doc, i18n, load }) {
    const $ = (id) => doc.getElementById(id);
    const t = (key, params) => i18n.t(key, params);
    const state = { projects: [], running: null, error: null };

    function element(tag, className, text) {
      const node = doc.createElement(tag);
      node.className = className;
      if (text !== undefined) node.textContent = text;
      return node;
    }

    function card(project) {
      const node = element(project.available ? 'a' : 'article', 'project-card');
      node.setAttribute('data-project', project.key);
      node.setAttribute('data-available', String(project.available));
      if (project.available) node.setAttribute('href', projectHref(project.key, i18n.locale));
      else node.setAttribute('aria-disabled', 'true');
      const top = element('div', 'project-card-top');
      const mark = element('span', 'project-mark');
      mark.setAttribute('aria-hidden', 'true');
      mark.innerHTML = '<svg width="23" height="23" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5"><path d="M3.5 7V5.5A1.5 1.5 0 0 1 5 4h5l2 3h7a1.5 1.5 0 0 1 1.5 1.5v10A1.5 1.5 0 0 1 19 20H5a1.5 1.5 0 0 1-1.5-1.5V7Z" stroke-linejoin="round"/><path d="M4 9h16"/></svg>';
      top.appendChild(mark);
      if (project.available) {
        const arrow = element('span', 'project-arrow', '↗');
        arrow.setAttribute('aria-hidden', 'true');
        top.appendChild(arrow);
      }
      node.appendChild(top);
      const title = element('h2', 'project-title', project.name);
      title.setAttribute('title', project.name);
      node.appendChild(title);
      const path = element('p', 'project-path', project.root);
      path.setAttribute('title', project.root);
      node.appendChild(path);
      if (!project.available) node.appendChild(element('p', 'project-unavailable', t('hub.unavailable_hint')));
      const bottom = element('div', 'project-card-bottom');
      bottom.appendChild(element('span', 'project-state', t(project.available ? 'hub.available' : 'hub.unavailable')));
      const date = new Date(project.last_used_at);
      const time = element('time', 'project-time', new Intl.DateTimeFormat(i18n.locale, { month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit' }).format(date));
      time.setAttribute('datetime', date.toISOString());
      time.setAttribute('title', `${t('hub.last_used')} · ${date.toLocaleString(i18n.locale)}`);
      bottom.appendChild(time);
      node.appendChild(bottom);
      return node;
    }

    function message(title, description) {
      $('hubMessageTitle').textContent = title;
      $('hubMessageText').textContent = description;
      $('hubMessage').hidden = false;
    }

    function render() {
      const projects = visibleProjects(state.projects, $('hubSearch').value);
      $('hubProjects').innerHTML = '';
      $('hubMessage').hidden = true;
      $('hubCount').textContent = t('hub.count', { count: projects.length });
      for (const project of projects) $('hubProjects').appendChild(card(project));
      if (!projects.length) message(t(state.projects.length ? 'hub.no_matches' : 'hub.empty'), t(state.projects.length ? 'hub.no_matches_hint' : 'hub.empty_hint'));
    }

    function refresh() {
      if (state.running) return state.running;
      const request = (async () => {
        $('hubRefresh').disabled = true;
        $('hubStatus').textContent = t('hub.loading');
        $('hubStatus').setAttribute('data-kind', 'info');
        state.error = null;
        try {
          const response = await Promise.resolve().then(load);
          if (!response || !response.ok || !response.data || !Array.isArray(response.data.projects)) {
            throw new Error(response && response.error ? response.error.message : t('progress.invalid_response'));
          }
          const projects = response.data.projects;
          if (projects.some((p) => !p || !/^[a-f0-9]{64}$/.test(p.key) || typeof p.name !== 'string' || typeof p.root !== 'string' || typeof p.available !== 'boolean' || !Number.isFinite(p.last_used_at))) {
            throw new Error(t('progress.invalid_response'));
          }
          state.projects = projects;
          render();
          $('hubStatus').textContent = '';
        } catch (error) {
          state.projects = [];
          state.error = error;
          $('hubProjects').innerHTML = '';
          $('hubCount').textContent = '';
          $('hubMessage').hidden = true;
          $('hubStatus').setAttribute('data-kind', 'error');
          $('hubStatus').textContent = t('hub.error', { message: error.message });
        } finally {
          $('hubRefresh').disabled = false;
          state.running = null;
        }
      })();
      state.running = request;
      return request;
    }

    $('hubSearch').addEventListener('input', () => { if (!state.error) render(); });
    $('hubRefresh').addEventListener('click', refresh);
    return { refresh, render, state };
  }

  const api = { visibleProjects, projectHref, createHub };
  if (commonJS) module.exports = api;
  else {
    root.AWR_PROJECT_HUB = api;
    const hub = createHub({ document: root.document, i18n: root.AWR_I18N, load: async () => {
      const response = await fetch('/api/projects', { credentials: 'same-origin', cache: 'no-store' });
      return response.json();
    } });
    hub.refresh();
  }
})(typeof globalThis !== 'undefined' ? globalThis : this);
