/**
 * Work item details for the project map: the facts shown in the detail popup, and the dependency chain used to highlight a
 * card's upstream and downstream. Both are pure functions of the model (a snapshot plus the display configuration), so what a
 * popup says can be tested without a browser. Everything shown comes from the snapshot, which comes from the official awr
 * commands; fields they do not provide are listed as unavailable, never guessed.
 */
(function (root) {
  'use strict';

  const commonJS = typeof module !== 'undefined' && module.exports;
  const kitLib = commonJS ? require('./svgkit.js') : root.AWR_PROJECT_MAP.svgkit;
  const modelLib = commonJS ? require('./model.js') : root.AWR_PROJECT_MAP.model;
  const viewsLib = commonJS ? require('./views.js') : root.AWR_PROJECT_MAP.views;
  const { esc, compare } = kitLib;

  const DAY_MS = 86400000;

  /** Declared prerequisites and dependents of every item, from the snapshot's edges (not only the drawn ones). */
  function dependencyIndex(model) {
    const up = new Map();
    const down = new Map();
    for (const node of model.nodes.values()) {
      up.set(node.id, [...node.deps]);
      if (!down.has(node.id)) down.set(node.id, []);
    }
    for (const node of model.nodes.values()) for (const d of node.deps) if (down.has(d)) down.get(d).push(node.id);
    for (const list of down.values()) list.sort(compare);
    return { up, down };
  }

  function reach(start, graph) {
    const seen = new Set();
    const stack = [start];
    while (stack.length) {
      for (const next of graph.get(stack.pop()) || []) {
        if (!seen.has(next) && next !== start) { // an item is never its own upstream, even in a dependency cycle
          seen.add(next);
          stack.push(next);
        }
      }
    }
    return seen;
  }

  /** All upstream and downstream items of `key`, through every level. */
  function neighborhood(model, key, index = dependencyIndex(model)) {
    return { up: reach(key, index.up), down: reach(key, index.down) };
  }

  /** The facts of one item. `t` is only used for the lane name. */
  function detailData(model, key, t, index = dependencyIndex(model)) {
    const node = model.nodes.get(key);
    const raw = model.raw.get(key);
    if (!node || !raw) return null;
    const { meta, cfg } = model;
    const lane = cfg.overview.lanes.find((l) => l.id === node.lane);
    const brief = (k) => {
      const n = model.nodes.get(k);
      return { key: k, short: n.short, title: n.title, vis: n.vis, status: n.status };
    };
    const around = neighborhood(model, key, index);
    const cancelledAbove = [...around.up].filter((k) => model.nodes.get(k).vis === 'cancelled').sort(compare);
    const sessions = meta.sessions.filter((s) => s.work_key === key)
      .map((s) => ({ id: s.id, agent: s.agent_id, status: s.status, startedAt: s.started_at, lastEventAt: s.last_event_at }));
    const related = new Set([key, raw.id, ...raw.claims.map((c) => c.id), ...sessions.map((s) => s.id)]);
    const findings = model.snapshot.doctor.findings.filter((f) => related.has(f.object_id))
      .map((f) => ({ code: f.code, severity: f.severity, kind: f.object_kind, message: f.message }));
    return {
      key,
      short: node.short,
      title: node.title,
      status: node.status,
      vis: node.vis,
      lane: lane ? modelLib.laneName(lane, t) : node.lane,
      milestone: node.milestone,
      owner: node.owner,
      ready: raw.ready,
      archived: raw.archived,
      blocker: raw.blocker,
      nextAction: raw.next_action,
      waits: raw.waits.map((w) => ({ kind: w.kind, summary: w.summary, release: w.release_condition })),
      diagnostics: raw.diagnostics.map((d) => ({ code: d.code, detail: d.detail, work: d.work_item_key })),
      claims: raw.claims.map((c) => ({ agent: c.agent_id, session: c.session_id, acquiredAt: c.acquired_at, expiresAt: c.expires_at })),
      lastEventAt: raw.last_event_at,
      idleDays: node.idle,
      generatedAt: meta.generated_at,
      prerequisites: (index.up.get(key) || []).map(brief),
      dependents: (index.down.get(key) || []).map(brief),
      upstream: around.up.size,
      downstream: around.down.size,
      stuck: meta.stuck.has(key),
      cancelledAbove,
      sessions,
      findings,
      gaps: model.snapshot.unavailable.filter((u) => u.field.startsWith('node.')),
    };
  }

  const utc = (ms) => `${viewsLib.utc(ms)} UTC`;

  /** The popup body as HTML (all text escaped). `t` is the language function. */
  function renderDetail(d, t, gapReason) {
    const badge = (vis, label) => `<span class="pm-badge" data-vis="${esc(vis)}">${esc(label)}</span>`;
    const jump = (n) => `<li><button type="button" class="pm-link" data-pm-jump="${esc(n.key)}"><code>${esc(n.short)}</code> <span class="pm-link-title">${esc(n.title)}</span> ${badge(n.vis, t(`map.status.${n.vis}`))}</button></li>`;
    const section = (title, body) => `<section class="pm-d-sec"><h3>${esc(title)}</h3>${body}</section>`;
    const row = (label, value) => `<dt>${esc(label)}</dt><dd>${value}</dd>`;
    const list = (items) => `<ul class="pm-d-list">${items.join('')}</ul>`;
    const none = `<p class="pm-d-none">${esc(t('map.detail.none'))}</p>`;

    const parts = [];
    const flags = [badge(d.vis, t(`map.status.${d.vis}`))];
    if (d.stuck) flags.push(`<span class="pm-badge pm-warn">${esc(t('map.detail.stuck'))}</span>`);
    if (d.archived) flags.push(`<span class="pm-badge">${esc(t('map.detail.archived'))}</span>`);
    parts.push(`<header class="pm-d-head"><div class="pm-d-key"><code>${esc(d.key)}</code> ${flags.join(' ')}</div><h2 id="pm-drawer-title">${esc(d.title)}</h2></header>`);

    const last = d.lastEventAt
      ? t('map.detail.last_value', { time: utc(d.lastEventAt), days: Math.max(0, Math.round((d.generatedAt - d.lastEventAt) / DAY_MS)) })
      : t('map.detail.never');
    parts.push(section(t('map.detail.summary'), `<dl class="pm-d-grid">${[
      row(t('map.detail.status'), `<code>${esc(d.status)}</code>`),
      row(t('map.detail.ready'), esc(t(d.ready ? 'map.detail.ready_yes' : 'map.detail.ready_no'))),
      row(t('map.detail.lane'), esc(d.lane)),
      row(t('map.detail.milestone'), d.milestone ? esc(d.milestone) : `<span class="pm-d-none">${esc(t('map.detail.unset'))}</span>`),
      row(t('map.detail.owner'), d.owner ? esc(d.owner) : `<span class="pm-d-none">${esc(t('map.detail.unset'))}</span>`),
      row(t('map.detail.last'), esc(last)),
      row(t('map.detail.chain'), esc(t('map.detail.chain_value', { up: d.upstream, down: d.downstream }))),
    ].join('')}</dl>`));

    if (d.stuck) {
      parts.push(section(t('map.detail.stuck'), `<p>${esc(t('map.detail.stuck_text'))}</p>`
        + (d.cancelledAbove.length ? `<p>${esc(t('map.detail.stuck_behind', { keys: d.cancelledAbove.join(t('map.join.list')) }))}</p>` : '')));
    }
    if (d.blocker) parts.push(section(t('map.detail.blocker'), `<p>${esc(d.blocker)}</p>`));
    if (d.nextAction) parts.push(section(t('map.detail.next_action'), `<p>${esc(d.nextAction)}</p>`));
    if (d.waits.length) {
      parts.push(section(t('map.detail.waits'), list(d.waits.map((w) => `<li><code>${esc(w.kind)}</code> ${esc(w.summary)}<br><small>${esc(w.release)}</small></li>`))));
    }
    if (d.diagnostics.length) {
      parts.push(section(t('map.detail.diagnostics'), list(d.diagnostics.map((x) => `<li><code>${esc(x.code)}</code> ${esc(x.detail)}${x.work && x.work !== d.key ? ` <small>${esc(t('map.detail.on', { key: x.work }))}</small>` : ''}</li>`))));
    }
    parts.push(section(t('map.detail.claims'), d.claims.length
      ? list(d.claims.map((c) => `<li><code>${esc(c.agent)}</code> · <code>${esc(c.session)}</code><br><small>${esc(t('map.detail.acquired'))} ${esc(utc(c.acquiredAt))} · ${esc(t('map.detail.expires'))} ${esc(utc(c.expiresAt))}</small></li>`))
      : none));
    parts.push(section(t('map.detail.needs', { count: d.prerequisites.length }), d.prerequisites.length ? list(d.prerequisites.map(jump)) : none));
    parts.push(section(t('map.detail.unlocks', { count: d.dependents.length }), d.dependents.length ? list(d.dependents.map(jump)) : none));
    if (d.sessions.length) {
      parts.push(section(t('map.detail.sessions'), list(d.sessions.map((x) => `<li><code>${esc(x.agent)}</code> <code>${esc(x.id)}</code> <span class="pm-badge">${esc(x.status)}</span><br><small>${esc(t('map.detail.started'))} ${esc(utc(x.startedAt))} · ${esc(t('map.detail.last_event'))} ${x.lastEventAt ? esc(utc(x.lastEventAt)) : '—'}</small></li>`))));
    }
    if (d.findings.length) {
      parts.push(section(t('map.detail.findings'), list(d.findings.map((f) => `<li><code>${esc(f.code)}</code> <small>${esc(f.severity)}</small> ${esc(f.message)}</li>`))));
    }
    parts.push(section(t('map.detail.gaps'), `<p class="pm-d-note">${esc(t('map.detail.gaps_text'))}</p>${list(d.gaps.map((g) => `<li><code>${esc(g.field)}</code>: ${esc(gapReason(g))}</li>`))}`));
    parts.push(`<footer class="pm-d-actions"><button type="button" class="pm-btn-solid" data-pm-action="reveal">${esc(t('map.detail.reveal'))}</button><button type="button" data-pm-action="copy-key">${esc(t('map.detail.copy_key'))}</button><button type="button" data-pm-action="copy-command">${esc(t('map.detail.copy_command'))}</button><span class="pm-d-status" data-pm-status-line role="status" aria-live="polite"></span></footer>`);
    return parts.join('');
  }

  /** The awr command that shows this item, for copying. */
  const commandFor = (key) => `awr work show ${/^[A-Za-z0-9_.:#/-]+$/.test(key) ? key : `'${key.replace(/'/g, "'\\''")}'`}`;

  const api = { dependencyIndex, neighborhood, detailData, renderDetail, commandFor };
  if (commonJS) module.exports = api;
  else {
    root.AWR_PROJECT_MAP = root.AWR_PROJECT_MAP || {};
    root.AWR_PROJECT_MAP.detail = api;
  }
})(typeof globalThis !== 'undefined' ? globalThis : this);
