/**
 * Interaction layer of the project map: what makes the cards and lanes of the diagrams alive.
 *
 *  - click or Enter/Space on a card (or a row of the table) opens its details in a panel, with jumpable prerequisites and
 *    dependents; the panel also offers "show in graph", copy key and copy the awr command;
 *  - hovering, focusing or selecting a card highlights its whole dependency chain (upstream and downstream, through every
 *    level) and dims the rest;
 *  - lanes, panels, finished work and "N more" blocks fold and unfold: the view is drawn again from a view state, so a
 *    folded diagram is exactly what the renderer produces for that state;
 *  - search, status filter, zoom (buttons, keys, ctrl/cmd + wheel), drag to pan, fold all / unfold all, show finished work;
 *    an executor chip in the overview shows only that agent's work (it sets the search), and the table follows the search;
 *  - keyboard access and ARIA throughout (cards and folds are focusable buttons, Esc closes the panel, "/" searches).
 *
 * The same file runs in the Inspector page and inside the exported HTML. The decisions are pure (detail.js and the views);
 * this module only turns events into state changes and applies them to the DOM, using a small subset of it:
 * querySelector(All), closest, contains, classList, get/set/hasAttribute, innerHTML, textContent, hidden, addEventListener,
 * focus, scrollIntoView and the scroll geometry of the diagram containers.
 */
