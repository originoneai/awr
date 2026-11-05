/**
 * Layered DAG layout (top to bottom): longest-path layers, dummy nodes for long edges, barycentre ordering inside layers and
 * isotonic (pool-adjacent-violators) x placement. Edges are [prerequisite, dependent] in drawing direction.
 */
(function (root) {
  'use strict';

  const commonJS = typeof module !== 'undefined' && module.exports;
  const kitLib = commonJS ? require('./svgkit.js') : root.AWR_PROJECT_MAP.svgkit;
  const { layering, orderLayers } = kitLib;

  const DUMMY_WIDTH = 18;
  const isDummy = (n) => n.startsWith('~');
  /** Key of an edge in a Set or Map. */
  const pairKey = (a, b) => `${a}\u0000${b}`;

  /** Non-decreasing least-squares fit (pool adjacent violators). */
  function pav(values) {
    const blocks = [];
    for (const v of values) {
      blocks.push([v, 1]);
      while (blocks.length > 1 && blocks[blocks.length - 2][0] / blocks[blocks.length - 2][1] > blocks[blocks.length - 1][0] / blocks[blocks.length - 1][1]) {
        const [sum, count] = blocks.pop();
        blocks[blocks.length - 1][0] += sum;
        blocks[blocks.length - 1][1] += count;
      }
    }
    const out = [];
    for (const [sum, count] of blocks) for (let i = 0; i < count; i++) out.push(sum / count);
    return out;
  }

  /** Returns {layerOf, rows (arrays of ids incl. dummies), x (centres), chains (pairKey -> dummy ids)}. */
  function layout(ids, edges, widthOf, gapX, iters = 40) {
    const deps = edges.map(([a, b]) => [b, a]); // [dependent, prerequisite] for the layering helpers
    const layerOf = layering(ids, deps);
    const hasPre = new Set(deps.map(([a]) => a));
    const succOf = new Map();
    for (const [a, b] of edges) {
      if (!succOf.has(a)) succOf.set(a, []);
      succOf.get(a).push(b);
    }
    for (const n of ids) { // roots float down next to their first consumer
      if (!hasPre.has(n) && succOf.get(n) && succOf.get(n).length) layerOf.set(n, Math.min(...succOf.get(n).map((s) => layerOf.get(s))) - 1);
    }
    const chains = new Map();
    const ext = [];
    for (const [a, b] of edges) {
      const la = layerOf.get(a);
      const lb = layerOf.get(b);
      if (!(lb > la)) throw new Error(`edge ${a} -> ${b} does not point to a later layer`);
      let prev = a;
      const dummies = [];
      for (let l = la + 1; l < lb; l++) {
        const d = `~${a}>${b}@${l}`;
        layerOf.set(d, l);
        dummies.push(d);
        ext.push([d, prev]);
        prev = d;
      }
      ext.push([b, prev]);
      chains.set(pairKey(a, b), dummies);
    }
    const layers = orderLayers(layerOf, ext, 24);
    const rows = [...layers.keys()].sort((p, q) => p - q).map((l) => layers.get(l));
    const w = (n) => (isDummy(n) ? DUMMY_WIDTH : widthOf(n));
    const nbPrev = new Map();
    const nbNext = new Map();
    const add = (map, key, value) => {
      if (!map.has(key)) map.set(key, []);
      map.get(key).push(value);
    };
    for (const [dep, pre] of ext) {
      add(nbPrev, dep, pre);
      add(nbNext, pre, dep);
    }
    const x = new Map();
    for (const row of rows) {
      const total = row.reduce((s, n) => s + w(n), 0) + gapX * (row.length - 1);
      let cx = -total / 2;
      for (const n of row) {
        x.set(n, cx + w(n) / 2);
        cx += w(n) + gapX;
      }
    }
    const settle = (row, nbrs) => {
      const anchors = row.filter((n) => (nbrs.get(n) || []).some((m) => x.has(m))).map((n) => x.get(n));
      const desired = row.map((n) => {
        const ns = (nbrs.get(n) || []).filter((m) => x.has(m)).map((m) => x.get(m));
        if (ns.length) return ns.reduce((s, v) => s + v, 0) / ns.length;
        return anchors.length ? anchors.reduce((s, v) => s + v, 0) / anchors.length : x.get(n);
      });
      const offsets = [];
      let o = 0;
      row.forEach((n, i) => {
        if (i) o += (w(row[i - 1]) + w(n)) / 2 + gapX;
        offsets.push(o);
      });
      const z = pav(desired.map((d, i) => d - offsets[i]));
      row.forEach((n, i) => x.set(n, z[i] + offsets[i]));
    };
    for (let it = 0; it < iters; it++) {
      for (const row of rows.slice(1)) settle(row, nbPrev);
      for (const row of [...rows.slice(0, -1)].reverse()) settle(row, nbNext);
    }
    const both = new Map([...x.keys()].map((n) => [n, [...(nbPrev.get(n) || []), ...(nbNext.get(n) || [])]]));
    for (const row of rows.slice(1)) settle(row, both);
    return { layerOf, rows, x, chains };
  }

  /**
   * Draw edges and cards into `svg`; returns {width, height, pos}. `cardFn(id, x, y)` returns markup.
   * `crit`, `thin` and `bad` are Sets of pairKey strings.
   */
  function drawDag(kit, svg, ox, oy, ids, edges, cardFn, widthOf, heightOf, opts = {}) {
    const gapX = opts.gapX === undefined ? 24 : opts.gapX;
    const gapY = opts.gapY === undefined ? 46 : opts.gapY;
    const crit = opts.crit || new Set();
    const thin = opts.thin || new Set();
    const bad = opts.bad || new Set();
    const { rows, x, chains } = layout(ids, edges, widthOf, gapX);
    const widthOfAny = (n) => (isDummy(n) ? DUMMY_WIDTH : widthOf(n));
    const real = [...x.keys()].filter((n) => !isDummy(n));
    const keys = [...x.keys()];
    const left = Math.min(...keys.map((n) => x.get(n) - widthOfAny(n) / 2));
    const right = Math.max(...keys.map((n) => x.get(n) + widthOfAny(n) / 2));
    const rowHeights = rows.map((row) => Math.max(0, ...row.filter((n) => !isDummy(n)).map(heightOf)));
    const centres = [];
    let yy = 0;
    for (const h of rowHeights) {
      centres.push(yy + h / 2);
      yy += h + gapY;
    }
    const height = yy - gapY;
    const pos = new Map();
    rows.forEach((row, r) => row.forEach((n) => pos.set(n, [ox - left + x.get(n), oy + centres[r]])));
    const rank = ([a, b]) => (crit.has(pairKey(a, b)) ? 2 : 0) + (bad.has(pairKey(a, b)) ? 1 : 0);
    const ordered = [...edges].sort((p, q) => rank(p) - rank(q));
    const f1 = kitLib.f1;
    for (const [a, b] of ordered) {
      const [xa, ya] = pos.get(a);
      const [xb, yb] = pos.get(b);
      const pts = [[xa, ya + heightOf(a) / 2], ...chains.get(pairKey(a, b)).map((d) => pos.get(d)), [xb, yb - heightOf(b) / 2 - 1]];
      let d = `M${f1(pts[0][0])},${f1(pts[0][1])}`;
      for (let i = 0; i + 1 < pts.length; i++) {
        const [x1, y1] = pts[i];
        const [x2, y2] = pts[i + 1];
        const dy = (y2 - y1) * 0.5;
        d += ` C${f1(x1)},${f1(y1 + dy)} ${f1(x2)},${f1(y2 - dy)} ${f1(x2)},${f1(y2)}`;
      }
      const key = pairKey(a, b);
      const kind = bad.has(key) ? 'cross' : crit.has(key) ? 'crit' : thin.has(key) ? 'thin' : 'dep';
      svg.add(kit.pathEl(d, kind, true, { from: a, to: b }));
    }
    for (const n of real) {
      const [cx, cy] = pos.get(n);
      svg.add(cardFn(n, cx - widthOf(n) / 2, cy - heightOf(n) / 2));
    }
    return { width: right - left, height, pos };
  }

  const api = { pairKey, pav, layout, drawDag, isDummy };
  if (commonJS) module.exports = api;
  else {
    root.AWR_PROJECT_MAP = root.AWR_PROJECT_MAP || {};
    root.AWR_PROJECT_MAP.layout = api;
  }
})(typeof globalThis !== 'undefined' ? globalThis : this);
