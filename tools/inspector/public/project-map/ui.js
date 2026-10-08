/**
 * Inspector page controller for the Project map.
 *
 * It reads the project through the bridge's read-only /api/map routes (one official awr command per request), builds the
 * snapshot in the browser with the same extractor the export uses, draws it with the same renderer and makes the drawing
 * interactive with the same interaction layer (details panel, folding, highlight, search, zoom). In demo mode it shows the
 * built-in demonstration project instead. Nothing here reads project files.
 */
(function (root) {
  'use strict';

  const commonJS = typeof module !== 'undefined' && module.exports;
  const L = commonJS
    ? {
      snapshot: require('./snapshot.js'), model: require('./model.js'), page: require('./page.js'), extract: require('./extract.js'), demo: require('./demo.js'), interact: require('./interact.js'),
    }
    : root.AWR_PROJECT_MAP;
  const LOCALES = commonJS ? { en: require('../locales/en.js'), 'zh-CN': require('../locales/zh-CN.js') } : root.AWR_LOCALES;
  const { AwrCallError, extractSnapshot } = L.extract;

  /** Bridge route of each extractor command. */
  const ROUTES = {
    workGraph: (p) => `/api/map/work-graph?limit=${encodeURIComponent(p.limit)}${p.cached ? '&cached=1' : ''}`,
    nav: (p) => (p.milestone ? `/api/map/nav?milestone=${encodeURIComponent(p.milestone)}` : '/api/map/nav'),
    goals: () => '/api/map/goals',
    events: (p) => {
      const q = [`limit=${encodeURIComponent(p.limit)}`];
      if (p.through !== undefined && p.through !== null) q.push(`through=${encodeURIComponent(p.through)}`);
      if (p.cursor) q.push(`cursor=${encodeURIComponent(JSON.stringify(p.cursor))}`);
      return `/api/map/events?${q.join('&')}`;
    },
    sessions: () => '/api/map/sessions',
    doctor: () => '/api/map/doctor',
  };

  /** Extractor `call` over the bridge envelope {ok, data|error}. */
  function bridgeCall(callApi) {
    return async (name, params) => {
      const envelope = await callApi(ROUTES[name](params || {}));
      if (envelope && envelope.ok) return envelope.data;
      // doctor exits nonzero when it has findings and still returns its report
      if (name === 'doctor' && envelope && envelope.data && Array.isArray(envelope.data.findings)) return envelope.data;
      const e = (envelope && envelope.error) || { code: 'BridgeError', message: 'no response from the bridge' };
      throw new AwrCallError(e.code || 'BridgeError', e.message || '', e.details || null);
    };
  }

  function browserSave(doc) {
    return (name, mime, text) => {
      const url = URL.createObjectURL(new Blob([text], { type: mime }));
      const a = doc.createElement('a');
      a.href = url;
      a.download = name;
      doc.body.appendChild(a);
      a.click();
      a.remove();
      setTimeout(() => URL.revokeObjectURL(url), 1000);
    };
  }

  /**
   * `interact` is the interaction layer (null draws the diagrams only), `catalogs` the language catalogs and `fetchSource`
   * how the module files of a downloaded interactive file are read (the files the page itself runs; without it the download
   * is the static file).
   */
  function createProjectMap({
    i18n, $, callApi, document: doc = root.document, now = () => Date.now(), clock, save, extractOptions = {},
    interact = L.interact, catalogs = LOCALES, fetchSource = null,
  }) {
    const t = (key, params) => i18n.t(key, params);
    const saveFile = save || browserSave(doc);
    const state = { running: null, loadedOnce: false, demo: false, last: null, error: null };
    let mounted = null;
    let runtime = null;
    const el = (id) => $(id);
    const on = (id, type, fn) => {
      const node = el(id);
      if (node) node.addEventListener(type, fn);
    };

    function setStatus(message, kind) {
      const node = el('mapStatus');
      if (!node) return;
      node.textContent = message;
      node.setAttribute('data-kind', kind || 'info');
      node.hidden = !message;
    }

    /** The details panel starts below the page's sticky top bar (the Inspector's `.topbar`), when there is one. */
    function offsetTop() {
      const bar = doc && typeof doc.querySelector === 'function' ? doc.querySelector('.topbar') : null;
      return bar ? bar.getBoundingClientRect().bottom : 0;
    }

    function setBusy(busy) {
      const button = el('mapRefreshBtn');
      if (button) button.disabled = busy;
      const body = el('mapBody');
      if (body) body.setAttribute('aria-busy', String(busy));
    }

    function errorText(e) {
      if (e && e.name === 'ConfigError') return t('map.error.ConfigError', { message: e.message });
      const code = e && e.code ? e.code : 'Error';
      const specific = `map.error.${code}`;
      const text = t(specific);
      return text !== specific ? text : t('map.error.generic', { code, message: e && (e.detail || e.message) ? (e.detail || e.message) : String(e) });
    }

    function showError(e) {
      state.error = e;
      setStatus(t('map.error.prefix', { detail: errorText(e) }), 'error');
      const retry = el('mapRetryBtn');
      if (retry) retry.hidden = false;
      const body = el('mapBody');
      if (body) body.hidden = !state.last;
    }

    async function loadConfig() {
      const envelope = await callApi('/api/map/config');
      if (envelope && envelope.ok) return { demo: false, config: envelope.data.config || undefined, source: envelope.data.source };
      if (envelope && envelope.error && envelope.error.code === 'DemoMode') return { demo: true };
      const e = (envelope && envelope.error) || { code: 'BridgeError', message: 'no response from the bridge' };
      throw new AwrCallError(e.code || 'BridgeError', e.message || '');
    }

    function render(snapshot, config, notes) {
      setStatus(t('map.progress.render'), 'info');
      const result = L.page.renderProjectMap(snapshot, config, { t, lang: i18n.locale, interactive: Boolean(interact) });
      state.last = { snapshot, config, result };
      const header = el('mapHeader');
      if (header) header.innerHTML = result.header;
      const stage = el('mapStage');
      if (stage) {
        // the folds, zoom and search of the map on screen survive a refresh
        const previous = mounted ? mounted.state : undefined;
        if (mounted) mounted.destroy();
        mounted = null;
        stage.innerHTML = result.body;
        if (interact) mounted = interact.mount({ container: stage, snapshot, config, t, model: result.model, restore: previous, offsetTop });
      }
      const body = el('mapBody');
      if (body) body.hidden = false;
      for (const id of ['mapDownloadBtn', 'mapDownloadJsonBtn']) {
        const button = el(id);
        if (button) button.disabled = false;
      }
      const retry = el('mapRetryBtn');
      if (retry) retry.hidden = true;
      setStatus(notes.join(' '), 'info');
    }

    function refresh() {
      if (state.running) return state.running;
      const run = (async () => {
        setBusy(true);
        state.error = null;
        const started = now();
        try {
          const loaded = await loadConfig();
          let snapshot;
          let config;
          const notes = [];
          if (loaded.demo) {
            ({ snapshot, config } = L.demo.buildDemo(t));
            state.demo = true;
            notes.push(t('map.status.demo'));
          } else {
            config = loaded.config;
            const cached = Boolean(el('mapCachedToggle') && el('mapCachedToggle').checked);
            snapshot = await extractSnapshot({
              call: bridgeCall(callApi), config, cached, now: clock ? clock() : undefined,
              onProgress: (step, attempt) => setStatus(step === 'retry' ? t('map.progress.retry', { attempt }) : t(`map.progress.${step}`), 'info'),
              ...extractOptions,
            });
            state.demo = false;
            notes.push(t('map.status.loaded', { revision: snapshot.project.revision, nodes: snapshot.nodes.length, seconds: ((now() - started) / 1000).toFixed(1) }));
            if (cached) notes.push(t('map.status.recorded'));
            notes.push(loaded.source ? t('map.config.source', { name: loaded.source }) : t('map.config.derived'));
          }
          render(snapshot, config, notes);
        } catch (e) {
          showError(e);
        } finally {
          setBusy(false);
          state.running = null;
        }
      })();
      state.running = run;
      return run;
    }

    function show() {
      if (state.loadedOnce) return Promise.resolve();
      state.loadedOnce = true;
      return refresh();
    }

    function downloadName(extension) {
      return `awr-project-map-r${state.last.snapshot.project.revision}.${extension}`;
    }

    on('mapRefreshBtn', 'click', () => refresh());
    on('mapRetryBtn', 'click', () => refresh());
    /** The interaction layer as the exported file carries it: the same module files the page runs, read once. */
    async function loadRuntime() {
      if (!runtime) {
        if (!fetchSource) throw new Error('the module files cannot be read');
        const texts = await Promise.all(['interact.css', ...L.page.RUNTIME_FILES].map((name) => fetchSource(`project-map/${name}`)));
        runtime = { css: texts[0], sources: texts.slice(1) };
      }
      return { ...runtime, catalogs: L.page.runtimeMessages(catalogs, i18n.locale) };
    }

    async function downloadHtml() {
      if (!state.last) return;
      const { snapshot, config } = state.last;
      const options = { t, lang: i18n.locale };
      let html;
      try {
        html = interact ? L.page.renderProjectMap(snapshot, config, { ...options, runtime: await loadRuntime() }).document : L.page.renderProjectMap(snapshot, config, options).document;
      } catch (e) {
        // the page keeps working; the file is the static one and the status says so
        html = L.page.renderProjectMap(snapshot, config, options).document;
        setStatus(t('map.status.download_static'), 'info');
      }
      saveFile(downloadName('html'), 'text/html;charset=utf-8', html);
    }

    on('mapDownloadBtn', 'click', () => downloadHtml());
    on('mapDownloadJsonBtn', 'click', () => {
      if (state.last) saveFile(downloadName('json'), 'application/json;charset=utf-8', `${JSON.stringify(state.last.snapshot, null, 1)}\n`);
    });

    return { refresh, show, state, ROUTES, downloadHtml, get mounted() { return mounted; } };
  }

  const api = { createProjectMap, bridgeCall, ROUTES };
  if (commonJS) module.exports = api;
  else {
    root.AWR_PROJECT_MAP = root.AWR_PROJECT_MAP || {};
    root.AWR_PROJECT_MAP.ui = api;
  }
})(typeof globalThis !== 'undefined' ? globalThis : this);
