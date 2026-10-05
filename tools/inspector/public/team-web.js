/**
 * Team Web collaboration loop (WS-044).
 *
 * Interactive My Projects → overview / card|list / detail surface with
 * collaboration writes. Talks to the Inspector bridge `/api/team/*`, which
 * either serves fixtures (demo) or proxies the designed awr-server `/v1/web`
 * cookie entry. Bearer tokens never stay in page JS after login handoff.
 */
(function (root) {
  'use strict';

  const GUARD = 'X-AWR-Inspector';
  const PROGRESS_REFRESH_MS = 5000;
  const DETAIL_TIMEOUT_MS = 5000;

  function el(tag, attrs, text) {
    const node = document.createElement(tag);
    if (attrs) {
      for (const [k, v] of Object.entries(attrs)) {
        if (k === 'class') node.className = v;
        else if (k === 'dataset') {
          for (const [dk, dv] of Object.entries(v)) node.dataset[dk] = dv;
        } else if (v != null) node.setAttribute(k, v);
      }
    }
    if (text != null) node.textContent = text;
    return node;
  }

  function t(i18n, key, vars) {
    try {
      return i18n.t(key, vars);
    } catch (_) {
      return key;
    }
  }

  function createTeamWeb(opts) {
    const i18n = opts.i18n;
    const $ = opts.$;
    const network = root.AWR_TEAM_NETWORK || (typeof require === 'function' ? require('./team-network') : null);
    const state = {
      viewMode: 'team', // personal | team
      layout: 'cards', // collaboration graph | list
      projectKey: null,
      projects: [],
      works: [],
      streams: [],
      graphLoading: false,
      selected: null,
      session: null,
      disconnect: false,
      inflight: Object.create(null),
      lastReceipts: Object.create(null),
      members: [],
      raw: null,
      error: null,
      loading: false,
      detailLoading: false,
      detailResponse: null,
      connectOpen: false,
      handoffOpen: Object.create(null),
      lastRefreshedAt: null,
      refreshing: false,
    };
    let generation = 0;
    let detailGeneration = 0;
    let loginForm = null;
    let loginInput = null;
    let net = null;
    let pendingDetails = new Map();
    const detailReadOrder = new WeakMap();
    let detailSequence = 0;
    let refreshTimer = null;
    const adminModule = root.AWR_TEAM_ADMIN || (typeof require === 'function' ? require('./team-admin') : null);
    const admin = adminModule && adminModule.createTeamAdmin({ $, i18n, api, onAuthError: failed });

    function clearProjectData() {
      if (admin) admin.reset();
      state.projects = [];
      state.projectKey = null;
      state.works = [];
      state.streams = [];
      state.graphLoading = false;
      pendingDetails = new Map();
      state.members = [];
      state.selected = null;
      state.raw = null;
      state.detailLoading = false;
      state.detailResponse = null;
      state.lastReceipts = Object.create(null);
      state.connectOpen = false;
      state.handoffOpen = Object.create(null);
      state.lastRefreshedAt = null;
    }

    function failed(body) {
      state.error = (body && body.error) || { code: 'InvalidResponse', message: 'Invalid Team response' };
      state.disconnect = state.error.code === 'BridgeUnreachable';
      if (['Unauthenticated', 'SessionExpired'].includes(state.error.code)) {
        ++generation;
        ++detailGeneration;
        state.session = null;
        clearProjectData();
        render();
      } else if (state.error.code === 'Forbidden') {
        ++generation;
        clearProjectData();
        render();
      }
      return body;
    }

    async function api(path, options) {
      const opts2 = Object.assign({ credentials: 'same-origin' }, options || {});
      opts2.headers = Object.assign(
        { 'content-type': 'application/json', [GUARD]: '1' },
        opts2.headers || {}
      );
      try {
        const res = await fetch(path, opts2);
        const body = await res.json();
        return res.ok === false && !body.error
          ? { ok: false, error: { code: body.code || 'RequestFailed', message: body.message || `HTTP ${res.status}` } }
          : body;
      } catch (err) {
        return {
          ok: false,
          error: { code: 'BridgeUnreachable', message: String(err && err.message) },
        };
      }
    }

    function renderProjects(host) {
      clear(host);
      host.appendChild(el('h3', null, t(i18n, 'ui.my_projects')));
      if (!state.projects.length) {
        host.appendChild(el('p', { class: 'sub' }, t(i18n, state.loading ? 'ui.team_loading' : 'ui.no_projects_yet')));
        return;
      }
      const picker = el('select', { id: 'teamProjectSelect', 'aria-label': t(i18n, 'ui.my_projects') });
      for (const p of state.projects) {
        const option = el('option', { value: p.key }, p.title || p.key);
        option.value = p.key;
        picker.appendChild(option);
      }
      picker.value = state.projectKey;
      picker.addEventListener('change', () => {
        state.projectKey = picker.value;
        try { root.sessionStorage.setItem('awr-team-project', state.projectKey); } catch (_) { /* Storage is optional. */ }
        state.selected = null;
        refresh();
      });
      host.appendChild(picker);
    }

    function participant(w) {
      return w.owner_person || (w.claimant ? t(i18n, 'feedback.claimed_by', { name: w.claimant })
        : w.last_participant ? t(i18n, 'feedback.last_participant', { name: w.last_participant })
          : t(i18n, isLive() && !w.detail_loaded ? 'ui.network_unread' : 'ui.network_owner_unknown'));
    }

    function nextStep(w) {
      const key = 'feedback.guidance_' + w.guidance?.code;
      const translated = t(i18n, key);
      return w.guidance ? (translated === key ? t(i18n, 'feedback.guidance_unknown') : translated) : w.next_step;
    }

    function missing(w, field, fallback) {
      const reason = w.missing?.[field], key = 'feedback.missing_' + reason;
      const translated = t(i18n, key);
      return reason && translated !== key ? translated : t(i18n, fallback || 'ui.network_not_reported');
    }

    const time = value => Number.isFinite(value) ? new Date(value).toLocaleString() : '—';

    // The website task-node structure; project data remains text, never HTML.
    function workCard(w, lane) {
      const card = el('article', {
        class: 'task-node', dataset: { key: w.key, status: network.visualStatus(w) },
        tabindex: '0', role: 'button', 'aria-label': w.title || w.key,
        'aria-pressed': String(state.selected === w.key), 'aria-controls': 'teamDetail',
      });
      card.appendChild(el('span', { class: 'node-status', 'aria-hidden': 'true' }));
      const body = el('span', { class: 'node-body' });
      const top = el('span', { class: 'node-top' });
      top.appendChild(el('b', { class: 'node-id' }, w.key));
      top.appendChild(el('span', { class: 'task-state' }, workStatus(w)));
      body.appendChild(top);
      body.appendChild(el('strong', null, w.title || w.key));
      const people = el('span', { class: 'node-people' });
      people.appendChild(el('span', { class: 'node-owner' }, participant(w)));
      const agent = w.agent && typeof w.agent === 'object' ? w.agent.id : w.agent;
      if (agent) people.appendChild(el('span', { class: 'node-agent' }, [agent, w.model].filter(Boolean).join(' · ')));
      if (lane) people.appendChild(el('span', { class: 'node-lane' }, lane.name));
      body.appendChild(people);
      if (w.dependency_export_unavailable) body.appendChild(el('span', { class: 'node-note' }, t(i18n, 'ui.network_dependency_gap')));
      if (w.detail_error) body.appendChild(el('span', { class: 'node-note' }, t(i18n, 'ui.team_detail_read_failed_short')));
      card.appendChild(body);
      card.addEventListener('click', () => selectWork(w.key));
      card.addEventListener('keydown', (event) => {
        if (event.key === 'Enter' || event.key === ' ') { event.preventDefault(); selectWork(w.key); }
      });
      return card;
    }

    function workRow(w) {
      const tr = el('tr', {
        class: state.selected === w.key ? 'selected' : '',
        dataset: { key: w.key },
      });
      const blocker =
        w.blocker && typeof w.blocker === 'object' ? w.blocker.summary : w.blocker;
      const agent =
        w.agent && typeof w.agent === 'object' ? w.agent.id || '' : w.agent || '';
      for (const text of [
        w.key,
        w.title,
        participant(w),
        agent,
        w.outcome || '',
        blocker || '',
        nextStep(w) || '',
      ]) {
        const td = el('td');
        if (tr.children.length === 0) {
          const select = el('button', { class: 'linkish', type: 'button' }, text);
          select.addEventListener('click', (event) => { event.stopPropagation(); return selectWork(w.key); });
          td.appendChild(select);
        } else td.textContent = text || '—';
        tr.appendChild(td);
      }
      tr.addEventListener('click', () => selectWork(w.key));
      return tr;
    }

    function renderOverview(host) {
      clear(host);
      const header = el('header', { class: 'network-header' });
      header.appendChild(el('h2', null, t(i18n, 'ui.network_title')));
      const toolbar = el('div', { class: 'team-toolbar' });
      const personalBtn = el(
        'button',
        { class: 'btn' + (state.viewMode === 'personal' ? ' primary' : ''), type: 'button' },
        t(i18n, 'ui.personal_view')
      );
      const teamBtn = el(
        'button',
        { class: 'btn' + (state.viewMode === 'team' ? ' primary' : ''), type: 'button' },
        t(i18n, 'ui.team_view')
      );
      if (isLive()) {
        personalBtn.disabled = true;
        personalBtn.title = t(i18n, 'ui.team_personal_unavailable');
      }
      personalBtn.addEventListener('click', () => {
        state.viewMode = 'personal';
        refresh();
      });
      teamBtn.addEventListener('click', () => {
        state.viewMode = 'team';
        refresh();
      });
      const cardsBtn = el(
        'button',
        { class: 'btn' + (state.layout === 'cards' ? ' primary' : ''), type: 'button' },
        t(i18n, 'ui.network_graph_view')
      );
      const listBtn = el(
        'button',
        { class: 'btn' + (state.layout === 'list' ? ' primary' : ''), type: 'button' },
        t(i18n, 'ui.list_layout')
      );
      cardsBtn.setAttribute('aria-pressed', String(state.layout === 'cards'));
      listBtn.setAttribute('aria-pressed', String(state.layout === 'list'));
      cardsBtn.addEventListener('click', () => {
        state.layout = 'cards';
        render();
      });
      listBtn.addEventListener('click', () => {
        state.layout = 'list';
        render();
      });
      if (state.raw && !isLive()) {
        const modes = el('div', { class: 'team-segment' });
        modes.appendChild(personalBtn); modes.appendChild(teamBtn);
        toolbar.appendChild(modes);
      }
      if (state.works.length || state.streams.length) {
        const layouts = el('div', { class: 'team-segment' });
        layouts.appendChild(cardsBtn); layouts.appendChild(listBtn);
        toolbar.appendChild(layouts);
      }
      header.appendChild(toolbar);
      host.appendChild(header);
      if (state.lastRefreshedAt && isLive()) host.appendChild(el('p', { class: 'sub', role: 'status' },
        t(i18n, state.error ? 'ui.team_progress_refresh_failed' : 'ui.team_progress_refreshed',
          { time: new Date(state.lastRefreshedAt).toLocaleTimeString(), seconds: PROGRESS_REFRESH_MS / 1000 })));
      if (isLive() && state.works.some(w => w.detail_error)) host.appendChild(el('p', { class: 'team-error', role: 'status' },
        t(i18n, 'ui.team_partial_details')));

      if (state.disconnect) {
        const banner = el('div', { class: 'banner' });
        banner.appendChild(el('span', null, t(i18n, 'ui.reconnect_needed')));
        const re = el('button', { class: 'btn', type: 'button' }, t(i18n, 'ui.reconnect'));
        re.addEventListener('click', () => refresh());
        banner.appendChild(re);
        host.appendChild(banner);
      }

      if (state.raw && !isLive()) host.appendChild(el('p', { class: 'network-demo-note' }, t(i18n, 'ui.network_demo')));
      if (!state.works.length && !state.streams.length) {
        host.appendChild(el('p', { class: 'network-empty', role: 'status' },
          t(i18n, state.loading ? 'ui.team_loading' : state.projectKey ? 'ui.network_empty' : 'ui.network_no_projects')));
        return;
      }
      net = network.model(state.works, state.streams, t(i18n, 'ui.network_unassigned'));
      if (isLive()) {
        const loaded = state.works.filter(w => w.detail_loaded).length;
        const coverage = el('div', { class: 'network-coverage', role: 'status' });
        coverage.appendChild(el('span', null, t(i18n, 'ui.network_coverage', { loaded, total: state.works.length })));
        if (state.graphLoading) coverage.appendChild(el('span', null, t(i18n, 'ui.team_detail_loading')));
        else if (loaded < state.works.length) {
          const load = el('button', { class: 'linkish', type: 'button' }, t(i18n, 'ui.network_load_details'));
          load.addEventListener('click', () => loadGraphDetails());
          coverage.appendChild(load);
        }
        if (state.graphLoading || loaded < state.works.length) host.appendChild(coverage);
      }
      if (state.layout === 'cards') {
        network.render(host, net, { el, card: workCard, count: state.works.length,
          project: state.projects.find(p => p.key === state.projectKey) || { key: state.projectKey },
          text: (key, vars) => t(i18n, 'ui.network_' + key, vars) });
      } else {
        const table = el('table', { class: 'team-table' });
        const thead = el('thead');
        const hr = el('tr');
        for (const h of [
          'key',
          'title',
          'owner',
          'agent',
          'outcome',
          'blocker',
          'next',
        ]) {
          hr.appendChild(el('th', null, t(i18n, 'ui.team_column_' + h)));
        }
        thead.appendChild(hr);
        table.appendChild(thead);
        const tbody = el('tbody');
        for (const w of state.works) tbody.appendChild(workRow(w));
        table.appendChild(tbody);
        const wrapper = el('div', { class: 'team-table-wrap' });
        wrapper.appendChild(table);
        host.appendChild(wrapper);
      }
    }

    function selectedWork() {
      return state.works.find((w) => w.key === state.selected) || null;
    }

    function isLive() { return state.raw && state.raw.interaction_mode === 'mcp'; }

    function workStatus(w) {
      if (w.attention) return t(i18n, 'ui.team_progress_' + w.attention);
      const known = ['planned', 'unclaimed', 'claimed', 'in_progress', 'running', 'blocked', 'waiting', 'in_review', 'review', 'completed', 'accepted', 'cancelled'];
      if (w.status && w.status !== 'unknown') return known.includes(w.status) ? t(i18n, 'ui.network_status_' + w.status) : w.status;
      return t(i18n, w.detail_loaded ? 'ui.team_no_runtime' : 'ui.network_unread');
    }

    async function readWork(work) {
      if (work.detail_loaded) return { ok: true, work };
      const current = generation, project = state.projectKey;
      const identity = JSON.stringify([current, project, work.workstream_id, work.key, work.contract_hash || null]);
      const params = new URLSearchParams({ project: state.projectKey, work: work.key, workstream: work.workstream_id });
      if (work.contract_hash) params.set('contract', work.contract_hash);
      let pending = pendingDetails.get(identity);
      if (!pending) {
        const response = api('/api/team/work?' + params, {
          signal: typeof AbortSignal !== 'undefined' && typeof AbortSignal.timeout === 'function'
            ? AbortSignal.timeout(DETAIL_TIMEOUT_MS) : undefined,
        }).finally(() => {
          if (pendingDetails.get(identity) === pending) pendingDetails.delete(identity);
        });
        pending = { response, order: ++detailSequence };
        pendingDetails.set(identity, pending);
      }
      const body = await pending.response;
      if (current !== generation || project !== state.projectKey) return null;
      if (!body?.ok || !body.work || body.work.key !== work.key || body.work.workstream_id !== work.workstream_id
        || (work.contract_hash && body.work.contract_hash !== work.contract_hash)) {
        // Authentication and authorization still invalidate protected data.
        // A local read failure must not block fresh reports for other tasks.
        if (['Unauthenticated', 'SessionExpired', 'Forbidden'].includes(body?.error?.code)) failed(body);
        else work.detail_error = { code: body?.error?.code || 'InvalidResponse' };
        return null;
      }
      // A poll and foreground selection may await the same read with separate
      // snapshot objects. Hydrate each caller's object, not only the first one.
      Object.assign(work, body.work);
      detailReadOrder.set(work, pending.order);
      delete work.detail_error;
      return body;
    }

    async function loadGraphDetails() {
      if (!isLive() || state.graphLoading) return;
      const current = generation;
      const queue = state.works.filter(w => !w.detail_loaded).slice(0, 60);
      const selected = selectedWork();
      if (selected && !selected.detail_loaded) {
        const position = queue.indexOf(selected);
        if (position >= 0) queue.splice(position, 1);
        queue.unshift(selected);
      }
      state.graphLoading = true;
      render();
      // Bound concurrent reads and total work per batch; large projects stay navigable.
      let index = 0;
      await Promise.all(Array.from({ length: Math.min(4, queue.length) }, async () => {
        while (current === generation && index < queue.length) {
          await readWork(queue[index++]);
          if (current === generation && !$('teamWorkspaceGrid')?.hidden && !editing() && !state.detailLoading) render();
        }
      }));
      if (current !== generation) return;
      state.graphLoading = false;
      render();
    }

    async function selectWork(key) {
      const request = ++detailGeneration;
      state.selected = key;
      state.detailResponse = null;
      const work = selectedWork();
      if (!work || !isLive()) { render(); return; }
      const current = generation;
      state.detailLoading = !work.detail_loaded;
      state.error = null;
      render();
      const body = await readWork(work);
      if (current !== generation || request !== detailGeneration || state.selected !== key) return;
      state.detailLoading = false;
      if (body) state.detailResponse = body;
      render();
    }

    function renderBlockerDetail(host, w) {
      host.appendChild(el('h3', null, t(i18n, 'ui.blocker_detail')));
      const b = w.blocker;
      if (!b) {
        host.appendChild(el('p', { class: 'sub' }, t(i18n, 'ui.no_blocker')));
        return;
      }
      if (typeof b === 'string') {
        host.appendChild(el('p', null, b));
        return;
      }
      host.appendChild(
        el('p', null, t(i18n, 'ui.prerequisite_outcome_p0', { p0: b.prerequisite_outcome || '—' }))
      );
      host.appendChild(
        el('p', null, t(i18n, 'ui.release_condition_p0', { p0: b.release_condition || '—' }))
      );
      host.appendChild(
        el('p', null, t(i18n, 'ui.check_basis_p0', { p0: b.check_basis || '—' }))
      );
      const deps = el('ul', { class: 'loops' });
      for (const d of w.depends_on || []) {
        if (!d.visible) continue;
        const li = el('li');
        const link = el('button', { class: 'linkish', type: 'button' }, d.key);
        link.addEventListener('click', () => {
          state.selected = d.key;
          render();
        });
        li.appendChild(link);
        li.appendChild(
          document.createTextNode(
            ` [${d.status || ''}] ${d.prerequisite_outcome || ''} → ${d.release_condition || ''}`
          )
        );
        deps.appendChild(li);
      }
      host.appendChild(el('h4', null, t(i18n, 'ui.visible_dependencies')));
      host.appendChild(deps);
      for (const h of w.hidden_deps || []) {
        host.appendChild(
          el('p', { class: 'sub' }, t(i18n, 'ui.hidden_dep_hint_p0', { p0: h.hint || '' }))
        );
      }
      const detail = el('details');
      detail.appendChild(el('summary', null, t(i18n, 'ui.backend_detail_layer')));
      detail.appendChild(
        el('pre', { class: 'raw' }, JSON.stringify({
          backend_code: b.backend_code || null,
          raw_receipt_ref: b.raw_receipt_ref || null,
        }, null, 2))
      );
      host.appendChild(detail);
    }

    function guardDouble(action, fn) {
      return async () => {
        if (state.inflight[action]) return;
        state.inflight[action] = true;
        try {
          await fn();
        } finally {
          state.inflight[action] = false;
          render();
        }
      };
    }

    async function runAction(action, extra) {
      const w = selectedWork();
      if (!w) return;
      const current = generation;
      const requestId = `${action}:${w.key}:${Date.now()}`;
      const body = await api('/api/team/action', {
        method: 'POST',
        body: JSON.stringify({
          project: state.projectKey,
          work_key: w.key,
          action,
          request_id: requestId,
          expected_receipt: state.lastReceipts[action + ':' + w.key] || null,
          ...extra,
        }),
      });
      if (current !== generation) return body;
      if (!body || !body.ok) return failed(body);
      if (body && body.ok && body.receipt) {
        state.lastReceipts[action + ':' + w.key] = body.receipt.id || body.receipt.request_id;
        // Exact replay returns the same receipt id.
        if (body.replayed) {
          /* idempotent */
        }
      }
      await refresh();
      return body;
    }

    function mcpUrl() {
      return state.raw && state.raw.mcp_url || '/v1/projects/' + encodeURIComponent(state.projectKey) + '/mcp';
    }
    function continuation(w) {
      return t(i18n, 'ui.team_task_prompt', { url: mcpUrl(), work: w.key, stream: w.workstream_id });
    }
    function copyBlock(host, text, label) {
      host.appendChild(el('pre', { class: 'team-connect-code' }, text));
      const button = el('button', { class: 'btn', type: 'button' }, t(i18n, label));
      const status = el('span', { class: 'sub', role: 'status' });
      button.addEventListener('click', async () => {
        try { await root.navigator.clipboard.writeText(text); status.textContent = t(i18n, 'ui.team_copied'); }
        catch (_) { status.textContent = t(i18n, 'ui.team_copy_manual'); }
      });
      host.appendChild(button); host.appendChild(status);
    }
    function renderConnect(host) {
      const details = el('details', { class: 'team-connect' });
      details.open = state.connectOpen;
      details.addEventListener('toggle', () => { state.connectOpen = details.open; });
      details.appendChild(el('summary', null, t(i18n, 'ui.team_connect_agent')));
      details.appendChild(el('p', { class: 'sub' }, t(i18n, 'ui.team_connect_help')));
      const manual = el('details');
      manual.appendChild(el('summary', null, t(i18n, 'ui.team_manual_setup')));
      manual.appendChild(el('p', { class: 'sub' }, t(i18n, 'ui.team_connect_other')));
      copyBlock(manual, 'Transport: Streamable HTTP\nURL: ' + mcpUrl() +
        '\nAuthentication: Bearer\nAuthorization: Bearer <PERSONAL_ACCESS_CREDENTIAL>', 'ui.team_copy_connection');
      copyBlock(manual, t(i18n, 'ui.team_project_prompt', { url: mcpUrl() }), 'ui.team_copy_project_prompt');
      details.appendChild(manual);
      host.appendChild(details);
    }

    function renderActions(host, w) {
      if (isLive()) {
        host.appendChild(el('h3', null, t(i18n, 'ui.team_agent_workflow')));
        host.appendChild(el('p', { class: 'sub' }, t(i18n, 'ui.team_agent_workflow_help')));
        const handoff = el('details', { class: 'team-connect' });
        const handoffKey = JSON.stringify([state.projectKey, w.key]);
        handoff.open = Boolean(state.handoffOpen[handoffKey]);
        handoff.addEventListener('toggle', () => { state.handoffOpen[handoffKey] = handoff.open; });
        handoff.appendChild(el('summary', null, t(i18n, 'ui.team_agent_handoff')));
        copyBlock(handoff, continuation(w), 'ui.team_copy_handoff');
        host.appendChild(handoff);
        return;
      }
      host.appendChild(el('h3', null, t(i18n, 'ui.collaboration_actions')));
      const caps = w.capabilities || {};
      host.appendChild(
        el(
          'p',
          { class: 'sub' },
          t(i18n, 'ui.run_pause_visibility_p0', {
            p0: `run=${Boolean(caps.run)} pause=${Boolean(caps.pause)}`,
          })
        )
      );
      const actions = [
        ['accept_responsibility', 'ui.accept_responsibility'],
        ['select_agent', 'ui.select_authorized_agent'],
        ['respond_blocker', 'ui.respond_to_blocker'],
        ['handoff_receive', 'ui.receive_handoff'],
        ['submit_review', 'ui.submit_review'],
        ['rework', 'ui.rework'],
        ['accept', 'ui.accept_work'],
      ];
      const bar = el('div', { class: 'team-actions' });
      for (const [action, label] of actions) {
        const btn = el('button', { class: 'btn', type: 'button' }, t(i18n, label));
        const payload =
          action === 'select_agent'
            ? { agent_id: (state.members[0] && state.members[0].agents[0]) || 'coding-agent' }
            : {};
        btn.addEventListener(
          'click',
          guardDouble(action + ':' + w.key, () => runAction(action, payload))
        );
        // Second listener proves double-click is ignored while inflight.
        btn.addEventListener('dblclick', (e) => e.preventDefault());
        bar.appendChild(btn);
      }
      host.appendChild(bar);
      const receipt = state.lastReceipts;
      host.appendChild(
        el('pre', { class: 'raw' }, JSON.stringify(receipt, null, 2))
      );
    }

    function renderDetail(host) {
      clear(host);
      const w = selectedWork();
      if (!w) {
        host.appendChild(el('p', { class: 'sub' }, t(i18n, 'ui.select_a_card_or_row')));
        return;
      }
      const header = el('div', { class: 'task-detail-heading' });
      const lane = state.streams.find(s => s.id === w.workstream_id);
      header.appendChild(el('span', null, [w.key, lane && (lane.title || lane.external_key)].filter(Boolean).join(' · ')));
      header.appendChild(el('h4', null, w.title || w.key));
      header.appendChild(el('em', { class: 'detail-state', dataset: { status: network.visualStatus(w) } }, workStatus(w)));
      if (w.description) header.appendChild(el('p', null, w.description));
      host.appendChild(header);
      renderActions(host, w);
      if (isLive() && !w.detail_loaded) {
        host.appendChild(el('p', { class: w.detail_error ? 'team-error' : 'sub', role: 'status' },
          t(i18n, state.detailLoading ? 'ui.team_detail_loading'
            : w.detail_error ? 'ui.team_detail_read_failed' : 'ui.team_detail_unread')));
        if (!state.detailLoading) {
          const retry = el('button', { class: 'btn', type: 'button' }, t(i18n, 'ui.team_detail_retry'));
          retry.addEventListener('click', () => w.detail_error?.code === 'SourceChanged'
            ? refreshProgress() : selectWork(w.key));
          host.appendChild(retry);
        }
        return;
      }
      const unknown = t(i18n, 'ui.network_not_reported');
      const section = (title, rows, target = host) => {
        const group = el('section', { class: 'detail-section' });
        group.appendChild(el('h5', null, t(i18n, 'ui.network_' + title)));
        const list = el('dl');
        for (const [key, value] of rows) {
          const row = el('div'); row.appendChild(el('dt', null, t(i18n, 'ui.network_' + key)));
          row.appendChild(el('dd', null, value == null || value === '' ? unknown : value)); list.appendChild(row);
        }
        group.appendChild(list); target.appendChild(group);
        return group;
      };
      const agent = w.agent && typeof w.agent === 'object' ? w.agent.id : w.agent;
      section('people', [['developer', w.owner_person || t(i18n, 'ui.team_progress_unassigned')],
        ['claimant', w.claimant || t(i18n, 'feedback.no_current_claim')], ['last_participant', w.last_participant],
        ['agent', agent || missing(w, 'model')],
        ['model', w.model || missing(w, 'model', 'ui.team_progress_model_missing')]]);
      section('progress', [['status', workStatus(w)], ['next', nextStep(w)]]);
      const report = w.progress_report;
      if (report) {
        const group = section('agent_report', [['phase', t(i18n, 'feedback.phase_' + report.phase)],
          ['summary', report.summary], ['reported_at', time(report.reported_at_unix_ms)]]);
        group.appendChild(el('p', { class: 'sub' }, t(i18n, 'feedback.caller_report')));
        if (report.stale) group.appendChild(el('p', { class: 'team-error' }, t(i18n, 'feedback.stale')));
        for (const [label, values] of [
          ['completed', report.completed], ['blockers', report.blockers],
          ['tests', (report.tests || []).map(r => [r.name, t(i18n, 'feedback.test_' + r.outcome), r.reference].filter(Boolean).join(' · '))],
          ['artifacts', (report.artifacts || []).map(r => r.label + ' · ' + r.reference)],
        ]) {
          if (!values?.length) continue;
          group.appendChild(el('h6', null, t(i18n, 'feedback.' + label)));
          const items = el('ul');
          for (const value of values) items.appendChild(el('li', null, value));
          group.appendChild(items);
        }
      } else section('agent_report', [['summary', missing(w, 'progress')]]);
      if (w.usage) {
        const usage = w.usage;
        const group = section('usage', [['input_tokens', usage.input_tokens], ['output_tokens', usage.output_tokens],
          ['cached_input_tokens', usage.cached_input_tokens], ['usage_coverage', t(i18n, 'feedback.coverage_' + usage.coverage)],
          ['measured_at', time(usage.observed_at_unix_ms)], ['reported_at', time(usage.reported_at_unix_ms)]]);
        group.appendChild(el('p', { class: 'sub' }, t(i18n, 'feedback.usage_scope')));
        if (usage.stale) group.appendChild(el('p', { class: 'team-error' }, t(i18n, 'feedback.stale')));
      } else section('usage', [['tokens', missing(w, 'usage', 'ui.team_progress_usage_missing')]]);
      section('related', [['pr', w.pr_reference?.url],
        ['ci', w.github ? w.pr_reference?.expected_head && w.github.head_sha
          && w.pr_reference.expected_head !== w.github.head_sha ? t(i18n, 'feedback.other_revision')
          : w.github.pending && !w.github.ci ? t(i18n, 'feedback.repository_pending')
            : t(i18n, 'ui.team_progress_ci_' + (w.github.ci || 'unavailable')) : null]]);
      if (w.pr_reference) {
        host.appendChild(el('a', { href: w.pr_reference.url, target: '_blank', rel: 'noopener noreferrer', class: 'linkish' }, t(i18n, 'ui.team_progress_open_pr')));
        host.appendChild(el('p', { class: 'sub' }, t(i18n, 'ui.team_progress_pr_' + w.pr_reference.registration)));
      }
      if (w.github && !w.github.unavailable) {
        if (w.github.head_sha && Number.isFinite(w.github.observed_at_ms)) host.appendChild(el('p', { class: 'sub' }, t(i18n, 'ui.team_progress_github_basis', {
          head: w.github.head_sha.slice(0, 8), time: new Date(w.github.observed_at_ms).toLocaleString(),
        })));
        if (w.github.pending) host.appendChild(el('p', { class: 'sub' }, t(i18n, 'feedback.repository_pending')));
        if (w.pr_reference?.expected_head && w.github.head_sha && w.pr_reference.expected_head !== w.github.head_sha)
          host.appendChild(el('p', { class: 'team-error' }, t(i18n, 'ui.team_progress_head_changed')));
      }
      if (w.execution) {
        const execution = w.execution;
        const dimension = key => t(i18n, execution[key] == null ? 'feedback.dimension_unknown'
          : 'feedback.' + key + (execution[key] ? '_yes' : '_no'));
        const known = (prefix, value) => {
          const key = 'feedback.' + prefix + value, translated = t(i18n, key);
          return translated === key ? t(i18n, 'ui.network_not_reported') : translated;
        };
        const group = section('execution', [['execution_state', t(i18n, 'ui.team_progress_execution_' + execution.state)],
          ['terminal_reported', dimension('terminal_reported')], ['artifact_verified', dimension('artifact_verified')],
          ['effects_settled', dimension('effects_settled')], ['settlement_basis', known('settlement_', execution.settlement_basis)],
          ['settlement_scope', known('scope_', execution.settlement_scope)],
          ['recovery', known('recovery_', execution.recovery_cause)],
          ['previous_epoch', execution.previous_epoch_review_required == null ? t(i18n, 'feedback.dimension_unknown')
            : t(i18n, execution.previous_epoch_review_required ? 'feedback.epoch_review_required' : 'feedback.epoch_review_not_required')]]);
        group.appendChild(el('p', { class: 'sub' }, t(i18n, 'feedback.execution_dimensions')));
      }
      if (w.checkpoint) {
        const history = el('details', { class: 'feedback-history' });
        history.appendChild(el('summary', null, t(i18n, 'feedback.handoff_history')));
        history.appendChild(el('p', { class: 'sub' }, time(w.checkpoint.created_at_unix_ms)));
        if (!w.checkpoint.contract_matches_current)
          history.appendChild(el('p', { class: 'team-error' }, t(i18n, 'ui.team_progress_checkpoint_stale')));
        history.appendChild(el('p', null, w.checkpoint.next_action));
        const loops = el('ul');
        for (const text of w.checkpoint.open_loops || []) loops.appendChild(el('li', null, text));
        history.appendChild(loops); host.appendChild(history);
      } else if (w.observation_available) host.appendChild(el('p', { class: 'sub' }, t(i18n, 'ui.team_progress_no_checkpoint')));
      if (w.execution?.report) {
        host.appendChild(el('h3', null, t(i18n, 'ui.team_progress_report')));
        host.appendChild(el('p', { class: 'sub' }, t(i18n, 'ui.team_progress_report_' + w.execution.report.kind)));
        host.appendChild(el('p', null, w.execution.report.note));
      } else if (w.execution?.receipt_missing) section('execution_report', [['summary', missing(w, 'execution_receipt')]]);
      const provenance = el('details', { class: 'feedback-history' });
      provenance.appendChild(el('summary', null, t(i18n, 'feedback.provenance')));
      section('provenance', [['client', w.client_id], ['session', w.session_id],
        ['delegated_agent', w.delegated_agent], ['client_version', w.client_info?.version],
        ['model_source', w.model_info?.source ? t(i18n, 'feedback.' + w.model_info.source) : null],
        ['reported_at', time(w.reporting?.client_reported_at_unix_ms)],
        ['recorded_revision', w.progress_report?.recorded_project_revision],
        ['reported_contract', w.progress_report?.reported_contract_hash],
        ['report_checkpoint', w.progress_report?.checkpoint_id],
        ['client_observed_at', time(w.progress_report?.client_observed_at_unix_ms)],
        ['usage_recorded_revision', w.usage?.recorded_project_revision],
        ['query_revision', w.snapshot?.project_revision],
        ['queried_at', time(w.snapshot?.queried_at_unix_ms ?? w.observed_at_ms)],
        ['usage_source', w.usage ? [w.usage.source, w.usage.source_ref, w.usage.counter_id].join(' · ') : null]], provenance);
      if (w.snapshot?.consistency === 'unconfirmed_legacy')
        provenance.appendChild(el('p', { class: 'team-error' }, t(i18n, 'feedback.legacy_consistency')));
      if (w.reporting?.client_stale) provenance.appendChild(el('p', { class: 'sub' }, t(i18n, 'feedback.stale')));
      host.appendChild(provenance);
      if (w.observation_available === false) host.appendChild(el('p', { class: 'team-error' }, t(i18n, 'ui.team_progress_unavailable')));
      if (isLive()) {
        if (state.detailLoading) {
          host.appendChild(el('p', { role: 'status' }, t(i18n, 'ui.team_detail_loading')));
          return;
        }
        host.appendChild(el('h3', null, t(i18n, 'ui.acceptance_criteria')));
        const acceptance = el('ul', { class: 'loops' });
        for (const criterion of w.acceptance || []) acceptance.appendChild(el('li', null, criterion));
        host.appendChild(acceptance);
        host.appendChild(el('h3', null, t(i18n, 'ui.visible_dependencies')));
        const dependencies = el('ul', { class: 'loops' });
        for (const dependency of w.depends_on || []) {
          if (dependency.visible !== true) continue;
          const item = el('li');
          const target = state.works.find((other) => other.key === dependency.key);
          const link = el(target ? 'button' : 'span', target ? { class: 'linkish', type: 'button' } : null, dependency.key);
          if (target) link.addEventListener('click', () => selectWork(dependency.key));
          item.appendChild(link);
          dependencies.appendChild(item);
        }
        host.appendChild(dependencies);
        if (w.dependency_export_unavailable) host.appendChild(el('p', { class: 'team-error' }, t(i18n, 'ui.team_dependency_unavailable')));
        if (w.recovery_blocked) host.appendChild(el('p', { class: 'team-error' }, t(i18n, 'ui.team_progress_recovery_required')));
        host.appendChild(el('p', { class: 'sub' }, t(i18n, 'ui.team_admission_not_evaluated')));
        if (!w.context_complete) host.appendChild(el('p', { class: 'sub' }, (w.completeness_reasons || []).join(' · ')));
        return;
      }
      renderBlockerDetail(host, w);
      host.appendChild(el('h3', null, t(i18n, 'ui.dependency_graph')));
      const graph = el('ul', { class: 'loops' });
      for (const d of w.depends_on || []) {
        if (!d.visible) continue;
        graph.appendChild(
          el('li', null, `${d.key} → ${w.key} · ${d.prerequisite_outcome || ''}`)
        );
      }
      host.appendChild(graph);
    }

    function renderAuth(host) {
      const refocusInput = loginInput && document.activeElement === loginInput;
      clear(host);
      if (state.session && state.session.session_id) {
        if (loginInput) loginInput.value = '';
        loginForm = null;
        loginInput = null;
        host.appendChild(
          el('p', null, t(i18n, 'ui.team_signed_in'))
        );
        const logout = el('button', { class: 'btn', type: 'button' }, t(i18n, 'ui.logout'));
        logout.addEventListener(
          'click', guardDouble('logout', () => signOut('/api/team/logout', {}))
        );
        const revoke = el('button', { class: 'btn', type: 'button' }, t(i18n, 'ui.revoke_session'));
        revoke.addEventListener(
          'click',
          guardDouble('revoke', () => signOut('/api/team/session/revoke', { all_mine: true }))
        );
        host.appendChild(logout);
        host.appendChild(revoke);
        if (isLive() && state.projectKey) renderConnect(host);
      } else {
        host.appendChild(el('label', { class: 'team-login-label', for: 'teamBearerInput' }, t(i18n, 'ui.team_access_token')));
        // A pending anonymous refresh must not discard a credential being typed.
        // Keep the form nodes, never copy the credential into application state.
        if (!loginForm) {
          const form = el('div', { class: 'team-login' });
          const input = el('input', {
            type: 'password',
            id: 'teamBearerInput',
            autocomplete: 'off',
            'aria-label': t(i18n, 'ui.team_access_token'),
            placeholder: 'awr1.…',
          });
          const btn = el('button', { class: 'btn primary', type: 'button' }, t(i18n, 'ui.web_login'));
          btn.addEventListener(
            'click',
            guardDouble('login', async () => {
              const current = ++generation;
              clearProjectData();
              state.error = null;
              const bearer = input.value;
              input.value = ''; // never retain bearer in the DOM after submit
              const body = await api('/api/team/login', {
                method: 'POST',
                body: JSON.stringify({ bearer }),
              });
              if (current !== generation) return;
              if (body && body.ok) {
                state.session = {
                  session_id: body.session_id,
                  expires_at_ms: body.expires_at_ms,
                };
                await refresh();
              } else failed(body);
            })
          );
          form.appendChild(input);
          form.appendChild(btn);
          input.addEventListener('keydown', (event) => {
            if (event.key === 'Enter') { event.preventDefault(); btn.click(); }
          });
          loginForm = form;
          loginInput = input;
        }
        host.appendChild(loginForm);
        host.appendChild(el('p', { class: 'team-login-help' }, t(i18n, 'ui.web_login_help')));
        if (refocusInput) loginInput.focus();
      }
      if (state.error) {
        const message = el('p', { class: 'team-error', role: 'alert' });
        message.appendChild(el('strong', null, state.error.code));
        message.appendChild(document.createTextNode(' · ' + state.error.message));
        host.appendChild(message);
        if (state.disconnect) host.appendChild(el('p', { class: 'sub' }, t(i18n, 'ui.team_unknown_outcome')));
      }
    }

    async function signOut(path, payload) {
      const current = ++generation;
      clearProjectData();
      state.error = null;
      state.loading = false;
      render();
      const body = await api(path, { method: 'POST', body: JSON.stringify(payload) });
      if (current !== generation) return;
      if (body && body.ok) state.session = null;
      else failed(body);
    }

    function clear(node) {
      while (node && node.firstChild) node.removeChild(node.firstChild);
    }

    function render() {
      const projects = $('teamProjects');
      const overview = $('teamOverview');
      const detail = $('teamDetail');
      const auth = $('teamAuth');
      const signedIn = Boolean(state.session && state.session.session_id);
      const view = $('view-team');
      if (view) view.dataset.teamState = signedIn ? 'signed-in' : 'signed-out';
      const grid = $('teamWorkspaceGrid');
      if (grid) grid.hidden = !signedIn;
      const intro = $('teamWorkspaceIntro');
      if (intro) intro.hidden = signedIn;
      const rawSection = $('teamRaw');
      if (rawSection) rawSection.hidden = !signedIn || !state.raw;
      if (projects) projects.hidden = !signedIn || !state.projects.length;
      if (detail) detail.hidden = !signedIn || !state.works.length;
      const active = typeof document !== 'undefined' ? document.activeElement : null;
      const activeKey = active && active.dataset && active.dataset.key;
      const frame = overview && overview.querySelector('.scene-frame');
      const scroll = frame ? [frame.scrollLeft, frame.scrollTop] : [0, 0];
      if (auth) { auth.classList.toggle('signed-in', signedIn); renderAuth(auth); }
      if (projects) renderProjects(projects);
      if (overview) renderOverview(overview);
      if (detail) renderDetail(detail);
      if (overview && net) {
        network.layout(overview, net);
        const nextFrame = overview.querySelector('.scene-frame');
        if (nextFrame) { nextFrame.scrollLeft = scroll[0]; nextFrame.scrollTop = scroll[1]; }
        if (activeKey) {
          const card = [...overview.querySelectorAll('.task-node')].find(n => n.dataset.key === activeKey);
          if (card) card.focus({ preventScroll: true });
        }
      }
      if (admin) admin.setContext(signedIn && state.raw && state.raw.identity ? {
        project: state.projectKey, session: state.session.session_id, identity: state.raw.identity,
        mcpUrl: state.raw.mcp_url,
      } : null);
      const raw = $('rawTeamBody');
      if (raw) {
        // The upstream session identifier is a cookie credential, not debug data.
        const { session, ...overviewData } = state.raw || {};
        raw.textContent = state.raw ? JSON.stringify({ overview: overviewData, detail: state.detailResponse }, null, 2) : '';
      }
    }

    async function refresh() {
      const current = ++generation;
      state.error = null;
      state.loading = true;
      state.works = [];
      state.streams = [];
      state.graphLoading = false;
      pendingDetails = new Map();
      state.members = [];
      state.raw = null;
      state.detailLoading = false;
      state.detailResponse = null;
      render();
      const projects = await api('/api/team/projects?view=' + encodeURIComponent(state.viewMode));
      if (current !== generation) return;
      if (!projects || !projects.ok || !Array.isArray(projects.projects)) {
        clearProjectData();
        if (projects && projects.error && projects.error.code === 'Unauthenticated' && !state.session) {
          state.error = null; // The first visit is a normal sign-in state.
        } else failed(projects);
      } else {
        state.projects = projects.projects;
        state.session = projects.session || state.session;
        if (!state.projects.some((p) => p.key === state.projectKey)) {
          let saved = null;
          try { saved = root.sessionStorage.getItem('awr-team-project'); } catch (_) { /* Storage is optional. */ }
          state.projectKey = state.projects.some(p => p.key === saved) ? saved : state.projects[0]?.key || null;
          state.selected = null;
          state.lastReceipts = Object.create(null);
        }
        if (state.projectKey) {
          const overview = await api('/api/team/overview?project=' + encodeURIComponent(state.projectKey) + '&view=' + encodeURIComponent(state.viewMode));
          if (current !== generation) return;
          if (!overview || !overview.ok || !Array.isArray(overview.works)) failed(overview);
          else {
            state.works = overview.works.map((work) => ({ ...work }));
            state.members = overview.members || [];
            state.streams = overview.workstreams || [];
            state.session = overview.session || state.session;
            state.raw = overview;
            state.disconnect = false;
          }
        }
      }
      if (!selectedWork()) state.selected = null;
      state.loading = false;
      render();
      if (isLive()) {
        if (state.streams.length) {
          if (!state.selected && state.works.length) state.selected = state.works[0].key;
          await loadGraphDetails();
          if (current !== generation) return;
          if (!state.selected && state.works.length) state.selected = state.works[0].key;
        }
        if (state.selected) await selectWork(state.selected);
      }
      if (!state.error && current === generation && state.session) {
        state.lastRefreshedAt = Date.now();
        render();
      }
      scheduleRefresh();
    }

    function editing() {
      const active = typeof document !== 'undefined' ? document.activeElement : null;
      return ['INPUT', 'TEXTAREA', 'SELECT'].includes(active?.tagName) || active?.isContentEditable;
    }

    function canRefreshProgress() {
      return state.session && isLive() && !state.loading && !state.graphLoading && !state.detailLoading && !state.refreshing
        && !root.document?.hidden && !$('teamWorkspaceGrid')?.hidden
        && (!root.location?.hash || root.location.hash === '#team')
        && !editing();
    }

    function scheduleRefresh(delay = PROGRESS_REFRESH_MS) {
      if (typeof root.addEventListener !== 'function') return;
      clearTimeout(refreshTimer);
      if (state.session) refreshTimer = setTimeout(async () => {
        const started = performance.now();
        try { await refreshProgress(); } finally {
          // Maintain the cadence without tight retry loops after a slow read.
          scheduleRefresh(Math.max(1000, PROGRESS_REFRESH_MS - (performance.now() - started)));
        }
      }, delay);
    }

    async function refreshProgress() {
      if (!canRefreshProgress()) return;
      const current = ++generation, project = state.projectKey, selection = detailGeneration;
      state.refreshing = true;
      state.error = null;
      pendingDetails = new Map();
      try {
        const overview = await api('/api/team/overview?project=' + encodeURIComponent(project) + '&view=team');
        if (current !== generation) return;
        if (!overview?.ok || !Array.isArray(overview.works)) { failed(overview); return; }
        const works = overview.works.map(work => ({ ...work }));
        const queue = works.slice(0, 60);
        const selected = works.find(w => w.key === state.selected);
        if (selected) {
          const position = queue.indexOf(selected);
          if (position >= 0) queue.splice(position, 1);
          queue.unshift(selected);
        }
        let redrawPending = false;
        const publish = () => {
          if (current !== generation || state.error || $('teamWorkspaceGrid')?.hidden || editing()) return;
          const foreground = selectedWork();
          state.works = works.map(work => {
            if (selection !== detailGeneration && foreground?.key === work.key
              && foreground.workstream_id === work.workstream_id && foreground.contract_hash === work.contract_hash
              && foreground.detail_loaded && (!work.detail_loaded
                || (detailReadOrder.get(foreground) || 0) > (detailReadOrder.get(work) || 0))) return foreground;
            return work;
          });
          state.streams = overview.workstreams || [];
          state.raw = overview;
          state.session = overview.session || state.session;
          state.disconnect = false;
          if (!selectedWork()) state.selected = works[0]?.key || null;
          state.detailResponse = selectedWork()?.detail_loaded ? { ok: true, work: selectedWork() } : null;
          if (!state.detailLoading && !redrawPending) {
            const redraw = () => {
              redrawPending = false;
              if (current === generation && !state.detailLoading && !$('teamWorkspaceGrid')?.hidden && !editing()) render();
            };
            if (typeof root.requestAnimationFrame === 'function') { redrawPending = true; root.requestAnimationFrame(redraw); }
            else redraw();
          }
        };
        let index = 0;
        await Promise.all(Array.from({ length: Math.min(4, queue.length) }, async () => {
          while (current === generation && index < queue.length) {
            await readWork(queue[index++]);
            publish();
          }
        }));
        // A new selection may be outside this bounded batch. Keep its foreground
        // detail rather than replacing it with an unread object from the poll.
        if (current !== generation || state.error || $('teamWorkspaceGrid')?.hidden || editing()) return;
        publish();
        state.lastRefreshedAt = Date.now();
      } finally {
        state.refreshing = false;
        // An in-flight observation must never redraw a newly opened member form.
        if (current === generation && !state.detailLoading && !$('teamWorkspaceGrid')?.hidden && !editing()) render();
      }
    }

    if (typeof root.addEventListener === 'function') {
      root.addEventListener('focus', () => { void refreshProgress(); });
      root.document?.addEventListener('visibilitychange', () => { if (!root.document.hidden) void refreshProgress(); });
    }

    const overviewHost = $('teamOverview');
    if (overviewHost && typeof ResizeObserver !== 'undefined') {
      new ResizeObserver(() => { if (net) network.layout(overviewHost, net); }).observe(overviewHost);
    }

    return {
      state,
      refresh,
      render,
      api,
      // test hooks
      _guardDouble: guardDouble,
      _runAction: runAction,
      _selectedWork: selectedWork,
      _selectWork: selectWork,
      _loadGraphDetails: loadGraphDetails,
      _refreshProgress: refreshProgress,
    };
  }

  const api = { createTeamWeb, PROGRESS_REFRESH_MS, DETAIL_TIMEOUT_MS };
  if (typeof module !== 'undefined' && module.exports) module.exports = api;
  root.AWR_TEAM_WEB = api;
})(typeof window !== 'undefined' ? window : globalThis);
