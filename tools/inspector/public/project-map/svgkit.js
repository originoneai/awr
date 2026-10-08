/**
 * SVG drawing kit of the project map: text metrics, status palette, cards, chips, edges and column flows.
 *
 * Everything is a pure function of its arguments and returns markup strings. Colors and sizes follow the AWR site
 * (glass cards, brand blue, one palette per status). Text comes through the `t` function given to `createKit`, so the
 * same drawing serves every UI language. No inline `style` attributes are produced: the pages that embed the result
 * run under a CSP that forbids them.
 */
(function (root) {
  'use strict';

  const commonJS = typeof module !== 'undefined' && module.exports;

  const FONT = "-apple-system,BlinkMacSystemFont,'SF Pro Display','Avenir Next','Segoe UI','PingFang SC','Microsoft YaHei',sans-serif";
  const MONO = "ui-monospace,SFMono-Regular,Menlo,Consolas,monospace";
  const INK = '#1d1d1f';
  const MUTED = '#6e6e73';
  const FAINT = '#86868b';

  /** Visual classes with their colors; labels are added per language by `createKit`. */
  const PALETTE = {
    done: { dot: '#1fae74', fill: '#1fae74', ring: null, border: 'rgba(31,174,116,.38)', fg: '#12855c', bg: 'rgba(31,174,116,.09)', dash: null, op: 1, cardbg: '#ffffff' },
    developing: { dot: 'url(#gDev)', fill: 'url(#gDev)', ring: null, border: 'rgba(10,92,255,.5)', fg: '#0a5cff', bg: 'rgba(10,92,255,.08)', dash: null, op: 1, cardbg: '#ffffff', glow: true },
    stalled: { dot: '#f59e0b', fill: '#ffffff', ring: '#f59e0b', border: 'rgba(245,158,11,.65)', fg: '#b4740a', bg: 'rgba(245,158,11,.11)', dash: '5 3', op: 1, cardbg: '#fffdf7' },
    blocked: { dot: '#e14b54', fill: '#e14b54', ring: null, border: 'rgba(225,75,84,.58)', fg: '#c93b44', bg: 'rgba(225,75,84,.09)', dash: null, op: 1, cardbg: '#fff8f8' },
    ready: { dot: '#b8c0cf', fill: '#ffffff', ring: '#aeb7c7', border: 'rgba(0,0,0,.11)', fg: '#556075', bg: 'rgba(0,0,0,.045)', dash: null, op: 1, cardbg: '#ffffff' },
    cancelled: { dot: '#9aa3b2', fill: '#ffffff', ring: '#9aa3b2', border: 'rgba(0,0,0,.24)', fg: '#6e6e73', bg: 'rgba(0,0,0,.05)', dash: '3 3', op: 0.92, cardbg: '#f4f5f8' },
    draft: { dot: '#d6dbe4', fill: '#ffffff', ring: '#cdd3df', border: 'rgba(0,0,0,.12)', fg: '#86868b', bg: 'rgba(0,0,0,.03)', dash: '2 3', op: 0.7, cardbg: '#ffffff' },
    waiting: { dot: '#cfd5df', fill: '#ffffff', ring: '#c9d0dc', border: 'rgba(0,0,0,.16)', fg: '#86868b', bg: 'rgba(0,0,0,.035)', dash: '4 3', op: 0.74, cardbg: '#ffffff' },
  };
  /** Stacked-bar segment colors, in drawing order. */
  const BAR = [['done', '#1fae74'], ['developing', 'url(#gDev)'], ['stalled', '#f59e0b'], ['blocked', '#e14b54'], ['ready', '#c2c9d6'], ['waiting', '#e6e9f0'], ['draft', '#eff1f5'], ['cancelled', '#8d96a6']];

  /** Edge styles: stroke, width, extra attributes, arrow marker. */
  const EDGE = {
    dep: ['#8fb0e6', 1.6, '', 'aBlue'],
    crit: ['#0a5cff', 2.8, '', 'aCrit'],
    cross: ['#e14b54', 1.5, ' stroke-dasharray="4 4"', 'aRed'],
    rel: ['#9aa3b2', 1.4, ' stroke-dasharray="1.5 4" stroke-linecap="round"', 'aGray'],
    thin: ['#a9bfe6', 1.2, '', 'aBlue'],
    xlane: ['#7ea6e8', 1.3, ' stroke-dasharray="5 4"', 'aBlue'],
  };

  /**
   * Fixed-point text with Python's rounding: an exact tie (a multiple of 0.25 for one decimal) rounds to the even digit,
   * where toFixed rounds it up. Rendering then matches the reference drawings digit for digit.
   */
  function fixed(x, decimals) {
    const v = Number(x);
    const scale = decimals === 0 ? 2 : 4;
    const q = v * scale;
    if (Number.isInteger(q) && Math.abs(q % 2) === 1) {
      const lo = Math.floor(v * (decimals === 0 ? 1 : 10));
      return ((lo % 2 === 0 ? lo : lo + 1) / (decimals === 0 ? 1 : 10)).toFixed(decimals);
    }
    return v.toFixed(decimals);
  }
  const f1 = (x) => fixed(x, 1);
  const f0 = (x) => fixed(x, 0);
  const compare = (a, b) => (a < b ? -1 : a > b ? 1 : 0);

  /** HTML-escape text for markup and attribute values (single quotes included). */
  function esc(s) {
    return String(s).replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;').replace(/'/g, '&#x27;');
  }

  /** Rough text width: CJK is one em, Latin by character class (good enough for sizing cards). */
  function tw(s, size) {
    let w = 0;
    for (const ch of String(s)) {
      const o = ch.codePointAt(0);
      if (o >= 0x2E80) w += size;
      else if (ch === 'W' || ch === 'M' || ch === 'm' || ch === 'w' || ch === '@') w += size * 0.86;
      else if (o < 128 ? (o >= 65 && o <= 90) : /\p{Lu}/u.test(ch)) w += size * 0.68;
      else if (o < 128 ? (o >= 48 && o <= 57) : /\p{Nd}/u.test(ch)) w += size * 0.58;
      else if (' ilIj.,:;|!\'`()[]/'.includes(ch)) w += size * 0.31;
      else w += size * 0.56;
    }
    return w;
  }

  function fit(s, size, maxw) {
    const text = String(s);
    if (tw(text, size) <= maxw) return text;
    const chars = Array.from(text);
    while (chars.length && tw(`${chars.join('')}…`, size) > maxw) chars.pop();
    return `${chars.join('')}…`;
  }

  const TOKEN = /[\u2E80-\u9FFF\uFF00-\uFFEF\uFF0C\u3002\uFF1B\uFF1A\u3001\uFF08\uFF09]|[^\s\u2E80-\u9FFF\uFF00-\uFFEF\uFF0C\u3002\uFF1B\uFF1A\u3001\uFF08\uFF09]+|\s+/gu;

  /** Greedy wrap: CJK may break anywhere, Latin only at spaces. */
  function wrap(s, size, maxw, maxlines = 2) {
    const tokens = String(s).match(TOKEN) || [];
    let lines = [];
    let cur = '';
    for (const token of tokens) {
      if (tw(cur + token, size) <= maxw || !cur.trim()) cur += token;
      else {
        lines.push(cur.trimEnd());
        cur = token.trimStart();
      }
    }
    if (cur.trim()) lines.push(cur.trimEnd());
    if (lines.length > maxlines) {
      lines = lines.slice(0, maxlines);
      const last = lines[maxlines - 1];
      lines[maxlines - 1] = tw(`${last}…`, size) > maxw ? fit(`${last}…`, size, maxw) : `${last}…`;
    }
    return lines.length ? lines : [''];
  }

  /** Identifier in mono type; a middle ellipsis keeps the numeric tail visible. */
  function dispId(s, maxw, size = 9.8) {
    const cw = size * 0.6 + 0.3;
    const n = Math.floor(maxw / cw);
    const chars = Array.from(s);
    if (chars.length <= n) return s;
    const tail = 4;
    return `${chars.slice(0, Math.max(n - tail - 1, 3)).join('')}…${chars.slice(-tail).join('')}`;
  }

  function text(x, y, s, size = 12, fill = INK, weight = 400, anchor = 'start', opts = {}) {
    const fam = opts.mono ? ` font-family="${esc(MONO)}"` : '';
    const ls = opts.spacing !== undefined && opts.spacing !== null ? ` letter-spacing="${opts.spacing}"` : '';
    const op = opts.opacity !== undefined && opts.opacity !== null ? ` opacity="${opts.opacity}"` : '';
    return `<text x="${f1(x)}" y="${f1(y)}" font-size="${size}" fill="${fill}" font-weight="${weight}" text-anchor="${anchor}"${fam}${ls}${op}${opts.extra || ''}>${esc(s)}</text>`;
  }

  /** Rounded label. Returns [markup, width]. */
  function chip(x, y, s, fg, bg, stroke = null, size = 9.5, anchor = 'start', h = 17, weight = 600, pad = 7, mono = false) {
    const w = tw(s, size) + pad * 2;
    const x0 = anchor === 'start' ? x : anchor === 'end' ? x - w : x - w / 2;
    const st = stroke ? ` stroke="${stroke}" stroke-width="1"` : '';
    return [[`<rect x="${f1(x0)}" y="${f1(y)}" width="${f1(w)}" height="${h}" rx="${h / 2}" fill="${bg}"${st}/>`,
      text(x0 + w / 2, y + h / 2 + size * 0.35, s, size, fg, weight, 'middle', { mono })].join('\n'), w];
  }

  /** Transitive reduction: drop an edge when another path already implies it. Edges are [dependent, prerequisite]. */
  function transitiveReduction(nodes, edges) {
    const set = new Set(nodes);
    const prereq = new Map();
    for (const [a, b] of edges) {
      if (set.has(a) && set.has(b)) {
        if (!prereq.has(a)) prereq.set(a, new Set());
        prereq.get(a).add(b);
      }
    }
    const memo = new Map();
    const ancestors = (n) => {
      if (!memo.has(n)) {
        const acc = new Set();
        for (const p of prereq.get(n) || []) {
          acc.add(p);
          for (const q of ancestors(p)) acc.add(q);
        }
        memo.set(n, acc);
      }
      return memo.get(n);
    };
    const out = [];
    for (const a of [...prereq.keys()].sort(compare)) {
      const ps = prereq.get(a);
      for (const b of [...ps].sort(compare)) {
        let implied = false;
        for (const o of ps) if (o !== b && ancestors(o).has(b)) { implied = true; break; }
        if (!implied) out.push([a, b]);
      }
    }
    return out;
  }

  /** Longest-path layer, 0 = no prerequisite inside ids. Edges are [dependent, prerequisite]. */
  function layering(ids, edges) {
    const set = new Set(ids);
    const pre = new Map();
    for (const [a, b] of edges) {
      if (set.has(a) && set.has(b)) {
        if (!pre.has(a)) pre.set(a, []);
        pre.get(a).push(b);
      }
    }
    const memo = new Map();
    const stack = new Set();
    const layer = (n) => {
      if (memo.has(n)) return memo.get(n);
      if (stack.has(n)) throw new Error(`cycle at ${n}`);
      stack.add(n);
      const ps = pre.get(n) || [];
      const value = ps.length ? 1 + Math.max(...ps.map(layer)) : 0;
      memo.set(n, value);
      stack.delete(n);
      return value;
    };
    const out = new Map();
    for (const n of [...set].sort(compare)) out.set(n, layer(n));
    return out;
  }

  /** Barycentre ordering inside layers (fewer crossings). Returns Map<layer, ids[]>. */
  function orderLayers(layerOf, edges, iters = 14) {
    const layers = new Map();
    for (const [n, l] of [...layerOf.entries()].sort((a, b) => compare(a[0], b[0]))) {
      if (!layers.has(l)) layers.set(l, []);
      layers.get(l).push(n);
    }
    const pos = new Map();
    for (const members of layers.values()) members.forEach((n, i) => pos.set(n, i));
    const prv = new Map();
    const nxt = new Map();
    const push = (map, key, value) => {
      if (!map.has(key)) map.set(key, []);
      map.get(key).push(value);
    };
    for (const [a, b] of edges) {
      if (layerOf.has(a) && layerOf.has(b)) {
        push(prv, a, b);
        push(nxt, b, a);
      }
    }
    const frac = (n) => (pos.get(n) + 0.5) / Math.max(layers.get(layerOf.get(n)).length, 1);
    const mean = (list, fallback) => (list && list.length ? list.reduce((s, m) => s + frac(m), 0) / list.length : fallback);
    const sortedLayers = [...layers.keys()].sort((a, b) => a - b);
    const sortLayer = (l, neighbours) => {
      const members = layers.get(l);
      // A node without neighbours on the other side keeps its relative order but sorts after every connected node
      // (its key is the unnormalized index, always above the 0..1 barycentres).
      const keys = new Map(members.map((n) => [n, mean(neighbours.get(n), pos.get(n) + 0.5)]));
      members.sort((a, b) => keys.get(a) - keys.get(b) || compare(a, b));
      members.forEach((n, i) => pos.set(n, i));
    };
    for (let i = 0; i < iters; i++) {
      for (const l of sortedLayers.slice(1)) sortLayer(l, prv);
      for (const l of [...sortedLayers].reverse().slice(1)) sortLayer(l, nxt);
    }
    return layers;
  }

  /** Status palette entry with its localized label. */
  function createKit(t) {
    const ST = {};
    for (const [vis, style] of Object.entries(PALETTE)) ST[vis] = { ...style, label: t(`map.status.${vis}`) };

    function dot(cx, cy, vis, r = 6) {
      const s = ST[vis];
      if (vis === 'developing') {
        return `<circle cx="${cx}" cy="${cy}" r="${r + 4}" fill="none" stroke="rgba(10,92,255,.45)" stroke-width="1.3"/>`
          + `<circle cx="${cx}" cy="${cy}" r="${r}" fill="url(#gDev)"/>`;
      }
      if (s.ring) return `<circle cx="${cx}" cy="${cy}" r="${r}" fill="#fff" stroke="${s.ring}" stroke-width="2"/>`;
      return `<circle cx="${cx}" cy="${cy}" r="${r}" fill="${s.fill}"/>`;
    }

    /** Task card in the site's glass style. Returns [markup, height]. */
    function card(x, y, w, o) {
      const vis = o.vis;
      const s = ST[vis];
      const pad = 12;
      const titleSize = o.titleSize || 12.5;
      const compact = Boolean(o.compact);
      const tl = wrap(o.title, titleSize, w - 2 * pad - 2, o.tmax || 2);
      const label = o.tag || s.label;
      const [chipMarkup, chipWidth] = chip(x + w - pad, y + 9, label, s.fg, s.bg, s.border, 9, 'end', 16);
      const idw = w - 2 * pad - 20 - chipWidth - 6;
      const lh = compact ? 15 : 16;
      const bodyY = y + (compact ? 37 : 42);
      const body = [];
      tl.forEach((line, i) => {
        body.push(text(x + pad + 1, bodyY + i * lh, line, titleSize, vis !== 'cancelled' ? INK : MUTED, 650, 'start',
          { extra: vis === 'cancelled' ? ' text-decoration="line-through"' : '' }));
      });
      let cy = bodyY + (tl.length - 1) * lh + (compact ? 10 : 12);
      for (const entry of o.lines || []) {
        const [line, color] = Array.isArray(entry) ? entry : [entry, MUTED];
        for (const piece of wrap(line, 9.8, w - 2 * pad - 2, 2)) {
          body.push(text(x + pad + 1, cy + 4, piece, 9.8, color, 500));
          cy += 13;
        }
      }
      const items = [];
      if (o.owner) items.push([o.owner, '#3b6fd4', 'rgba(122,165,248,.12)', 'rgba(122,165,248,.4)', false]);
      for (const c of o.chips || []) items.push([c, '#556075', 'rgba(10,92,255,.06)', 'rgba(10,92,255,.18)', true]);
      if (o.idle !== undefined && o.idle !== null && vis === 'stalled') {
        items.push([o.idle < 99 ? t('map.card.stalled_idle', { days: f0(o.idle) }) : t('map.card.no_record'), '#b4740a', 'rgba(245,158,11,.12)', 'rgba(245,158,11,.4)', false]);
      }
      if (items.length) {
        let cx = x + pad;
        let ly = cy + 2;
        const placed = [];
        for (const [label2, fg, bg, st, mono] of items) {
          const shown = fit(label2, 9, w - 2 * pad - 14);
          let [markup, width] = chip(cx, ly, shown, fg, bg, st, 9, 'start', 16, 600, 6, mono);
          if (cx + width > x + w - pad) {
            cx = x + pad;
            ly += 20;
            [markup, width] = chip(cx, ly, shown, fg, bg, st, 9, 'start', 16, 600, 6, mono);
          }
          placed.push(markup);
          cx += width + 5;
        }
        body.push(...placed);
        cy = ly + 16 + 4;
      }
      const h = Math.max(cy + (compact ? 8 : 10) - y, compact ? 52 : 62);
      const dash = s.dash ? ` stroke-dasharray="${s.dash}"` : '';
      const flt = s.glow ? ' filter="url(#glow)"' : ' filter="url(#sh)"';
      const op = o.fade === undefined || o.fade === null ? s.op : o.fade;
      const halo = o.stuck
        ? `<rect x="${x - 3.5}" y="${y - 3.5}" width="${w + 7}" height="${f1(h + 7)}" rx="17" fill="none" stroke="#e14b54" stroke-opacity=".55" stroke-width="1.3" stroke-dasharray="2.5 3"/>`
        : '';
      const hook = o.key ? ` class="pm-card" data-key="${esc(o.key)}" data-vis="${vis}"` : '';
      const ring = o.key ? `<rect class="pm-ring" x="${x - 2}" y="${y - 2}" width="${w + 4}" height="${f1(h + 4)}" rx="16" fill="none" stroke="none"/>` : '';
      const out = [`<g opacity="${op}"${hook}><title>${esc(o.full || o.nid)} · ${esc(o.title)}</title>`, halo,
        `<rect x="${x}" y="${y}" width="${w}" height="${f1(h)}" rx="14" fill="${s.cardbg}" fill-opacity="${o.opaque ? 1 : 0.9}" stroke="${s.border}" stroke-width="1.2"${dash}${flt}/>`,
        dot(x + pad + 5, y + 17, vis),
        text(x + pad + 17, y + 20.5, dispId(o.nid, idw), 9.8, FAINT, 600, 'start', { mono: true, spacing: 0.3 }), chipMarkup, ...body, ring, '</g>'];
      return [out.join('\n'), h];
    }

    /** Chevron for a header that folds: pointing down when open, right when folded. */
    function chevron(x, y, open) {
      const d = open ? `M${f1(x)},${f1(y)} l4.5,4.5 l4.5,-4.5` : `M${f1(x + 2)},${f1(y - 3)} l4.5,4.5 l-4.5,4.5`;
      return `<path class="pm-chevron" d="${d}" fill="none" stroke="#86868b" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"/>`;
    }

    /** Lane header; with `hook` {attr, value, open} it folds its lane when clicked and shows a chevron. */
    function laneHead(cx, y, name, tag, color, minw = 150, hook = null) {
      const w = Math.max(minw, tw(name, 12) + 52 + (hook ? 14 : 0), tw(tag, 8.5) + 52);
      const x = cx - w / 2;
      const open = hook ? ` class="pm-fold" ${hook.attr}="${esc(hook.value)}" data-open="${hook.open}"` : '';
      return [`<g${open}><rect x="${f1(x)}" y="${y}" width="${f1(w)}" height="42" rx="12" fill="#fff" fill-opacity=".86" stroke="rgba(255,255,255,.9)" filter="url(#shS)"/>`
        + `<rect x="${f1(x + 16)}" y="${y + 16}" width="10" height="10" rx="3" fill="${color}"/>`
        + text(x + 34, y + 18, name, 12, INK, 700) + text(x + 34, y + 31, tag, 8.5, FAINT, 500, 'start', { spacing: 0.3 })
        + (hook ? chevron(x + w - 22, y + 19, hook.open) : '') + '</g>', w];
    }

    const edgeHook = (hook) => (hook ? ` data-from="${esc(hook.from)}" data-to="${esc(hook.to)}"` : '');

    function edge(x1, y1, x2, y2, kind = 'dep', hook = null) {
      const [stroke, width, extra, marker] = EDGE[kind];
      let d;
      if (Math.abs(x1 - x2) < 1) d = `M${f1(x1)},${f1(y1)} L${f1(x2)},${f1(y2)}`;
      else {
        const my = (y1 + y2) / 2;
        d = `M${f1(x1)},${f1(y1)} C${f1(x1)},${f1(my)} ${f1(x2)},${f1(my)} ${f1(x2)},${f1(y2)}`;
      }
      return `<path d="${d}" fill="none" stroke="${stroke}" stroke-width="${width}"${extra}${edgeHook(hook)} marker-end="url(#${marker})"/>`;
    }

    function pathEl(d, kind, arrow = true, hook = null) {
      const [stroke, width, dash, marker] = EDGE[kind];
      return `<path d="${d}" fill="none" stroke="${stroke}" stroke-width="${width}"${dash}${edgeHook(hook)}${arrow ? ` marker-end="url(#${marker})"` : ''}/>`;
    }

    function header(svg, cx, y, kicker, title, subtitle, note) {
      svg.add(text(cx, y, kicker, 9.5, FAINT, 600, 'middle', { spacing: 1.6 }));
      svg.add(text(cx, y + 34, title, 25, INK, 750, 'middle', { spacing: -0.3 }));
      svg.add(text(cx, y + 58, subtitle, 11, MUTED, 500, 'middle', { spacing: 1.2 }));
      svg.add(chip(cx, y + 72, note, '#8a6417', 'rgba(245,158,11,.12)', 'rgba(245,158,11,.34)', 10, 'middle', 22, 500, 12)[0]);
    }

    const LAYERS_GLYPH = 'M12 3l9 5-9 5-9-5 9-5z M3 12l9 5 9-5 M3 16l9 5 9-5';

    function projectCard(svg, cx, y, name, tag, source, goal, stats) {
      const w = 760;
      const x = cx - w / 2;
      const h = 76;
      svg.add(`<rect x="${x}" y="${y}" width="${w}" height="${h}" rx="18" fill="url(#gProj)" stroke="rgba(10,92,255,.32)" stroke-width="1.2" filter="url(#sh)"/>`);
      svg.add(`<rect x="${x + 16}" y="${y + 17}" width="42" height="42" rx="12" fill="url(#gDev)"/>`);
      svg.add(`<g transform="translate(${x + 25},${y + 26}) scale(.98)" fill="none" stroke="#fff" stroke-width="1.7" stroke-linejoin="round"><path d="${LAYERS_GLYPH}"/></g>`);
      svg.add(text(x + 72, y + 29, fit(name, 15, 330), 15, INK, 700));
      svg.add(chip(x + 72 + Math.min(tw(name, 15), 330) + 10, y + 15, tag, '#0a5cff', 'rgba(10,92,255,.09)', 'rgba(10,92,255,.28)', 9.5, 'start', 17)[0]);
      svg.add(text(x + 72, y + 45, fit(source, 9.5, 360), 9.5, FAINT, 500, 'start', { mono: true }));
      svg.add(text(x + 72, y + 61, fit(goal, 9.5, 360), 9.5, MUTED, 500));
      let sx = x + w - 20;
      for (const [val, label] of [...stats].reverse()) {
        sx -= 66;
        svg.add(text(sx + 33, y + 38, val, 18, INK, 700, 'middle'));
        svg.add(text(sx + 33, y + 54, label, 8.5, FAINT, 500, 'middle'));
        svg.add(`<line x1="${sx - 4}" y1="${y + 22}" x2="${sx - 4}" y2="${y + 56}" stroke="rgba(0,0,0,.08)"/>`);
      }
      return y + h;
    }

    /** Status legend, then the edge kinds given as [kind, label] (straight samples when `straight`). Returns the y below it. */
    function legend(svg, x, y, w, items, edgeKinds = [], straight = false) {
      svg.add(`<line x1="${x}" y1="${y}" x2="${x + w}" y2="${y}" stroke="rgba(0,0,0,.10)" stroke-dasharray="5 4"/>`);
      let cx = x + 6;
      const cy = y + 24;
      svg.add(text(cx, cy + 3, t('map.legend.title'), 9.5, FAINT, 600, 'start', { spacing: 1 }));
      cx += 40;
      for (const vis of items) {
        svg.add(dot(cx + 5, cy, vis, 5));
        svg.add(text(cx + 16, cy + 3.5, ST[vis].label, 10, MUTED, 500));
        cx += 16 + tw(ST[vis].label, 10) + 22;
      }
      for (const [kind, label] of edgeKinds) {
        svg.add(straight ? pathEl(`M${cx},${cy} L${cx + 30},${cy}`, kind) : edge(cx, cy, cx + 30, cy, kind));
        svg.add(text(cx + 38, cy + 3.5, label, 10, MUTED, 500));
        cx += 38 + tw(label, 10) + 22;
      }
      return y + 40;
    }

    function stackedBar(svg, x, y, w, counts, h = 7) {
      const total = Object.values(counts).reduce((s, c) => s + c, 0) || 1;
      const segs = BAR.filter(([k]) => counts[k]).map(([k, color]) => [k, counts[k], color]);
      const mins = segs.map(([, c]) => Math.max((w * c) / total, 3.5));
      const scale = mins.length ? w / mins.reduce((s, m) => s + m, 0) : 1;
      const id = `bar-${Math.round(x)}-${Math.round(y)}`; // derived from the position, so rendering stays a pure function
      const out = [`<clipPath id="${id}"><rect x="${x}" y="${y}" width="${w}" height="${h}" rx="${h / 2}"/></clipPath><g clip-path="url(#${id})">`,
        `<rect x="${x}" y="${y}" width="${w}" height="${h}" fill="#e6e9f0"/>`];
      let cx = x;
      segs.forEach(([, , color], i) => {
        const sw = mins[i] * scale;
        out.push(`<rect x="${f1(cx)}" y="${y}" width="${f1(sw)}" height="${h}" fill="${color}"/>`);
        out.push(`<rect x="${f1(cx + sw - 0.8)}" y="${y}" width=".8" height="${h}" fill="#fff" opacity=".9"/>`);
        cx += sw;
      });
      out.push('</g>');
      svg.add(out.join('\n'));
    }

    /** Block standing for finished work; with `hook` {attr, value, open} it unfolds the work when clicked. */
    function stub(x, y, w, title, sub, color = '#12855c', border = 'rgba(31,174,116,.42)', bg = 'rgba(31,174,116,.07)', hook = null) {
      const h = 48;
      const open = hook ? ` class="pm-fold" ${hook.attr}="${esc(hook.value)}" data-open="${hook.open}"` : '';
      return [[`<g${open}><title>${esc(title)} · ${esc(sub)}</title><rect x="${x}" y="${y}" width="${w}" height="${h}" rx="14" fill="${bg}" stroke="${border}" stroke-width="1.2" stroke-dasharray="5 4"/>`,
        `<circle cx="${x + 22}" cy="${y + 24}" r="9" fill="#1fae74"/>`,
        `<path d="M${x + 17.5},${y + 24.3} l3.2,3.3 l6.2,-6.8" fill="none" stroke="#fff" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/>`,
        text(x + 40, y + 21, fit(title, 12, w - 40 - (hook ? 26 : 12)), 12, color, 700),
        text(x + 40, y + 36, fit(sub, 9.5, w - 52 - (hook ? 14 : 0)), 9.5, MUTED, 500),
        hook ? chevron(x + w - 24, y + 21, hook.open) : '', '</g>'].join('\n'), h];
    }

    /** One finished item as a single line, for unfolded finished work. */
    function miniRow(x, y, w, o) {
      const h = 24;
      return [`<g class="pm-card" data-key="${esc(o.key)}" data-vis="${o.vis}" opacity=".95"><title>${esc(o.full || o.nid)} · ${esc(o.title)}</title>`
        + `<rect x="${x}" y="${y}" width="${w}" height="${h}" rx="9" fill="#fff" fill-opacity=".82" stroke="${ST[o.vis].border}" stroke-width="1"/>`
        + dot(x + 14, y + 12, o.vis, 4.2)
        + text(x + 26, y + 15.5, dispId(o.nid, 70, 9), 9, FAINT, 600, 'start', { mono: true })
        + text(x + 26 + Math.min(tw(dispId(o.nid, 70, 9), 9) + 8, 78), y + 15.5, fit(o.title, 10, w - 26 - 80 - 8), 10, MUTED, 500)
        + `<rect class="pm-ring" x="${x - 2}" y="${y - 2}" width="${w + 4}" height="${h + 4}" rx="11" fill="none" stroke="none"/></g>`, h];
    }

    /** Small button drawn inside a diagram: a label in a pill. Returns [markup, width]. */
    function pill(x, y, label, attrs, { anchor = 'start', h = 20, fg = '#556075', bg = 'rgba(10,92,255,.07)', stroke = 'rgba(10,92,255,.22)' } = {}) {
      const [body, w] = chip(x, y, label, fg, bg, stroke, 9.5, anchor, h, 600, 9);
      return [`<g class="pm-btn" ${attrs}>${body}</g>`, w];
    }

    /** A stack of cards standing for several items. */
    function groupCard(x, y, w, o) {
      const s = ST[o.vis];
      const fold = o.group ? ` class="pm-fold" data-group="${esc(o.group)}" data-open="false"` : '';
      const pad = 12;
      const depth = o.depth === undefined ? 2 : o.depth;
      const lines = o.items || [];
      const h = 42 + 14 * lines.length + (o.note ? 16 : 0) + 6;
      const out = [`<g${fold}><title>${esc(o.title)} · ${esc(o.tag || s.label)}</title>`];
      for (let i = depth; i > 0; i--) {
        out.push(`<rect x="${x + i * 4}" y="${y + i * 4}" width="${w - i * 8}" height="${h}" rx="14" fill="#fff" fill-opacity=".7" stroke="${s.border}" stroke-width="1"/>`);
      }
      out.push(`<rect x="${x}" y="${y}" width="${w}" height="${h}" rx="14" fill="${s.cardbg}" fill-opacity=".92" stroke="${s.border}" stroke-width="1.2" filter="url(#sh)"/>`);
      out.push(dot(x + pad + 5, y + 18, o.vis));
      const [chipMarkup, chipWidth] = chip(x + w - pad, y + 10, o.tag || s.label, s.fg, s.bg, s.border, 9, 'end', 16);
      out.push(chipMarkup);
      out.push(text(x + pad + 17, y + 21.5, fit(o.title, 12, w - pad * 2 - 20 - chipWidth - 6), 12, INK, 700));
      let yy = y + 40;
      for (const entry of lines) {
        const [line, color] = Array.isArray(entry) ? entry : [entry, MUTED];
        out.push(text(x + pad + 1, yy + 6, fit(line, 9.6, w - 2 * pad - 2), 9.6, color, 500));
        yy += 14;
      }
      if (o.note) out.push(text(x + pad + 1, yy + 8, fit(o.note, 9.2, w - 2 * pad - 2), 9.2, s.fg, 600));
      if (o.group) out.push(chevron(x + w - 24, y + 36, false));
      out.push('</g>');
      return [out.join('\n'), h + depth * 4];
    }

    return { ST, dot, card, laneHead, edge, pathEl, header, projectCard, legend, stackedBar, stub, groupCard, miniRow, pill, chevron, t };
  }

  /** Vertical flow of cards inside one lane; arrows join linked consecutive cards. */
  class Col {
    constructor(kit, svg, x, y, w) {
      Object.assign(this, { kit, svg, x, y, w, prev: null });
    }

    label(s, color = FAINT, keepChain = false) {
      this.y += 8;
      this.svg.add(text(this.x + 4, this.y + 8, s, 8.8, color, 650, 'start', { spacing: 1.1 }));
      this.y += 14;
      if (!keepChain) this.prev = null;
    }

    put(fn, link = null, hook = null, gap = null) {
      if (this.prev !== null) this.y += link ? 24 : gap === null ? 11 : gap;
      const [markup, h] = fn(this.x, this.y, this.w);
      if (link && this.prev !== null) this.svg.add(this.kit.edge(this.x + this.w / 2, this.prev, this.x + this.w / 2, this.y, link, hook));
      this.svg.add(markup);
      this.prev = this.y + h;
      this.y += h;
      return h;
    }
  }

  /** Drawing surface: a list of markup fragments rendered inside one <svg>. */
  class Svg {
    constructor(w, h) {
      Object.assign(this, { w, h, b: [] });
    }

    add(s) {
      this.b.push(s);
    }

    defs() {
      return `<defs>
<radialGradient id="gBg" cx="50%" cy="-12%" r="95%"><stop offset="0" stop-color="#ffffff"/><stop offset=".52" stop-color="#edf1f9"/><stop offset="1" stop-color="#e0e9f6"/></radialGradient>
<linearGradient id="gPanel" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#f8faff"/><stop offset="1" stop-color="#eef3fc"/></linearGradient>
<linearGradient id="gProj" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#ffffff"/><stop offset="1" stop-color="#eef4ff"/></linearGradient>
<linearGradient id="gDev" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="#57b8ff"/><stop offset="1" stop-color="#0a5cff"/></linearGradient>
<filter id="sh" x="-25%" y="-25%" width="150%" height="170%"><feDropShadow dx="0" dy="7" stdDeviation="7" flood-color="#0d3078" flood-opacity=".11"/></filter>
<filter id="shS" x="-25%" y="-25%" width="150%" height="170%"><feDropShadow dx="0" dy="3" stdDeviation="3.5" flood-color="#0d3078" flood-opacity=".10"/></filter>
<filter id="glow" x="-30%" y="-30%" width="160%" height="190%"><feDropShadow dx="0" dy="10" stdDeviation="10" flood-color="#0a5cff" flood-opacity=".30"/></filter>
<filter id="shell" x="-5%" y="-3%" width="110%" height="108%"><feDropShadow dx="0" dy="24" stdDeviation="26" flood-color="#0a3cb4" flood-opacity=".12"/></filter>
<marker id="aBlue" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="5.4" markerHeight="5.4" orient="auto-start-reverse"><path d="M1 1L9 5L1 9" fill="none" stroke="#6e98d9" stroke-width="1.3"/></marker>
<marker id="aCrit" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="5" markerHeight="5" orient="auto-start-reverse"><path d="M1 1L9 5L1 9" fill="none" stroke="#0a5cff" stroke-width="1.6"/></marker>
<marker id="aRed" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="5.4" markerHeight="5.4" orient="auto-start-reverse"><path d="M1 1L9 5L1 9" fill="none" stroke="#e14b54" stroke-width="1.3"/></marker>
<marker id="aGray" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="5" markerHeight="5" orient="auto-start-reverse"><path d="M1 1L9 5L1 9" fill="none" stroke="#9aa3b2" stroke-width="1.2"/></marker>
</defs>`;
    }

    render() {
      return `<svg xmlns="http://www.w3.org/2000/svg" width="${this.w}" height="${this.h}" viewBox="0 0 ${this.w} ${this.h}" font-family="${esc(FONT)}" role="img">\n${this.defs()}\n${this.b.join('\n')}\n</svg>\n`;
    }
  }

  const api = { FONT, MONO, INK, MUTED, FAINT, PALETTE, BAR, EDGE, esc, tw, fit, wrap, dispId, text, chip, transitiveReduction, layering, orderLayers, createKit, Col, Svg, compare, fixed, f0, f1 };
  if (commonJS) module.exports = api;
  else {
    root.AWR_PROJECT_MAP = root.AWR_PROJECT_MAP || {};
    root.AWR_PROJECT_MAP.svgkit = api;
  }
})(typeof globalThis !== 'undefined' ? globalThis : this);