(function (root) {
  'use strict';

  const commonJS = typeof module !== 'undefined' && module.exports;
  const modelLib = commonJS ? require('./model.js') : root.AWR_PROJECT_MAP.model;
  const viewsLib = commonJS ? require('./views.js') : root.AWR_PROJECT_MAP.views;
  const detailLib = commonJS ? require('./detail.js') : root.AWR_PROJECT_MAP.detail;
  const pageLib = commonJS ? require('./page.js') : root.AWR_PROJECT_MAP.page;
  const kitLib = commonJS ? require('./svgkit.js') : root.AWR_PROJECT_MAP.svgkit;

  const VIEW_IDS = ['overview', 'mainline', 'explore'];
  const ZOOM_STEPS = [0.25, 0.33, 0.5, 0.67, 0.8, 0.9, 1, 1.1, 1.25, 1.5, 2, 3];
  const STATUS_ORDER = ['done', 'developing', 'stalled', 'blocked', 'ready', 'draft', 'waiting', 'cancelled'];
  const FOLD_ATTRS = ['data-lane', 'data-panel', 'data-xlane', 'data-done-lane', 'data-group', 'data-panel-done'];
  const FOLDABLE = FOLD_ATTRS.map((a) => `[${a}]`).join(',');
  const ACTIVATABLE = '.pm-card,.pm-fold,.pm-btn,.pm-agent,tr[data-key]';
  const PAN_THRESHOLD = 4;
  const { esc, compare } = kitLib;

  const sets = (...names) => Object.fromEntries(names.map((n) => [n, new Set()]));
  const quote = (value) => String(value).replace(/["\\]/g, '\\$&');

  /** Fresh view state: nothing folded, selected, searched or zoomed. */
  function createState(view = 'overview') {
    return {
      view,
      selected: null,
      hover: null,
      search: '',
      filters: new Set(),
      zoom: { overview: 1, mainline: 1, explore: 1 },
      drawer: null,
      fold: {
        overview: sets('collapsed', 'doneOpen', 'groupsOpen'),
        mainline: sets('collapsed', 'showDone'),
        explore: { collapsed: new Set(), showDone: false },
      },
    };
  }

  /** Items matching a search text: key, title, owner, milestone and status, case-insensitive. */
  function matching(model, text, t) {
    const q = String(text || '').trim().toLowerCase();
    const found = new Set();
    if (!q) return found;
    for (const n of model.nodes.values()) {
      const fields = [n.id, n.title, n.owner, n.milestone, n.status, t(`map.status.${n.vis}`)];
      if (fields.some((f) => f && String(f).toLowerCase().includes(q))) found.add(n.id);
    }
    return found;
  }

  /**
   * What each fold action reaches in the current model: lanes, panels, finished work and the groups that hide cards.
   * The toolbar and the tests read the same lists.
   */
  function foldTargets(model) {
    const { nodes, cfg } = model;
    const all = [...nodes.values()];
    const groups = [];
    for (const lane of cfg.overview.lanes) {
      for (const vis of viewsLib.GROUP_ORDER) {
        if (all.filter((n) => n.lane === lane.id && n.vis === vis).length > 5) groups.push(`${lane.id}|${vis}`);
      }
    }
    const panels = cfg.mainline_panels || [];
    const milestones = new Set((cfg.explore_lanes || []).flatMap((l) => l.milestones));
    return {
      overviewLanes: cfg.overview.lanes.map((l) => l.id),
      overviewDone: cfg.overview.lanes.filter((l) => all.some((n) => n.lane === l.id && n.vis === 'done')).map((l) => l.id),
      overviewGroups: groups,
      mainlinePanels: panels.filter((p) => all.some((n) => n.lane === p.lane)).map((p) => p.lane),
      mainlineDone: panels.filter((p) => !p.keep_done && all.some((n) => n.lane === p.lane && n.vis === 'done')).map((p) => p.lane),
      exploreLanes: (cfg.explore_lanes || []).map((_, i) => i),
      exploreDone: all.some((n) => n.vis === 'done' && milestones.has(n.milestone)),
    };
  }

  /**
   * Attach the interaction layer to `container`, which holds the markup of `page.renderProjectMap({interactive: true}).body`.
   * Options: snapshot and config (or an existing `model`), `t` (language function), `restore` (the `state` of an earlier
   * mount, so a refresh keeps the folds, zoom and search), `offsetTop()` (pixels of a host bar the details panel must start
   * below; default none), `copy(text)` (clipboard; injectable), `timers` and `win` (the window: resize, scrollBy and the
   * clipboard; injectable for tests).
   */
  function mount({
    container, snapshot, config, t, model: given, restore, offsetTop, copy, win = root, timers = { set: (fn, ms) => setTimeout(fn, ms), clear: (id) => clearTimeout(id) },
  }) {
    const model = given || modelLib.loadModel(snapshot, config);
    const views = viewsLib.createViews(t);
    const index = detailLib.dependencyIndex(model);
    const targets = foldTargets(model);
    const state = createState();
    const doc = container.ownerDocument;
    const lanes = new Map(model.cfg.overview.lanes.map((l) => [l.id, l]));
    const exploreLanes = model.cfg.explore_lanes || [];
    const exploreIndex = new Map();
    exploreLanes.forEach((lane, i) => lane.milestones.forEach((m) => exploreIndex.set(m, i)));
    const all = [...model.nodes.values()];
    const edgeCount = all.reduce((s, n) => s + n.deps.length, 0);
    const listeners = [];
    const pulses = new Set();
    let returnTo = null; // where the keyboard goes back to when the panel closes: the element and its key (it may be drawn again meanwhile)
    let matchCursor = -1;
    let pan = null;
    let suppressClick = false;

    const section = (id) => container.querySelector(`#s-${id} .scroll`);
    const attr = (el, name) => el.getAttribute(name);
    const on = (target, type, fn, options) => {
      target.addEventListener(type, fn, options);
      listeners.push([target, type, fn, options]);
    };
    const labelOf = (id) => ({
      overview: t('map.view.overview_label', { nodes: model.nodes.size, edges: edgeCount }),
      mainline: t('map.view.mainline_label'),
      explore: t('map.view.explore_label'),
    }[id]);
    const toggle = (set, value) => {
      if (set.has(value)) set.delete(value);
      else set.add(value);
    };

    // ------------------------------------------------------------------ drawing

    /** Draw view `id` for the current fold state and make its parts reachable. */
    function draw(id) {
      const target = section(id);
      if (!target) return;
      const render = id === 'overview' ? views.renderOverview : id === 'mainline' ? views.renderMainline : views.renderExplore;
      target.innerHTML = pageLib.finishView(id, render(model, state.fold), labelOf(id));
      decorate(id);
    }

    const laneLabel = (id) => (lanes.has(id) ? modelLib.laneName(lanes.get(id), t) : id);

    /** What a fold control is called: its lane or panel, and for the smaller controls of a lane also what they say. */
    function foldName(el) {
      const named = el.querySelector('title');
      const text = ((named ? named.textContent : el.textContent) || '').trim().replace(/\s+/g, ' ').slice(0, 80);
      const header = attr(el, 'data-lane') || attr(el, 'data-panel');
      if (header) return laneLabel(header);
      const x = attr(el, 'data-xlane');
      if (x !== null && exploreLanes[Number(x)]) return exploreLanes[Number(x)].name;
      const owner = attr(el, 'data-done-lane') || attr(el, 'data-panel-done') || String(attr(el, 'data-group') || '').split('|')[0];
      return owner ? `${laneLabel(owner)} \u00b7 ${text}` : text;
    }

    /** Roles, tab order, names and zoom for what the markup of a view contains. */
    function decorate(id) {
      const target = section(id);
      if (!target) return;
      const svg = target.querySelector('svg');
      // The drawing is no longer one picture: its parts are controls, so it is a group, not an image.
      if (svg) svg.setAttribute('role', 'group');
      for (const card of target.querySelectorAll('.pm-card')) {
        const n = model.nodes.get(attr(card, 'data-key'));
        card.setAttribute('tabindex', '0');
        card.setAttribute('role', 'button');
        if (n) card.setAttribute('aria-label', t('map.ui.card_label', { key: n.id, title: n.title, status: t(`map.status.${n.vis}`) }));
      }
      for (const agent of target.querySelectorAll('.pm-agent')) {
        agent.setAttribute('tabindex', '0');
        agent.setAttribute('role', 'button');
        agent.setAttribute('aria-label', t('map.ui.agent_filter', { name: attr(agent, 'data-agent') }));
      }
      for (const control of target.querySelectorAll('.pm-fold,.pm-btn')) {
        control.setAttribute('tabindex', '0');
        control.setAttribute('role', 'button');
        if (control.hasAttribute('data-open')) control.setAttribute('aria-expanded', attr(control, 'data-open'));
        const name = foldName(control);
        if (name) control.setAttribute('aria-label', t('map.ui.fold', { name }));
      }
      applyZoom(id);
      applyEmphasis();
    }

    function applyZoom(id) {
      const target = section(id);
      const svg = target && target.querySelector('svg');
      const box = svg && svg.getAttribute('viewBox') ? svg.getAttribute('viewBox').trim().split(/\s+/).map(Number) : null;
      if (!box || box.length !== 4 || box.some((v) => !Number.isFinite(v))) return;
      svg.setAttribute('width', String(Math.round(box[2] * state.zoom[id])));
      svg.setAttribute('height', String(Math.round(box[3] * state.zoom[id])));
    }

    // ------------------------------------------------------------------ emphasis (highlight, search, filter)

    function focusSet() {
      const key = state.selected || state.hover;
      if (!key || !model.nodes.has(key)) return null;
      const around = detailLib.neighborhood(model, key, index);
      // an item without any chain has nothing to set apart from the rest, so the rest is not dimmed
      return { key, up: around.up, down: around.down, alone: around.up.size + around.down.size === 0 };
    }

    function applyEmphasis() {
      const focus = focusSet();
      const found = matching(model, state.search, t);
      const query = state.search.trim() !== '';
      const inFocus = (k) => !focus || focus.alone || k === focus.key || focus.up.has(k) || focus.down.has(k);
      for (const card of container.querySelectorAll('.pm-card')) {
        const key = attr(card, 'data-key');
        const dim = (focus && !inFocus(key)) || (query && !found.has(key)) || (state.filters.size > 0 && !state.filters.has(attr(card, 'data-vis')));
        card.classList.toggle('pm-dim', Boolean(dim));
        card.classList.toggle('pm-self', Boolean(focus && key === focus.key));
        card.classList.toggle('pm-up', Boolean(focus && focus.up.has(key)));
        card.classList.toggle('pm-down', Boolean(focus && focus.down.has(key)));
        card.classList.toggle('pm-match', Boolean(query && found.has(key)));
        card.classList.toggle('pm-selected', key === state.selected);
      }
      for (const path of container.querySelectorAll('[data-from]')) {
        const from = attr(path, 'data-from');
        const to = attr(path, 'data-to');
        // An edge belongs to the chain when it leads into the item or one of its prerequisites (so it starts upstream too, or at
        // the block that stands for finished work) or when it leaves the item or one of its dependents (so it ends downstream).
        const onChain = Boolean(focus) && (to === focus.key || focus.up.has(to) || from === focus.key || focus.down.has(from));
        path.classList.toggle('pm-hl', onChain);
        path.classList.toggle('pm-dim', Boolean(focus) && !focus.alone && !onChain);
      }
      for (const agent of container.querySelectorAll('.pm-agent')) {
        const on = query && state.search.trim() === attr(agent, 'data-agent');
        agent.classList.toggle('pm-on', Boolean(on));
        agent.setAttribute('aria-pressed', String(Boolean(on)));
      }
      for (const row of container.querySelectorAll('tr[data-key]')) {
        const key = attr(row, 'data-key');
        row.classList.toggle('pm-selected', key === state.selected);
        row.classList.toggle('pm-dim', Boolean((query && !found.has(key)) || (state.filters.size > 0 && !state.filters.has(attr(row, 'data-vis')))));
      }
      container.classList.toggle('pm-focusing', Boolean(focus || query || state.filters.size));
      syncToolbar(focus, found, query);
    }

    // ------------------------------------------------------------------ toolbar

    function toolbarHtml() {
      const present = STATUS_ORDER.filter((v) => all.some((n) => n.vis === v));
      const chips = present.map((v) => `<button type="button" class="pm-chip" data-pm-status="${v}" aria-pressed="false"><span class="pm-swatch" data-vis="${v}"></span>${esc(t(`map.status.${v}`))}</button>`).join('');
      const act = (name, label, extra = '') => `<button type="button" data-pm-action="${name}"${extra}>${esc(label)}</button>`;
      return `<div class="pm-bar" role="toolbar" aria-label="${esc(t('map.ui.toolbar'))}">`
        + `<label class="pm-search"><span class="pm-sr">${esc(t('map.ui.search'))}</span><input type="search" data-pm-search placeholder="${esc(t('map.ui.search_placeholder'))}" autocomplete="off" spellcheck="false"></label>`
        + '<span class="pm-count" data-pm-count role="status" aria-live="polite"></span>'
        + `<span class="pm-group" role="group" aria-label="${esc(t('map.ui.filter'))}">${chips}</span>`
        + `<span class="pm-group" role="group" aria-label="${esc(t('map.ui.zoom'))}">${act('zoom-out', '\u2212', ` aria-label="${esc(t('map.ui.zoom_out'))}"`)}${act('zoom-fit', t('map.ui.zoom_fit'))}${act('zoom-100', t('map.ui.zoom_100'))}${act('zoom-in', '+', ` aria-label="${esc(t('map.ui.zoom_in'))}"`)}</span>`
        + `<span class="pm-group" role="group" aria-label="${esc(t('map.ui.layout'))}">${act('collapse-all', t('map.ui.collapse_all'))}${act('expand-all', t('map.ui.expand_all'))}${act('toggle-done', t('map.ui.show_done'), ' aria-pressed="false" data-pm-done-toggle')}</span>`
        + `${act('clear', t('map.ui.clear'), ' hidden')}</div><p class="pm-note" data-pm-note>${esc(t('map.ui.hint'))}</p>`;
    }

    /** Whether the active view has anything for the fold buttons to act on. */
    function foldableNow() {
      if (state.view === 'overview') return { folds: targets.overviewLanes.length > 0, done: targets.overviewDone.length > 0 };
      if (state.view === 'mainline') return { folds: targets.mainlinePanels.length > 0, done: targets.mainlineDone.length > 0 };
      return { folds: targets.exploreLanes.length > 0, done: targets.exploreDone };
    }

    function doneIsOpen() {
      const f = state.fold;
      if (state.view === 'overview') return targets.overviewDone.length > 0 && targets.overviewDone.every((l) => f.overview.doneOpen.has(l));
      if (state.view === 'mainline') return targets.mainlineDone.length > 0 && targets.mainlineDone.every((l) => f.mainline.showDone.has(l));
      return f.explore.showDone;
    }

    function setDisabled(el, disabled) {
      if (!el) return;
      if (disabled) el.setAttribute('disabled', '');
      else el.removeAttribute('disabled');
    }

    function syncToolbar(focus, found, query) {
      const bar = container.querySelector('.pm-toolbar');
      if (!bar) return;
      for (const chip of bar.querySelectorAll('[data-pm-status]')) chip.setAttribute('aria-pressed', String(state.filters.has(attr(chip, 'data-pm-status'))));
      const count = bar.querySelector('[data-pm-count]');
      if (count) count.textContent = query ? (found.size ? t('map.ui.matches', { count: found.size }) : t('map.ui.no_matches')) : '';
      const note = bar.querySelector('[data-pm-note]');
      if (note) note.textContent = focus ? t('map.ui.highlighting', { key: focus.key, up: focus.up.size, down: focus.down.size }) : t('map.ui.hint');
      const clear = bar.querySelector('[data-pm-action="clear"]');
      if (clear) clear.hidden = !(state.selected || state.search || state.filters.size);
      const can = foldableNow();
      setDisabled(bar.querySelector('[data-pm-action="collapse-all"]'), !can.folds);
      setDisabled(bar.querySelector('[data-pm-action="expand-all"]'), !can.folds);
      const done = bar.querySelector('[data-pm-done-toggle]');
      if (done) {
        const open = doneIsOpen();
        done.setAttribute('aria-pressed', String(open));
        done.textContent = t(open ? 'map.ui.hide_done' : 'map.ui.show_done');
        setDisabled(done, !can.done);
      }
    }

    // ------------------------------------------------------------------ folding

    function foldSelector(el) {
      for (const a of FOLD_ATTRS) {
        const value = el.getAttribute(a);
        if (value !== null) return `[${a}="${quote(value)}"]`;
      }
      return null;
    }

    function foldClick(el) {
      const f = state.fold;
      const lane = attr(el, 'data-lane');
      const panel = attr(el, 'data-panel');
      const x = attr(el, 'data-xlane');
      const doneLane = attr(el, 'data-done-lane');
      const group = attr(el, 'data-group');
      const panelDone = attr(el, 'data-panel-done');
      const selector = foldSelector(el);
      if (lane) toggle(f.overview.collapsed, lane);
      else if (panel) toggle(f.mainline.collapsed, panel);
      else if (x !== null) toggle(f.explore.collapsed, Number(x));
      else if (doneLane) toggle(f.overview.doneOpen, doneLane);
      else if (group) toggle(f.overview.groupsOpen, group);
      else if (panelDone) toggle(f.mainline.showDone, panelDone);
      draw(state.view);
      // the clicked part was drawn again; keep the keyboard where it was
      const again = selector && section(state.view).querySelector(selector);
      if (again && again.focus) again.focus();
    }

    /** Unfold, one step at a time, whatever hides the card of `key` in the active view. Returns whether it is drawn. */
    function ensureVisible(key) {
      const n = model.nodes.get(key);
      const f = state.fold;
      const drawn = () => Boolean(section(state.view) && section(state.view).querySelector(`.pm-card[data-key="${quote(key)}"]`));
      if (!n) return false;
      if (drawn()) return true;
      let steps = [];
      if (state.view === 'overview') {
        const group = `${n.lane}|${n.vis}`;
        steps = [
          () => f.overview.collapsed.delete(n.lane),
          () => n.vis === 'done' && !f.overview.doneOpen.has(n.lane) && Boolean(f.overview.doneOpen.add(n.lane)),
          () => targets.overviewGroups.includes(group) && !f.overview.groupsOpen.has(group) && Boolean(f.overview.groupsOpen.add(group)),
        ];
      } else if (state.view === 'mainline') {
        steps = [
          () => f.mainline.collapsed.delete(n.lane),
          () => n.vis === 'done' && targets.mainlineDone.includes(n.lane) && !f.mainline.showDone.has(n.lane) && Boolean(f.mainline.showDone.add(n.lane)),
        ];
      } else if (exploreIndex.has(n.milestone)) {
        const at = exploreIndex.get(n.milestone);
        steps = [
          () => f.explore.collapsed.delete(at),
          () => n.vis === 'done' && !f.explore.showDone && Boolean((f.explore.showDone = true)),
        ];
      }
      for (const step of steps) {
        if (step()) {
          draw(state.view);
          if (drawn()) return true;
        }
      }
      return drawn();
    }

    // ------------------------------------------------------------------ actions

    function setView(id) {
      if (!VIEW_IDS.includes(id)) return;
      state.view = id;
      const input = container.querySelector(`#v-${id}`);
      if (input) input.checked = true;
      applyEmphasis();
    }

    function setZoom(id, value, anchor) {
      const before = state.zoom[id];
      state.zoom[id] = Math.max(0.2, Math.min(3, value));
      applyZoom(id);
      const target = section(id);
      if (anchor && target && before > 0 && state.zoom[id] !== before) {
        // keep the point under the cursor where it was
        const rect = target.getBoundingClientRect ? target.getBoundingClientRect() : { left: 0 };
        const offset = anchor.clientX - rect.left;
        target.scrollLeft = ((target.scrollLeft + offset) / before) * state.zoom[id] - offset;
      }
    }

    const stepZoom = (current, direction) => {
      const nearest = ZOOM_STEPS.reduce((best, z, i) => (Math.abs(z - current) < Math.abs(ZOOM_STEPS[best] - current) ? i : best), 0);
      return ZOOM_STEPS[Math.max(0, Math.min(ZOOM_STEPS.length - 1, nearest + direction))];
    };

    function copyText(text) {
      const done = () => {
        const line = container.querySelector('[data-pm-status-line]');
        if (line) line.textContent = t('map.detail.copied');
      };
      if (copy) {
        Promise.resolve(copy(text)).then(done, () => {});
        return;
      }
      const nav = win.navigator;
      if (nav && nav.clipboard && nav.clipboard.writeText) nav.clipboard.writeText(text).then(done, () => {});
    }

    /** Show only the work of one agent: the chip sets the search to the agent, and sets it free again when it is pressed once more. */
    function filterAgent(name) {
      state.search = state.search.trim() === name ? '' : name;
      matchCursor = -1;
      const input = container.querySelector('[data-pm-search]');
      if (input) input.value = state.search;
      applyEmphasis();
    }

    function clearAll() {
      state.selected = null;
      state.hover = null;
      state.search = '';
      state.filters = new Set();
      const input = container.querySelector('[data-pm-search]');
      if (input) input.value = '';
      applyEmphasis();
    }

    /** Scroll the card of `key` into view in the active view (unfolding what hides it) and pulse it. */
    function reveal(key) {
      const line = container.querySelector('[data-pm-status-line]');
      if (!ensureVisible(key)) {
        if (line) line.textContent = t('map.detail.not_drawn');
        return false;
      }
      if (line) line.textContent = '';
      const card = section(state.view).querySelector(`.pm-card[data-key="${quote(key)}"]`);
      if (card.scrollIntoView) card.scrollIntoView({ block: 'center', inline: 'center' });
      card.classList.add('pm-pulse');
      const id = timers.set(() => { card.classList.remove('pm-pulse'); pulses.delete(id); }, 2400);
      pulses.add(id);
      return true;
    }

    function runAction(name) {
      const f = state.fold;
      const view = state.view;
      switch (name) {
        case 'zoom-in': setZoom(view, stepZoom(state.zoom[view], 1)); return;
        case 'zoom-out': setZoom(view, stepZoom(state.zoom[view], -1)); return;
        case 'zoom-100': setZoom(view, 1); return;
        case 'zoom-fit': {
          const target = section(view);
          const svg = target && target.querySelector('svg');
          const box = svg && svg.getAttribute('viewBox') ? svg.getAttribute('viewBox').trim().split(/\s+/).map(Number) : null;
          const room = target && target.clientWidth;
          setZoom(view, box && room ? Math.floor((room / box[2]) * 100) / 100 : 1);
          return;
        }
        case 'collapse-all':
          if (view === 'overview') f.overview.collapsed = new Set(targets.overviewLanes);
          else if (view === 'mainline') f.mainline.collapsed = new Set(targets.mainlinePanels);
          else f.explore.collapsed = new Set(targets.exploreLanes);
          draw(view);
          return;
        case 'expand-all':
          if (view === 'overview') { f.overview.collapsed = new Set(); f.overview.groupsOpen = new Set(targets.overviewGroups); }
          else if (view === 'mainline') f.mainline.collapsed = new Set();
          else f.explore.collapsed = new Set();
          draw(view);
          return;
        case 'toggle-done': {
          const open = doneIsOpen();
          if (view === 'overview') f.overview.doneOpen = open ? new Set() : new Set(targets.overviewDone);
          else if (view === 'mainline') f.mainline.showDone = open ? new Set() : new Set(targets.mainlineDone);
          else f.explore.showDone = !open;
          draw(view);
          return;
        }
        case 'clear': clearAll(); return;
        case 'reveal': if (state.drawer) reveal(state.drawer); return;
        case 'copy-key': if (state.drawer) copyText(state.drawer); return;
        case 'copy-command': if (state.drawer) copyText(detailLib.commandFor(state.drawer)); return;
        default:
      }
    }

    // ------------------------------------------------------------------ details panel

    const drawer = () => container.querySelector('.pm-drawer');

    /** The panel starts below the host's own bar, wherever that ends now. */
    function placeDrawer() {
      const box = drawer();
      if (box && box.style && offsetTop) box.style.setProperty('--pm-top', `${Math.max(0, Math.round(offsetTop()))}px`);
    }

    function openDetails(key, from) {
      const box = drawer();
      const data = detailLib.detailData(model, key, t, index);
      if (!box || !data) return false;
      if (from) returnTo = { el: from, key: attr(from, 'data-key') };
      placeDrawer();
      state.drawer = key;
      state.selected = key;
      box.querySelector('.pm-drawer-body').innerHTML = detailLib.renderDetail(data, t, (g) => pageLib.gapReason(t, g));
      const panel = box.querySelector('.pm-drawer-panel');
      if (panel) panel.setAttribute('aria-labelledby', 'pm-drawer-title');
      box.hidden = false;
      container.classList.add('pm-drawer-open');
      applyEmphasis();
      const close = box.querySelector('.pm-drawer-close');
      if (close && close.focus) close.focus();
      return true;
    }

    function closeDetails() {
      const box = drawer();
      if (!box || !state.drawer) return false;
      box.hidden = true;
      state.drawer = null;
      container.classList.remove('pm-drawer-open');
      applyEmphasis();
      restoreFocus();
      return true;
    }

    /** Give the keyboard back to the card or row that opened the panel, drawn again or not. */
    function restoreFocus() {
      const back = returnTo;
      returnTo = null;
      if (!back) return;
      let el = back.el && container.contains(back.el) ? back.el : null;
      if (!el && back.key) {
        const view = section(state.view);
        el = (view && view.querySelector(`.pm-card[data-key="${quote(back.key)}"]`)) || container.querySelector(`tr[data-key="${quote(back.key)}"]`);
      }
      if (el && el.focus) el.focus();
    }

    // ------------------------------------------------------------------ events

    function activate(el, event) {
      const jump = el.closest('[data-pm-jump]');
      if (jump) {
        const key = attr(jump, 'data-pm-jump');
        openDetails(key);
        reveal(key);
        return true;
      }
      const action = el.closest('[data-pm-action]');
      if (action) { runAction(attr(action, 'data-pm-action')); return true; }
      const chip = el.closest('[data-pm-status]');
      if (chip) {
        toggle(state.filters, attr(chip, 'data-pm-status'));
        applyEmphasis();
        return true;
      }
      if (el.closest('[data-pm-close]')) { closeDetails(); return true; }
      const agent = el.closest('.pm-agent');
      if (agent) { filterAgent(attr(agent, 'data-agent')); return true; }
      const fold = el.closest(FOLDABLE);
      if (fold) { foldClick(fold); return true; }
      const card = el.closest('.pm-card');
      if (card) { openDetails(attr(card, 'data-key'), card); return true; }
      const row = el.closest('tr[data-key]');
      if (row) { openDetails(attr(row, 'data-key'), row); return true; }
      // an empty spot of a diagram lets go of the highlighted chain (an open panel stays until it is closed)
      if (event && event.type === 'click' && el.closest('svg') && state.selected && !state.drawer) {
        state.selected = null;
        applyEmphasis();
      }
      return false;
    }

    const inField = (el) => Boolean(el.closest && el.closest('input,textarea,select'));

    on(container, 'click', (event) => {
      if (suppressClick) { suppressClick = false; return; }
      activate(event.target, event);
    });

    on(container, 'keydown', (event) => {
      const el = event.target;
      if (el.closest && el.closest('[data-pm-search]')) {
        if (event.key === 'Enter') {
          event.preventDefault();
          const cards = [...container.querySelectorAll(`#s-${state.view} .pm-card.pm-match`)];
          if (cards.length) {
            matchCursor = (matchCursor + 1) % cards.length;
            if (cards[matchCursor].scrollIntoView) cards[matchCursor].scrollIntoView({ block: 'center', inline: 'center' });
          } else {
            const first = [...matching(model, state.search, t)].sort(compare)[0];
            if (first) reveal(first);
          }
        }
        return;
      }
      if (inField(el)) return;
      if (event.key === 'Enter' || event.key === ' ') {
        if (el.closest && el.closest(ACTIVATABLE)) {
          event.preventDefault();
          activate(el, event);
        }
        return;
      }
      if (event.key === '/' && !event.ctrlKey && !event.metaKey) {
        const input = container.querySelector('[data-pm-search]');
        if (input && input.focus) { event.preventDefault(); input.focus(); }
        return;
      }
      if (event.key === '+' || event.key === '=') runAction('zoom-in');
      else if (event.key === '-') runAction('zoom-out');
      else if (event.key === '0') runAction('zoom-100');
    });

    // Esc works wherever the focus is, as long as this map is on screen.
    on(doc, 'keydown', (event) => {
      if (event.key !== 'Escape' || (container.closest && container.closest('[hidden]'))) return;
      if (closeDetails()) { event.preventDefault(); return; }
      if (state.search || state.selected || state.filters.size) { clearAll(); event.preventDefault(); }
    });

    on(container, 'input', (event) => {
      if (event.target.closest && event.target.closest('[data-pm-search]')) {
        state.search = event.target.value || '';
        matchCursor = -1;
        applyEmphasis();
      }
    });

    on(container, 'change', (event) => {
      const radio = event.target.closest && event.target.closest('input[name="view"]');
      if (radio) setView(String(attr(radio, 'id') || '').replace(/^v-/, ''));
    });

    if (win.addEventListener) on(win, 'resize', () => { if (state.drawer) placeDrawer(); });

    const cardAt = (event) => (event.target && event.target.closest ? event.target.closest('.pm-card') : null);
    const hoverCard = (card) => {
      const key = card ? attr(card, 'data-key') : null;
      if (key !== state.hover) {
        state.hover = key;
        if (!state.selected) applyEmphasis();
      }
    };
    on(container, 'mouseover', (event) => hoverCard(cardAt(event)));
    on(container, 'mouseout', (event) => {
      if (cardAt(event) && !(event.relatedTarget && event.relatedTarget.closest && event.relatedTarget.closest('.pm-card'))) hoverCard(null);
    });
    on(container, 'focusin', (event) => hoverCard(cardAt(event)));
    on(container, 'focusout', (event) => { if (cardAt(event)) hoverCard(null); });

    // drag the background of a diagram to move it; ctrl/cmd + wheel (or a pinch) zooms around the pointer
    function endPan() {
      if (!pan) return;
      doc.removeEventListener('mousemove', movePan);
      doc.removeEventListener('mouseup', endPan);
      pan.scroll.classList.remove('pm-panning');
      if (pan.moved) {
        suppressClick = true;
        timers.set(() => { suppressClick = false; }, 0);
      }
      pan = null;
    }
    function movePan(event) {
      if (!pan) return;
      const dx = event.clientX - pan.x;
      const dy = event.clientY - pan.y;
      if (!pan.moved && Math.abs(dx) < PAN_THRESHOLD && Math.abs(dy) < PAN_THRESHOLD) return;
      pan.moved = true;
      pan.scroll.classList.add('pm-panning');
      pan.scroll.scrollLeft = pan.left - dx;
      if (win.scrollBy) win.scrollBy(0, pan.lastY - event.clientY);
      pan.lastY = event.clientY;
    }
    on(container, 'mousedown', (event) => {
      if (event.button !== 0 || !event.target.closest) return;
      const scroll = event.target.closest('.scroll');
      if (!scroll || !event.target.closest('svg') || event.target.closest('.pm-card,.pm-fold,.pm-btn,.pm-agent')) return;
      pan = { scroll, x: event.clientX, y: event.clientY, lastY: event.clientY, left: scroll.scrollLeft, moved: false };
      doc.addEventListener('mousemove', movePan);
      doc.addEventListener('mouseup', endPan);
    });
    on(container, 'wheel', (event) => {
      if (!(event.ctrlKey || event.metaKey) || !event.target.closest || !event.target.closest('svg')) return;
      event.preventDefault();
      setZoom(state.view, stepZoom(state.zoom[state.view], event.deltaY < 0 ? 1 : -1), event);
    }, { passive: false });

    // ------------------------------------------------------------------ start

    const bar = container.querySelector('.pm-toolbar');
    if (bar) {
      bar.innerHTML = toolbarHtml();
      bar.hidden = false;
    }
    for (const row of container.querySelectorAll('tr[data-key]')) {
      row.setAttribute('tabindex', '0');
      row.setAttribute('role', 'button');
      row.setAttribute('aria-label', t('map.ui.row_open', { key: attr(row, 'data-key') }));
    }
    container.classList.add('pm-scope');
    const checked = container.querySelector('input[name="view"][checked]');
    state.view = checked ? String(attr(checked, 'id') || '').replace(/^v-/, '') || 'overview' : 'overview';
    for (const id of VIEW_IDS) {
      const target = section(id);
      if (target) target.classList.add('pm-live');
    }

    if (restore) {
      // continue from an earlier mount: the folds, zoom, filters and search survive a refresh of the map
      for (const [name, value] of Object.entries(restore.fold.overview)) state.fold.overview[name] = new Set(value);
      for (const [name, value] of Object.entries(restore.fold.mainline)) state.fold.mainline[name] = new Set(value);
      state.fold.explore = { collapsed: new Set(restore.fold.explore.collapsed), showDone: Boolean(restore.fold.explore.showDone) };
      Object.assign(state.zoom, restore.zoom);
      state.filters = new Set(restore.filters);
      state.search = restore.search || '';
      if (VIEW_IDS.includes(restore.view)) setView(restore.view);
      const input = container.querySelector('[data-pm-search]');
      if (input) input.value = state.search;
      for (const id of VIEW_IDS) draw(id);
    } else {
      for (const id of VIEW_IDS) decorate(id);
    }

    function destroy() {
      endPan();
      for (const [target, type, fn, options] of listeners) target.removeEventListener(type, fn, options);
      listeners.length = 0;
      for (const id of pulses) timers.clear(id);
      pulses.clear();
    }

    return {
      state, model, targets, destroy, draw, setView, openDetails, closeDetails, reveal, runAction,
      select: (key) => { state.selected = key; applyEmphasis(); },
      matching: (text) => matching(model, text, t),
    };
  }

  const api = { mount, createState, foldTargets, matching, VIEW_IDS, ZOOM_STEPS };
  if (commonJS) module.exports = api;
  else {
    root.AWR_PROJECT_MAP = root.AWR_PROJECT_MAP || {};
    root.AWR_PROJECT_MAP.interact = api;
  }
})(typeof globalThis !== 'undefined' ? globalThis : this);
