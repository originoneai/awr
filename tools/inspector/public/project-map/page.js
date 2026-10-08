/**
 * Project-map page assembly: the three SVG views, the data table and the notes, both as parts for the Inspector page and
 * as one self-contained HTML document for the static export.
 *
 * `renderProjectMap(snapshot, config, {t, lang, interactive, runtime})` is a pure function: the same arguments always give
 * the same bytes. Without `runtime` the document has inline SVG and inline CSS, no script, no external request and no font
 * download; the tabs are CSS-only radio buttons, so the file also works offline and in a mail attachment. With `runtime`
 * (the module sources and stylesheet of the interaction layer plus the messages) the same document also carries the
 * interactive layer inline: details panel, folding, highlighting, search and zoom. The static markup stays in the file, so
 * a viewer that does not run scripts still shows everything.
 */
(function (root) {
  'use strict';

  const commonJS = typeof module !== 'undefined' && module.exports;
  const kitLib = commonJS ? require('./svgkit.js') : root.AWR_PROJECT_MAP.svgkit;
  const modelLib = commonJS ? require('./model.js') : root.AWR_PROJECT_MAP.model;
  const viewsLib = commonJS ? require('./views.js') : root.AWR_PROJECT_MAP.views;
  const { esc, compare } = kitLib;

  const VIEW_IDS = ['overview', 'mainline', 'explore'];

  /** Standalone-document styles. The Inspector page uses its own stylesheet and only embeds the parts. */
  const CSS = `
:root{color-scheme:light dark;--bg:#f2f5fb;--ink:#1d1d1f;--muted:#6e6e73;--faint:#86868b;--line:rgba(0,0,0,.1);--panel:#fff;--chip:#fff;--accent:#0a5cff;--shadow:0 10px 30px rgba(13,48,120,.12);--scroll:#e9eff9}
@media (prefers-color-scheme:dark){:root{--bg:#10131a;--ink:#eceef2;--muted:#a3a8b3;--faint:#8b909c;--line:rgba(255,255,255,.14);--panel:#1b2030;--chip:#1b2030;--accent:#7fb0ff;--shadow:0 10px 30px rgba(0,0,0,.45);--scroll:#1b2030}}
*{box-sizing:border-box}
body{margin:0;background:var(--bg);color:var(--ink);font:14px/1.55 -apple-system,BlinkMacSystemFont,"SF Pro Text","Segoe UI","PingFang SC","Microsoft YaHei",sans-serif}
.top{display:flex;flex-wrap:wrap;gap:16px 28px;align-items:center;justify-content:space-between;padding:22px max(16px,3vw) 8px}
.brand{display:flex;gap:14px;align-items:center;min-width:0}
.badge{flex:none;width:44px;height:44px;border-radius:13px;background:linear-gradient(135deg,#57b8ff,#0a5cff);color:#fff;font-weight:700;font-size:20px;display:grid;place-items:center}
h1{font-size:20px;line-height:1.25;margin:0}
.sub{margin:2px 0 0;color:var(--muted);font-size:12.5px;overflow-wrap:anywhere}
.stats{display:flex;gap:10px;list-style:none;padding:0;margin:0;flex-wrap:wrap}
.stats li{background:var(--chip);border:1px solid var(--line);border-radius:14px;padding:6px 14px;text-align:center;min-width:72px}
.stats b{display:block;font-size:18px}.stats span{color:var(--faint);font-size:11.5px}
.tabs{padding:6px max(16px,3vw) 0}
.tabs>input{position:absolute;opacity:0;pointer-events:none}
.tabs>label{display:inline-block;margin:0 8px 10px 0;padding:7px 16px;border-radius:999px;border:1px solid var(--line);background:var(--chip);color:var(--muted);cursor:pointer;font-weight:600;font-size:13px}
.tabs>input:checked+label{background:var(--accent);border-color:var(--accent);color:#fff}
.tabs>input:focus-visible+label{outline:2px solid var(--accent);outline-offset:2px}
.view{display:none}
#v-overview:checked~.views #s-overview,#v-mainline:checked~.views #s-mainline,#v-explore:checked~.views #s-explore{display:block}
.scroll{overflow-x:auto;max-width:100%;border-radius:18px;background:var(--scroll);box-shadow:var(--shadow);border:1px solid var(--line)}
.scroll svg{display:block;margin:0 auto}
.table,.notes{margin:18px max(16px,3vw)}
.table summary{cursor:pointer;font-weight:600;padding:8px 0}
table{border-collapse:collapse;width:100%;font-size:12.5px;min-width:760px}
th,td{padding:6px 10px;border-bottom:1px solid var(--line);text-align:left;vertical-align:top}
th{position:sticky;top:0;background:var(--panel);color:var(--muted);font-weight:600}
td.k{font-family:ui-monospace,SFMono-Regular,Menlo,Consolas,monospace;white-space:nowrap}
.notes{color:var(--muted);font-size:12.5px}.notes h2{font-size:13px;margin:12px 0 4px;color:var(--ink)}.notes ul{margin:4px 0;padding-left:20px}
@media (max-width:640px){h1{font-size:17px}.stats li{min-width:64px;padding:5px 10px}}
@media print{.tabs>input,.tabs>label{display:none}.view{display:block!important;break-inside:avoid;margin-bottom:16px}.scroll{box-shadow:none;overflow:visible}}
`.trim();

  /** Language function over the catalogs: unknown keys fall back to English, then to the key itself. */
  function makeT(catalogs, lang) {
    const has = (catalog, name) => Object.prototype.hasOwnProperty.call(catalog || {}, name);
    return (key, params = {}) => {
      const pattern = has(catalogs[lang], key) ? catalogs[lang][key] : has(catalogs.en, key) ? catalogs.en[key] : key;
      return pattern.replace(/\{([A-Za-z0-9_]+)\}/g, (placeholder, name) => (Object.prototype.hasOwnProperty.call(params, name) ? String(params[name]) : placeholder));
    };
  }

  /** Make element ids unique per view so the three SVGs can share one document. */
  function namespace(svg, prefix) {
    const ids = [...new Set([...svg.matchAll(/\bid="([^"]+)"/g)].map((m) => m[1]))].sort((a, b) => b.length - a.length || compare(a, b));
    let out = svg;
    for (const id of ids) out = out.split(`id="${id}"`).join(`id="${prefix}${id}"`).split(`url(#${id})`).join(`url(#${prefix}${id})`);
    return out;
  }

  /** A drawn view as the page shows it: element ids unique to the view and an accessible name on the drawing. */
  function finishView(id, svg, label) {
    return namespace(svg, `${id}-`).replace('role="img"', `role="img" aria-label="${esc(label)}"`);
  }

  /** Localized explanation of a missing field; falls back to the reason the snapshot carries. */
  function gapReason(t, entry) {
    const key = `map.gap.${entry.field.replace(/\./g, '_')}`;
    const text = t(key);
    return text === key ? entry.reason : text;
  }

  /** The modules the interaction layer needs in the browser, in load order, under public/project-map/. */
  const RUNTIME_FILES = ['snapshot.js', 'model.js', 'svgkit.js', 'layout.js', 'views.js', 'detail.js', 'page.js', 'interact.js'];

  /** The `map.*` messages an exported file needs: the chosen language, with English as the fallback. */
  function runtimeMessages(catalogs, lang) {
    const pick = (catalog) => Object.fromEntries(Object.entries(catalog || {}).filter(([key]) => key.startsWith('map.')));
    return lang === 'en' ? { en: pick(catalogs.en) } : { en: pick(catalogs.en), [lang]: pick(catalogs[lang]) };
  }

  /** Starts the interaction layer of an exported file from the data block next to it. */
  const BOOT = "(function(){var P=globalThis.AWR_PROJECT_MAP,d=JSON.parse(document.getElementById('pm-data').textContent);"
    + "P.interact.mount({container:document.getElementById('pm-root'),snapshot:d.snapshot,config:d.config||undefined,t:P.page.makeT(d.catalogs,d.lang)});})();";

  /**
   * The data and script of an interactive export: the snapshot and configuration as JSON (a `<` is escaped, so the data
   * cannot end its own element), then the module sources and the start-up call. A source that could not sit inside a script
   * element is refused instead of being rewritten.
   */
  function runtimeBlock({ snapshot, userConfig, lang, runtime }) {
    for (const source of runtime.sources) {
      if (/<\/script|<!\-\-/i.test(source)) throw new Error('a runtime source cannot be placed inside a script element');
    }
    const data = JSON.stringify({ lang, snapshot, config: userConfig === undefined ? null : userConfig, catalogs: runtime.catalogs }).replace(/</g, '\\u003c');
    // the closing tag is assembled here so that this very file (which is one of the inlined sources) never contains it
    const end = '<' + '/script>';
    return `<script type="application/json" id="pm-data">${data}${end}\n<script>\n${runtime.sources.join('\n')}\n${BOOT}\n${end}`;
  }

  function renderProjectMap(snapshot, userConfig, { t, lang = 'en', interactive = false, runtime = null }) {
    const live = interactive || Boolean(runtime);
    const model = modelLib.loadModel(snapshot, userConfig);
    const { nodes, meta, cfg } = model;
    const views = viewsLib.createViews(t);
    const kit = kitLib.createKit(t);
    const svgs = [views.renderOverview(model), views.renderMainline(model), views.renderExplore(model)];
    const all = [...nodes.values()];
    const edges = all.reduce((s, n) => s + n.deps.length, 0);
    const tot = {};
    for (const n of all) tot[n.vis] = (tot[n.vis] || 0) + 1;
    const proj = meta.project;
    const names = { overview: t('map.view.overview'), mainline: t('map.view.mainline'), explore: t('map.view.explore') };
    const labels = {
      overview: t('map.view.overview_label', { nodes: nodes.size, edges }),
      mainline: t('map.view.mainline_label'),
      explore: t('map.view.explore_label'),
    };
    const parts = VIEW_IDS.map((id, i) => ({
      id,
      name: names[id],
      label: labels[id],
      svg: finishView(id, svgs[i], labels[id]),
    }));

    const laneName = new Map(cfg.overview.lanes.map((l) => [l.id, modelLib.laneName(l, t)]));
    const laneIndex = new Map(cfg.overview.lanes.map((l, i) => [l.id, i]));
    const rows = [...all].sort((a, b) => (laneIndex.get(a.lane) ?? 99) - (laneIndex.get(b.lane) ?? 99) || compare(a.id, b.id)).map((n) => {
      const idle = n.idle === null ? '—' : kitLib.f0(n.idle);
      return `<tr data-key="${esc(n.id)}" data-vis="${n.vis}"><td class="k">${esc(n.id)}</td><td>${esc(n.title)}</td><td title="${esc(n.status)}">${esc(kit.ST[n.vis].label)}</td>`
        + `<td>${esc(laneName.get(n.lane) || n.lane)}</td><td>${esc(n.milestone || '—')}</td><td>${esc(n.owner || '—')}</td><td>${idle}</td></tr>`;
    });
    const table = `<details class="table"><summary>${esc(t('map.table.summary', { count: nodes.size }))}</summary><div class="scroll"><table><thead><tr>`
      + ['key', 'title', 'status', 'lane', 'milestone', 'owner', 'idle'].map((c) => `<th>${esc(t(`map.table.${c}`))}</th>`).join('')
      + `</tr></thead><tbody>${rows.join('')}</tbody></table></div></details>`;

    const time = viewsLib.utc(meta.generated_at);
    const gaps = meta.unavailable.map((u) => `<li><code>${esc(u.field)}</code>: ${esc(gapReason(t, u))}</li>`).join('');
    const notes = `<section class="notes"><h2>${esc(t('map.notes.basis'))}</h2><p>${t('map.notes.basis_text', {
      revision: esc(String(proj.revision)), fingerprint: `<code>${esc(meta.fingerprint)}</code>`, time: esc(time),
    })}</p>`
      + `<h2>${esc(t('map.notes.rules'))}</h2><ul><li>${esc(t('map.notes.rule_stalled', { days: cfg.stale_days }))}</li><li>${esc(t('map.notes.rule_lanes'))}</li><li>${esc(t('map.notes.rule_claims'))}</li></ul>`
      + `<h2>${esc(t('map.notes.gaps'))}</h2><ul>${gaps}</ul></section>`;

    const title = cfg.titles.page || t('map.page.title', { name: proj.name });
    const stats = [[nodes.size, t('map.stat.items')], [edges, t('map.stat.edges')], [tot.stalled || 0, t('map.status.stalled')],
      [tot.blocked || 0, t('map.status.blocked')], [meta.sessions_active, t('map.stat.sessions')]];
    const sub = t('map.page.sub', { revision: esc(String(proj.revision)), fingerprint: esc(meta.fingerprint.slice(7, 19)), time: esc(time) });
    const header = `<header class="top"><div class="brand"><span class="badge" aria-hidden="true">A</span><div><h1>${esc(title)}</h1><p class="sub">${sub}</p></div></div>`
      + `<ul class="stats">${stats.map(([n, label]) => `<li><b>${n}</b><span>${esc(label)}</span></li>`).join('')}</ul></header>`;

    const inputs = parts.map((p, i) => `<input type="radio" name="view" id="v-${p.id}"${i === 0 ? ' checked' : ''}><label for="v-${p.id}">${esc(p.name)}</label>`).join('');
    const main = parts.map((p) => `<section class="view" id="s-${p.id}" aria-label="${esc(p.name)}"><div class="scroll">${p.svg}</div></section>`).join('');
    // Places for the toolbar and the details panel. They are empty and hidden: the interaction layer fills them, so a viewer
    // without script sees the same page as before.
    const toolbar = live ? '<div class="pm-toolbar" hidden></div>' : '';
    const panel = live
      ? `\n<aside class="pm-drawer" hidden><div class="pm-drawer-panel" role="dialog" aria-modal="false" aria-label="${esc(t('map.detail.dialog'))}" tabindex="-1">`
        + `<button type="button" class="pm-drawer-close" data-pm-close aria-label="${esc(t('map.detail.close'))}">\u00d7</button><div class="pm-drawer-body"></div></div></aside>`
      : '';
    // The Inspector page and the export share this block, so the page and the saved file show the same thing.
    const body = `<div class="tabs">${inputs}${toolbar}<div class="views">${main}</div></div>\n${table}\n${notes}${panel}`;
    const css = runtime ? `${CSS}\n${runtime.css}` : CSS;
    const content = runtime ? `<main class="pm-root pm-scope" id="pm-root">${body}</main>\n${runtimeBlock({ snapshot, userConfig, lang, runtime })}` : `<main>${body}</main>`;
    const document = `<!doctype html>\n<html lang="${esc(lang)}">\n<head>\n<meta charset="utf-8">\n<meta name="viewport" content="width=device-width, initial-scale=1">\n<meta name="color-scheme" content="light dark">\n`
      + `<title>${esc(title)} · r${esc(String(proj.revision))}</title>\n<style>${css}</style>\n</head>\n<body>\n${header}\n${content}\n</body>\n</html>\n`;

    return { document, title, header, body, views: parts, table, notes, stats, model, fingerprint: snapshot.fingerprint, revision: proj.revision };
  }

  const api = { CSS, VIEW_IDS, RUNTIME_FILES, makeT, namespace, finishView, gapReason, runtimeMessages, renderProjectMap };
  if (commonJS) module.exports = api;
  else {
    root.AWR_PROJECT_MAP = root.AWR_PROJECT_MAP || {};
    root.AWR_PROJECT_MAP.page = api;
  }
})(typeof globalThis !== 'undefined' ? globalThis : this);
