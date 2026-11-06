/** The local project catalog, rendered without reading project files in the browser. */
(function (root) {
  'use strict';
  const commonJS = typeof module !== 'undefined' && module.exports;

  function visibleProjects(projects, query = '') {
    const needle = query.trim().toLocaleLowerCase();
    return projects.filter((project) => `${project.name}\n${project.root}`.toLocaleLowerCase().includes(needle))
      .slice().sort((a, b) => b.last_used_at - a.last_used_at || (b.registered_at || 0) - (a.registered_at || 0) || (a.root < b.root ? -1 : a.root > b.root ? 1 : 0));
  }

  function projectHref(key, locale, state = 'connected') {
    if (!/^[a-f0-9]{64}$/.test(key)) throw new Error('invalid local project key');
    return `/?project=${key}&lang=${locale === 'zh-CN' ? 'zh-CN' : 'en'}${state === 'connected' ? '' : '&intake=1'}`;
  }

  function validProject(project) {
    return project && /^[a-f0-9]{64}$/.test(project.key) && typeof project.name === 'string' && typeof project.root === 'string'
      && typeof project.available === 'boolean' && Number.isFinite(project.last_used_at)
      && (!project.state || ['connected', 'discovered', 'attention'].includes(project.state));
  }

  function createHub({ document: doc, i18n, load, selectedKey = null, loadIntake, copy, enroll, remove, rescan }) {
    const $ = (id) => doc.getElementById(id);
    const t = (key, params) => i18n.t(key, params);
    const state = { projects: [], running: null, removing: new Set(), enrolling: false, error: null, discovery: null, scope: null, signature: null, coverageSignature: null };

    function element(tag, className, text) {
      const node = doc.createElement(tag);
      node.className = className;
      if (text !== undefined) node.textContent = text;
      return node;
    }

    function card(project) {
      const node = element('article', 'project-card');
      node.setAttribute('data-project', project.key);
      node.setAttribute('data-available', String(project.available));
      node.setAttribute('data-state', project.state || 'connected');
      const content = element(project.available ? 'a' : 'div', project.available ? 'project-card-open' : 'project-card-content');
      if (project.available) content.setAttribute('href', projectHref(project.key, i18n.locale, project.state));
      else content.setAttribute('aria-disabled', 'true');
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
      content.appendChild(top);
      const title = element('h2', 'project-title', project.name);
      title.setAttribute('title', project.name);
      content.appendChild(title);
      const path = element('p', 'project-path', project.root);
      path.setAttribute('title', project.root);
      content.appendChild(path);
      if (project.state === 'discovered') content.appendChild(element('p', 'project-intake-hint', t('hub.intake_card_hint')));
      if (!project.available) content.appendChild(element('p', 'project-unavailable', t('hub.unavailable_hint')));
      const bottom = element('div', 'project-card-bottom');
      const label = !project.available ? 'hub.unavailable' : project.state === 'discovered' ? 'hub.pending' : project.state === 'attention' ? 'hub.attention' : 'hub.available';
      bottom.appendChild(element('span', 'project-state', t(label)));
      const date = new Date(project.last_used_at || project.registered_at || 0);
      const formatted = new Intl.DateTimeFormat(i18n.locale, { month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit' }).format(date);
      const manuallyAdded = project.state === 'discovered' && project.intake_requested;
      const time = element('time', 'project-time', project.last_used_at ? formatted : t(manuallyAdded ? 'hub.added_time' : 'hub.discovered_time', { time: formatted }));
      time.setAttribute('datetime', date.toISOString());
      time.setAttribute('title', `${t(project.last_used_at ? 'hub.last_used' : manuallyAdded ? 'hub.first_added' : 'hub.first_seen')} · ${date.toLocaleString(i18n.locale)}`);
      bottom.appendChild(time);
      content.appendChild(bottom);
      node.appendChild(content);
      if (remove) {
        const button = element('button', 'project-delete', t('hub.remove'));
        button.type = 'button';
        button.setAttribute('aria-label', t('hub.remove_label', { name: project.name }));
        button.setAttribute('title', t('hub.remove_hint'));
        button.disabled = state.removing.has(project.key);
        button.addEventListener('click', async () => {
          if (state.removing.has(project.key)) return;
          state.removing.add(project.key);
          button.disabled = true;
          $('hubRefresh').disabled = true;
          $('hubEnroll').disabled = true;
          try {
            if (state.running) await state.running;
            const response = await remove(project.key);
            if (!response?.ok) throw new Error('project removal failed');
            state.projects = state.projects.filter((entry) => entry.key !== project.key);
            state.signature = null;
            render();
            $('hubStatus').textContent = '';
          } catch (_) {
            $('hubStatus').setAttribute('data-kind', 'error');
            $('hubStatus').textContent = t('hub.remove_failed');
          } finally {
            state.removing.delete(project.key);
            const current = $('hubProjects').querySelector(`[data-project="${project.key}"] .project-delete`);
            if (current) current.disabled = false;
            $('hubRefresh').disabled = Boolean(state.running) || state.removing.size > 0;
            $('hubEnroll').disabled = state.removing.size > 0;
          }
        });
        node.appendChild(button);
      }
      return node;
    }

    function renderCoverage() {
      const scope = $('hubScope');
      if (scope) { scope.hidden = state.scope !== 'isolated'; scope.textContent = t('hub.isolated_scope'); }
      const target = $('hubCoverage');
      const report = state.discovery;
      if (!target) return;
      if (!report) { target.hidden = true; state.coverageSignature = null; return; }
      target.hidden = false;
      const counts = Object.fromEntries(['connected', 'discovered', 'attention'].map((phase) => [phase, state.projects.filter((project) => (project.state || 'connected') === phase).length]));
      const signature = JSON.stringify([report, counts]);
      if (signature === state.coverageSignature) return;
      state.coverageSignature = signature;
      const open = target.querySelector('details')?.hasAttribute('open');
      target.innerHTML = '';
      const phase = ['pending', 'scanning', 'complete', 'partial', 'interrupted', 'error'].includes(report.state) ? report.state : 'error';
      const heading = element('div', 'hub-coverage-heading');
      heading.appendChild(element('h2', '', t('hub.discovery_title')));
      heading.appendChild(element('span', `hub-discovery-state ${phase}`, t(`hub.discovery_${phase}`)));
      target.appendChild(heading);
      target.appendChild(element('p', 'hub-coverage-summary', t('hub.discovery_summary', { directories: report.directories_scanned || 0, connected: counts.connected, pending: counts.discovered, attention: counts.attention })));
      const details = element('details', 'hub-coverage-details');
      if (open) details.setAttribute('open', '');
      details.appendChild(element('summary', '', t('hub.discovery_scope')));
      const roots = element('ul', 'hub-coverage-roots');
      for (const path of report.roots || []) roots.appendChild(element('li', '', path));
      details.appendChild(roots);
      if (!report.roots?.length) details.appendChild(element('p', '', t('hub.discovery_no_roots')));
      details.appendChild(element('p', '', t('hub.discovery_exclusions', { count: report.excluded_directories || 0, symlinks: report.symlinks_skipped || 0 })));
      details.appendChild(element('p', 'hub-exclusion-patterns', (report.exclude_names || []).join(' · ')));
      details.appendChild(element('p', '', t('hub.discovery_boundaries', { caches: report.dependency_caches_skipped || 0 })));
      details.appendChild(element('p', '', t('hub.discovery_temporary', { count: report.temporary_directories_skipped || 0 })));
      if (report.completed_at) details.appendChild(element('p', '', t('hub.discovery_completed', { time: new Date(report.completed_at).toLocaleString(i18n.locale) })));
      const issues = element('ul', 'hub-coverage-issues');
      for (const issue of report.issues || []) issues.appendChild(element('li', '', `${issue.path} — ${issue.reason}`));
      if (report.message) issues.appendChild(element('li', '', report.message));
      if (report.omitted_issues) issues.appendChild(element('li', '', t('hub.discovery_more_issues', { count: report.omitted_issues })));
      details.appendChild(issues);
      target.appendChild(details);
    }

    async function renderIntake() {
      const panel = $('hubIntake');
      if (!selectedKey || !panel || !loadIntake) return;
      if (!/^[a-f0-9]{64}$/.test(selectedKey)) throw new Error(t('progress.invalid_response'));
      const response = await loadIntake(selectedKey);
      if (!response?.ok || response.data?.project?.key !== selectedKey) throw new Error(response?.error?.message || t('progress.invalid_response'));
      const project = response.data.project;
      panel.hidden = false;
      $('hubProjects').hidden = true;
      $('hubListHeading').hidden = true;
      $('hubControls').hidden = true;
      $('hubIntakeBack').setAttribute('href', `/?lang=${i18n.locale}`);
      $('hubIntakeTitle').textContent = project.name;
      $('hubIntakePath').textContent = project.root;
      const connected = project.state === 'connected';
      $('hubIntakeState').textContent = t(connected ? 'hub.connected' : project.state === 'attention' ? 'hub.attention' : 'hub.pending');
      $('hubIntakeDescription').textContent = t(connected ? 'hub.intake_connected' : project.state === 'attention' ? 'hub.intake_repair' : 'hub.intake_description');
      $('hubIntakePrompt').textContent = t('hub.intake_prompt', { root: project.root });
      $('hubIntakeOpen').hidden = !connected;
      $('hubIntakeOpen').setAttribute('href', projectHref(selectedKey, i18n.locale));
      $('hubIntakeDiagnostic').hidden = !project.issue;
      $('hubIntakeIssue').textContent = project.issue || '';
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
      renderCoverage();
      if (!projects.length) message(t(state.projects.length ? 'hub.no_matches' : 'hub.empty'), t(state.projects.length ? 'hub.no_matches_hint' : 'hub.empty_hint'));
    }

    function refresh({ quiet = false, scan = false } = {}) {
      if (state.running) return state.running;
      if (state.removing.size) return Promise.resolve();
      const request = (async () => {
        $('hubRefresh').disabled = true;
        $('hubStatus').textContent = t('hub.loading');
        $('hubStatus').setAttribute('data-kind', 'info');
        state.error = null;
        try {
          const response = await Promise.resolve().then(scan && rescan ? rescan : load);
          if (!response || !response.ok || !response.data || !Array.isArray(response.data.projects)) {
            throw new Error(response && response.error ? response.error.message : t('progress.invalid_response'));
          }
          const projects = response.data.projects;
          if (projects.some((p) => !validProject(p))) {
            throw new Error(t('progress.invalid_response'));
          }
          state.projects = projects;
          state.discovery = response.data.discovery || null;
          state.scope = response.data.catalog_scope || null;
          const signature = JSON.stringify(projects.map(({ key, root, name, available, state, last_used_at, registered_at, intake_requested }) => ({ key, root, name, available, state, last_used_at, registered_at, intake_requested })));
          if (!quiet || state.signature !== signature) render();
          else renderCoverage();
          state.signature = signature;
          await renderIntake();
          $('hubStatus').textContent = '';
        } catch (error) {
          state.projects = [];
          state.signature = null;
          state.error = error;
          if ($('hubIntake')) $('hubIntake').hidden = true;
          if ($('hubCoverage')) $('hubCoverage').hidden = true;
          $('hubProjects').innerHTML = '';
          $('hubCount').textContent = '';
          $('hubMessage').hidden = true;
          $('hubStatus').setAttribute('data-kind', 'error');
          $('hubStatus').textContent = t('hub.error', { message: error.message });
        } finally {
          $('hubRefresh').disabled = state.removing.size > 0;
          state.running = null;
        }
      })();
      state.running = request;
      return request;
    }

    $('hubSearch').addEventListener('input', () => { if (!state.error) render(); });
    $('hubRefresh').addEventListener('click', () => refresh({ scan: true }));
    $('hubIntakeRefresh')?.addEventListener('click', () => refresh());
    const dialog = $('hubEnrollDialog');
    if (dialog && enroll) {
      $('hubEnroll').addEventListener('click', () => {
        $('hubEnrollError').hidden = true;
        dialog.showModal();
        $('hubEnrollPath').focus();
      });
      $('hubEnrollCancel').addEventListener('click', () => dialog.close());
      $('hubEnrollForm').addEventListener('submit', async (event) => {
        event.preventDefault();
        if (state.enrolling) return;
        state.enrolling = true;
        $('hubEnrollSubmit').disabled = true;
        $('hubEnrollError').hidden = true;
        const path = $('hubEnrollPath').value.trim();
        try {
          if (!path) throw new Error('missing project path');
          const response = await enroll(path);
          if (!response?.ok || !validProject(response.data?.project)) throw new Error('invalid enrollment response');
          $('hubSearch').value = '';
          await refresh();
          if (!state.projects.some((project) => project.key === response.data.project.key)) state.projects.push(response.data.project);
          state.signature = null;
          render();
          dialog.close();
          $('hubEnrollPath').value = '';
        } catch (_) {
          $('hubEnrollError').textContent = t('hub.enroll_failed');
          $('hubEnrollError').hidden = false;
        } finally {
          state.enrolling = false;
          $('hubEnrollSubmit').disabled = false;
        }
      });
    }
    const copyButton = $('hubIntakeCopy');
    if (copyButton && copy) copyButton.addEventListener('click', async () => {
      try { await copy($('hubIntakePrompt').textContent); copyButton.textContent = t('hub.intake_copied'); }
      catch (_) { copyButton.textContent = t('hub.intake_copy_failed'); }
    });
    return { refresh, render, state };
  }

  const api = { visibleProjects, projectHref, createHub };
  if (commonJS) module.exports = api;
  else {
    root.AWR_PROJECT_HUB = api;
    const selectedKey = new URL(root.location.href).searchParams.get('intake') === '1' ? new URL(root.location.href).searchParams.get('project') : null;
    const hub = createHub({ document: root.document, i18n: root.AWR_I18N, selectedKey,
      copy: (text) => root.navigator.clipboard.writeText(text),
      enroll: async (path) => (await fetch('/api/projects/intake', { method: 'POST', credentials: 'same-origin', headers: { 'Content-Type': 'application/json', 'X-AWR-Project-Hub': '1' }, body: JSON.stringify({ root: path }) })).json(),
      remove: async (key) => (await fetch(`/api/projects/${encodeURIComponent(key)}`, { method: 'DELETE', credentials: 'same-origin', headers: { 'X-AWR-Project-Hub': '1' } })).json(),
      rescan: async () => (await fetch('/api/projects/refresh', { method: 'POST', credentials: 'same-origin', headers: { 'X-AWR-Project-Hub': '1' } })).json(),
      loadIntake: async (key) => (await fetch(`/api/projects/${encodeURIComponent(key)}/intake`, { credentials: 'same-origin', cache: 'no-store' })).json(),
      load: async () => {
      const response = await fetch('/api/projects', { credentials: 'same-origin', cache: 'no-store' });
      return response.json();
    } });
    hub.refresh();
  }
})(typeof globalThis !== 'undefined' ? globalThis : this);
