/**
 * The three project-map views as SVG documents: the project node graph (overview), the dependency panels of the main
 * lanes, and the milestone-lane dependency graph. Each renderer is a pure function of the model from `loadModel` and the
 * language function `t`; nothing is read from anywhere else, so the same snapshot and configuration always give the same
 * bytes. Edges are drawn after transitive reduction, and finished work is collapsed.
 */
(function (root) {
  'use strict';

  const commonJS = typeof module !== 'undefined' && module.exports;
  const kitLib = commonJS ? require('./svgkit.js') : root.AWR_PROJECT_MAP.svgkit;
  const layoutLib = commonJS ? require('./layout.js') : root.AWR_PROJECT_MAP.layout;
  const modelLib = commonJS ? require('./model.js') : root.AWR_PROJECT_MAP.model;
  const { INK, MUTED, FAINT, esc, tw, fit, wrap, text, chip, Svg, Col, layering, orderLayers, transitiveReduction, compare, createKit, f0, f1 } = kitLib;
  const { pairKey, drawDag } = layoutLib;

  const CW = 214;
  const GAP = 28;
  const LANE_Y = 318;
  const TOP = 414;
  const GROUP_ORDER = ['developing', 'stalled', 'blocked', 'ready', 'draft', 'waiting'];
  const RANK = { blocked: 3, stalled: 2, developing: 1 };
  const BLOCK = 'BLOCK';
  const STATE_MARK = { done: '✓', developing: '●', stalled: '◐', blocked: '✕', ready: '○', waiting: '○', draft: '○', cancelled: '✕' };
  const RED = '#c93b44';
  const RED_CHIP = [RED, 'rgba(225,75,84,.09)', 'rgba(225,75,84,.4)'];
  const AMBER_CHIP = ['#b4740a', 'rgba(245,158,11,.11)', 'rgba(245,158,11,.4)'];

  const counter = (items, keyOf) => {
    const c = {};
    for (const item of items) c[keyOf(item)] = (c[keyOf(item)] || 0) + 1;
    return c;
  };
  const pad2 = (n) => String(n).padStart(2, '0');
  /** `YYYY-MM-DD HH:MM` in UTC. */
  function utc(ms) {
    const d = new Date(ms);
    return `${d.getUTCFullYear()}-${pad2(d.getUTCMonth() + 1)}-${pad2(d.getUTCDate())} ${pad2(d.getUTCHours())}:${pad2(d.getUTCMinutes())}`;
  }

  function createViews(t) {
    const kit = createKit(t);
    const { ST } = kit;

    /** ['OPS-001','OPS-002'] -> 'OPS-001/002' (works on the shortened keys). */
    function compress(shorts, asList = false) {
      const groups = new Map();
      for (const s of shorts) {
        const m = /^(.*-)(\d+[A-Z]?)$/.exec(s);
        const prefix = m ? m[1] : s;
        if (!groups.has(prefix)) groups.set(prefix, []);
        groups.get(prefix).push(m ? m[2] : '');
      }
      const parts = [...groups.entries()].map(([prefix, sfx]) => (sfx[0] ? prefix + sfx.filter(Boolean).join('/') : prefix));
      return asList ? parts : parts.join(' · ');
    }

    function member(x, y, name, work, vis, h = 30) {
      const nm = name.startsWith('codex-') ? name.slice(6) : name;
      const t1 = fit(nm, 10.5, 190);
      const t2 = fit(work, 9.5, 130);
      const w1 = tw(t1, 10.5) * 1.12;
      const w = 10 + 20 + 7 + w1 + 6 + tw(t2, 9.5) * 1.1 + 12;
      const colors = { developing: ['url(#gDev)', 'rgba(10,92,255,.4)'], stalled: ['#f59e0b', 'rgba(245,158,11,.55)'], blocked: ['#e14b54', 'rgba(225,75,84,.5)'] }[vis] || ['#aeb7c7', 'rgba(0,0,0,.14)'];
      const out = [`<g class="pm-agent" data-agent="${esc(name)}"><title>${esc(name)} · ${esc(work)}</title>`,
        `<rect x="${f1(x)}" y="${y}" width="${f1(w)}" height="${h}" rx="${h / 2}" fill="#fff" fill-opacity=".9" stroke="${colors[1]}" filter="url(#shS)"/>`,
        `<circle cx="${f1(x + 20)}" cy="${y + h / 2}" r="10" fill="${colors[0]}"/>`,
        text(x + 20, y + h / 2 + 3.6, Array.from(nm)[0] ? Array.from(nm)[0].toUpperCase() : '', 10, '#fff', 700, 'middle'),
        text(x + 37, y + h / 2 + 3.8, t1, 10.5, INK, 650),
        text(x + 37 + w1 + 6, y + h / 2 + 3.6, t2, 9.5, MUTED, 500),
        '</g>'];
      return [out.join('\n'), w];
    }

    /** Frame shared by all three views: page gradient and the glass shell around the body. */
    function frame(body, W, H) {
      const final = new Svg(W, H);
      final.add(`<rect width="${W}" height="${H}" fill="url(#gBg)"/>`);
      final.add(`<rect x="28" y="24" width="${W - 56}" height="${H - 48}" rx="28" fill="#fff" fill-opacity=".55" stroke="rgba(255,255,255,.9)" stroke-width="1.5" filter="url(#shell)"/>`);
      final.b.push(...body.b);
      return final.render();
    }

    function placeholder(title, message, kicker = 'AWR · DEPENDENCY GRAPH') {
      const sv = new Svg(1100, 0);
      kit.header(sv, 550, 74, kicker, title, '', message);
      return frame(sv, 1100, 330);
    }

    // ------------------------------------------------------------------ overview

    function renderOverview(model, view = {}) {
      const { nodes, meta, cfg } = model;
      const fold = view.overview || {};
      const collapsedLanes = fold.collapsed || new Set();
      const doneOpen = fold.doneOpen || new Set();
      const groupsOpen = fold.groupsOpen || new Set();
      const stale = cfg.stale_days;
      const lanes = cfg.overview.lanes;
      const N = lanes.length;
      const laneTitle = (lane) => modelLib.laneName(lane, t);
      const K = 5;
      const inner = N * CW + (N - 1) * GAP;
      const W = Math.floor(Math.max(1100, inner + 2 * 70));
      const X0 = (W - inner) / 2;
      const now = meta.generated_at;
      const all = [...nodes.values()];
      const byLane = new Map(lanes.map((l) => [l.id, all.filter((n) => n.lane === l.id)]));
      const counts = new Map([...byLane.entries()].map(([id, ns]) => [id, counter(ns, (n) => n.vis)]));
      const tot = counter(all, (n) => n.vis);
      const edges = [];
      for (const n of all) for (const b of n.deps) if (nodes.has(b)) edges.push([n.id, b]);
      const cross = edges.filter(([a, b]) => nodes.get(a).lane !== nodes.get(b).lane);
      const cancelEdges = edges.filter(([, b]) => nodes.get(b).vis === 'cancelled');
      const stuck = meta.stuck;
      const dependents = new Map();
      for (const [a, b] of edges) {
        if (!dependents.has(b)) dependents.set(b, []);
        dependents.get(b).push(a);
      }
      const shortOf = (k) => nodes.get(k).short;

      const downstream = (c) => {
        const seen = new Set();
        const stack = [c];
        while (stack.length) {
          for (const d of dependents.get(stack.pop()) || []) {
            if (!seen.has(d) && nodes.get(d).vis !== 'done' && nodes.get(d).vis !== 'cancelled') {
              seen.add(d);
              stack.push(d);
            }
          }
        }
        return seen;
      };

      const cardOf = (n, { owner = false, tag = null, lines = [], prereq = false } = {}) => (x, y, w) => {
        const ls = [...lines];
        if (prereq) {
          const unmet = n.deps.filter((d) => nodes.get(d).status !== 'completed');
          if (unmet.length) ls.push([t('map.card.prereq', { keys: compress(unmet.map(shortOf)) }), FAINT]);
        }
        let label = tag;
        if (n.vis === 'stalled') label = n.idle !== null ? t('map.card.stalled_idle', { days: f0(n.idle) }) : ST.stalled.label;
        return kit.card(x, y, w, { vis: n.vis, nid: n.short, full: n.id, key: n.id, title: n.title, owner: owner && n.owner ? n.owner : null, lines: ls, tag: label, compact: true, titleSize: 11.5 });
      };

      const doneFamily = (ns, k = 3) => {
        const c = new Map();
        for (const n of ns) {
          const m = /^(?:[A-Z]+-)?([A-Za-z]+)/.exec(n.id) || /^(.*)$/.exec(n.id);
          c.set(m[1], (c.get(m[1]) || 0) + 1);
        }
        return [...c.entries()].sort((a, b) => b[1] - a[1]).slice(0, k).map(([p, v]) => `${p} ${v}`).join(' · ');
      };

      const bufs = new Map();
      const ends = new Map();
      lanes.forEach((lane, i) => {
        const sv = new Svg(0, 0);
        const col = new Col(kit, sv, X0 + i * (CW + GAP), TOP, CW);
        bufs.set(lane.id, sv);
        const ns = byLane.get(lane.id);
        const done = ns.filter((n) => n.vis === 'done');
        if (collapsedLanes.has(lane.id)) { // folded lane: the header and its counts stay, the cards are hidden
          col.label(t('map.lane.collapsed', { count: ns.length }));
          ends.set(lane.id, col.y);
          return;
        }
        if (done.length) {
          const open = doneOpen.has(lane.id);
          col.put((x, y, w) => kit.stub(x, y, w, t('map.done.title', { count: done.length }), open ? t('map.done.fold') : doneFamily(done),
            undefined, undefined, undefined, { attr: 'data-done-lane', value: lane.id, open }));
          if (open) {
            for (const n of [...done].sort((a, b) => compare(a.id, b.id))) {
              col.put((x, y, w) => kit.miniRow(x, y, w, { key: n.id, vis: 'done', nid: n.short, full: n.id, title: n.title }), null, null, 4);
            }
          }
        }
        const openIds = ns.filter((n) => n.vis !== 'done' && n.vis !== 'cancelled').map((n) => n.id);
        const openSet = new Set(openIds);
        const depth = openIds.length ? layering(openIds, openIds.flatMap((a) => nodes.get(a).deps.filter((b) => openSet.has(b)).map((b) => [a, b]))) : new Map();
        for (const c of ns.filter((n) => n.vis === 'cancelled').map((n) => n.id).sort(compare)) {
          const depsOf = (dependents.get(c) || []).filter((d) => nodes.get(d).vis !== 'done' && nodes.get(d).vis !== 'cancelled').sort(compare);
          if (!depsOf.length) continue;
          const down = downstream(c);
          col.label(t('map.defect.label'), RED);
          col.put((x, y, w) => kit.card(x, y, w, {
            vis: 'blocked', nid: `${shortOf(c)} → ${shortOf(depsOf[0])}`, full: `${c} <- ${depsOf[0]}`, key: c,
            title: t('map.defect.title', { count: depsOf.length, key: shortOf(c) }), tag: t('map.defect.tag'), compact: true, titleSize: 11.5,
            lines: [[t('map.defect.line', { count: down.size }), RED]],
          }));
        }
        for (const vis of GROUP_ORDER) {
          const gIds = ns.filter((n) => n.vis === vis).map((n) => n.id).sort((a, b) => (depth.get(a) || 0) - (depth.get(b) || 0) || compare(a, b));
          if (!gIds.length) continue;
          const label = vis === 'stalled' ? t('map.group.stalled', { days: stale }) : t(`map.group.${vis}`);
          col.label(label, { stalled: '#b4740a', blocked: RED }[vis] || FAINT);
          const groupKey = `${lane.id}|${vis}`;
          const expanded = groupsOpen.has(groupKey);
          const shown = expanded || gIds.length <= K ? gIds : gIds.slice(0, K - 1);
          let prev = null;
          for (const k of shown) {
            const n = nodes.get(k);
            const lines = vis === 'blocked' && n.blocker ? [[fit(n.blocker, 9.8, 400), RED]] : [];
            col.put(cardOf(n, { owner: vis === 'developing' || vis === 'stalled', lines, prereq: vis === 'waiting' }), prev !== null && n.deps.includes(prev) ? 'dep' : null, prev !== null ? { from: prev, to: k } : null);
            prev = k;
          }
          const rest = gIds.slice(shown.length);
          if (rest.length) {
            col.put((x, y, w) => kit.groupCard(x, y, w, {
              vis, title: t('map.more.title', { count: rest.length }), tag: ST[vis].label, group: groupKey,
              items: compress(rest.map(shortOf), true).slice(0, 4).map((s) => [s, MUTED]), note: label.split(' ·')[0],
            }));
          } else if (expanded && gIds.length > K) {
            col.put((x, y) => [kit.pill(x + 2, y, t('map.more.fewer'), `data-group="${esc(groupKey)}" data-open="true"`, { h: 22 })[0], 22]);
          }
        }
        if (!ns.length) col.label(t('map.lane.empty'));
        ends.set(lane.id, col.y);
      });

      const lanesBottom = Math.max(...ends.values()) + 14;
      const body = new Svg(W, 0);
      const cx = W / 2;
      kit.header(body, cx, 74, 'AWR · PROJECT NODE GRAPH', cfg.titles.overview || t('map.title.overview'),
        t('map.overview.subtitle', { nodes: nodes.size, cancelled: tot.cancelled || 0, edges: edges.length, lanes: N, time: utc(now) }),
        t('map.overview.note'));
      const pcBottom = kit.projectCard(body, cx, 196, meta.project.name, t('map.project.revision', { revision: meta.revision }), t('map.project.source'), cfg.project_goal || '',
        [[String(nodes.size), t('map.stat.items')], [String(edges.length), t('map.stat.edges')], [String(tot.stalled || 0), ST.stalled.label], [String(tot.blocked || 0), ST.blocked.label]]);
      const busY = pcBottom + 20;
      const laneCx = lanes.map((_, i) => X0 + i * (CW + GAP) + CW / 2);
      body.add(`<path d="M${cx},${pcBottom} L${cx},${busY}" stroke="#b7c9ee" stroke-width="1.6" fill="none"/>`);
      body.add(`<path d="M${laneCx[0]},${busY} L${laneCx[laneCx.length - 1]},${busY}" stroke="#b7c9ee" stroke-width="1.6" fill="none"/>`);
      for (const lx of laneCx) body.add(`<path d="M${lx},${busY} L${lx},${LANE_Y - 2}" stroke="#b7c9ee" stroke-width="1.6" fill="none" marker-end="url(#aBlue)"/>`);
      lanes.forEach((lane, i) => {
        const x = X0 + i * (CW + GAP);
        body.add(`<rect x="${x - 8}" y="${LANE_Y - 8}" width="${CW + 16}" height="${lanesBottom - LANE_Y + 14}" rx="22" fill="#fff" fill-opacity=".38" stroke="rgba(255,255,255,.85)"/>`);
      });
      lanes.forEach((lane, i) => {
        const x = X0 + i * (CW + GAP);
        const c = counts.get(lane.id);
        const [head] = kit.laneHead(laneCx[i], LANE_Y, laneTitle(lane), lane.tag, lane.color, 150, { attr: 'data-lane', value: lane.id, open: !collapsedLanes.has(lane.id) });
        body.add(head);
        kit.stackedBar(body, x + 4, LANE_Y + 52, CW - 8, c);
        body.add(text(x + 4, LANE_Y + 80, t('map.lane.done_of', { done: c.done || 0, total: Object.values(c).reduce((s, v) => s + v, 0) }), 10.5, INK, 650));
        let rx = x + CW - 4;
        for (const vis of ['blocked', 'stalled', 'developing']) {
          if (c[vis]) {
            const lab = String(c[vis]);
            rx -= tw(lab, 10) + 4;
            body.add(text(rx, LANE_Y + 80, lab, 10, ST[vis].fg, 700));
            rx -= 10;
            body.add(kit.dot(rx + 1, LANE_Y + 76.5, vis, 3.6));
            rx -= 7;
          }
        }
        body.b.push(...bufs.get(lane.id).b);
      });

      const byAgent = new Map();
      for (const [k, agent] of Object.entries(meta.claims)) {
        const n = nodes.get(k);
        if (n && n.vis !== 'done') {
          if (!byAgent.has(agent)) byAgent.set(agent, []);
          byAgent.get(agent).push(n);
        }
      }
      const staleClaims = Object.keys(meta.claims).filter((k) => nodes.has(k) && nodes.get(k).vis === 'done');
      const by = lanesBottom + 44;
      let rowx = X0;
      let rowy = by + 44;
      const chips = [];
      const agents = [...byAgent.entries()].sort((a, b) => {
        const live = (ns) => (ns.some((n) => (RANK[n.vis] || 0) === 1) ? 1 : 0);
        return live(b[1]) - live(a[1]) || compare(a[0], b[0]);
      });
      for (const [agent, ns] of agents) {
        const worst = ns.reduce((acc, n) => ((RANK[n.vis] || 0) > (RANK[acc.vis] || 0) ? n : acc), ns[0]).vis;
        const work = [...ns].sort((a, b) => compare(a.id, b.id)).map((n) => n.short).join(' · ');
        const [, w0] = member(0, 0, agent, work, worst);
        if (rowx + w0 > X0 + inner) {
          rowx = X0;
          rowy += 38;
        }
        const [markup, w] = member(rowx, rowy, agent, work, worst);
        chips.push(markup);
        rowx += w + 8;
      }
      const membersBottom = rowy + 30;
      const hy = membersBottom + 16;
      const hygiene = [[t('map.doctor.active_sessions'), meta.sessions_active, 'warn']];
      for (const code of Object.keys(meta.doctor.codes).sort(compare)) {
        const sev = meta.doctor.severity_by_code[code];
        if (sev === 'error' || sev === 'warning') hygiene.push([code, meta.doctor.codes[code], sev === 'error' ? 'err' : 'warn']);
      }
      const barBottom = hy + 34;
      body.add(`<rect x="${X0 - 8}" y="${by}" width="${inner + 16}" height="${barBottom - by + 14}" rx="22" fill="#fff" fill-opacity=".72" stroke="rgba(255,255,255,.9)" filter="url(#shS)"/>`);
      body.add(text(X0 + 8, by + 28, 'EXECUTORS', 9.5, FAINT, 600, 'start', { spacing: 1.4 }));
      body.add(text(X0 + 84, by + 28, t('map.executors.note', { items: [...byAgent.values()].reduce((s, v) => s + v.length, 0), agents: byAgent.size }), 10.5, MUTED, 500));
      body.b.push(...chips);
      body.add(text(X0 + 8, hy + 15, 'DOCTOR', 9.5, FAINT, 600, 'start', { spacing: 1.4 }));
      let hx = X0 + 70;
      for (const [name, val, sev] of hygiene) {
        const [fg, bg, bd] = sev === 'err' ? RED_CHIP : AMBER_CHIP;
        const [markup, w] = chip(hx, hy, `${name}  ${val}`, fg, bg, bd, 10, 'start', 24, 600, 11);
        body.add(markup);
        hx += w + 8;
      }
      if (cancelEdges.length) {
        const [markup, w] = chip(hx, hy, t('map.doctor.cancelled_deps', { count: cancelEdges.length, stuck: stuck.size }), ...RED_CHIP, 10, 'start', 24, 600, 11);
        body.add(markup);
        hx += w + 8;
      }
      if (staleClaims.length) {
        const [markup, w] = chip(hx, hy, t('map.doctor.stale_claims', { count: staleClaims.length }), ...RED_CHIP, 10, 'start', 24, 600, 11);
        body.add(markup);
        hx += w + 8;
      }
      const visItems = ['done', 'developing', 'stalled', 'blocked', 'ready', 'waiting'].concat(tot.draft ? ['draft'] : []);
      const lgBottom = kit.legend(body, X0, barBottom + 40, inner, visItems, [['dep', t('map.edge.dep')]]);
      const gaps = meta.unavailable.map((u) => u.field).join(', ');
      body.add(text(X0 + 6, lgBottom + 4, t('map.overview.note_stalled', { days: stale }), 9.5, FAINT, 500));
      body.add(text(X0 + 6, lgBottom + 20, t('map.overview.note_counts', { edges: edges.length, cross: cross.length, fingerprint: meta.fingerprint.slice(7, 19), revision: meta.revision, time: utc(now) }), 9.5, FAINT, 500));
      body.add(text(X0 + 6, lgBottom + 36, t('map.overview.note_gaps', { fields: fit(gaps, 9.5, inner - 20) }), 9.5, FAINT, 500));
      return frame(body, W, Math.floor(lgBottom + 72));
    }

    // ------------------------------------------------------------------ dependency panels

    function prepare(model, idsIn, keepDone = false) {
      const { nodes } = model;
      const ids = [...idsIn].sort(compare);
      const iset = new Set(ids);
      const E = ids.flatMap((a) => nodes.get(a).deps.filter((b) => iset.has(b)).map((b) => [a, b])); // [dependent, prerequisite]
      const R = transitiveReduction(ids, E);
      const pred = new Map(ids.map((k) => [k, []]));
      const succ = new Map(ids.map((k) => [k, []]));
      for (const [a, b] of R) {
        pred.get(a).push(b);
        succ.get(b).push(a);
      }
      const open = new Set(ids.filter((k) => nodes.get(k).vis !== 'done'));
      let shown;
      let coll;
      if (keepDone) {
        shown = new Set(ids);
        coll = new Set();
      } else {
        const finishedUnlocking = ids.filter((k) => nodes.get(k).vis === 'done' && succ.get(k).some((s) => open.has(s)));
        shown = new Set([...open, ...finishedUnlocking]);
        coll = new Set(ids.filter((k) => !shown.has(k)));
      }
      const edges = R.filter(([a, b]) => shown.has(a) && shown.has(b)).map(([a, b]) => [b, a]); // [prerequisite, dependent]
      const blockTargets = [...shown].sort(compare).filter((a) => pred.get(a).some((p) => coll.has(p)) && !pred.get(a).some((p) => shown.has(p)));
      if (coll.size && blockTargets.length) for (const a of blockTargets) edges.push([BLOCK, a]);
      const live = new Set([...open].filter((k) => nodes.get(k).vis !== 'cancelled'));
      const bad = new Set(edges.filter(([p]) => p !== BLOCK && nodes.get(p).vis === 'cancelled').map(([p, d]) => pairKey(p, d)));
      return { ids, E, R, pred, succ, open, live, shown, coll, edges, bad };
    }

    /** Longest chain of unfinished nodes that ends at `end`: {edges: Set<pairKey>, path}. */
    function critical(G, end) {
      const op = new Map();
      for (const a of G.open) op.set(a, G.edges.filter(([p, d]) => d === a && G.open.has(p)).map(([p]) => p));
      const memo = new Map();
      const longest = (n) => {
        if (!memo.has(n)) {
          let best = [1, null];
          for (const p of op.get(n) || []) {
            const candidate = [longest(p)[0] + 1, p];
            // Python compares (length, key) tuples: the longer chain wins, then the larger key.
            if (candidate[0] > best[0] || (candidate[0] === best[0] && (best[1] === null || compare(candidate[1], best[1]) > 0))) best = candidate;
          }
          memo.set(n, best);
        }
        return memo.get(n);
      };
      const path = [];
      for (let n = end; n; n = longest(n)[1]) path.push(n);
      path.reverse();
      return { edges: new Set(path.slice(0, -1).map((a, i) => pairKey(a, path[i + 1]))), path };
    }

    /** The open node that ends the longest open chain (ties: smallest key); null when no chain has two nodes. */
    function autoEnd(G) {
      let best = [1, null];
      for (const k of [...G.open].sort(compare)) {
        if (!G.shown.has(k)) continue;
        const n = critical(G, k).path.length;
        if (n > best[0]) best = [n, k];
      }
      return best[1];
    }

    function makeCard(model, G, n, w, size = 11.5, tmax = 2) {
      const { nodes, meta } = model;
      const nd = nodes.get(n);
      const lines = [];
      const declared = nd.deps.length;
      const reduced = G.pred.get(n).length;
      if (declared - reduced >= 2) lines.push([t('map.card.reduced', { declared, reduced }), FAINT]);
      const kids = G.ids.filter((k) => k !== n && k.startsWith(n) && /^[A-Z]$/.test(k.slice(n.length))).sort(compare);
      if (kids.length) {
        model.kidsShown = true;
        lines.push([t('map.card.rollup', { kids: kids.map((k) => `${k.slice(n.length)}${STATE_MARK[nodes.get(k).vis]}`).join(' ') }), MUTED]);
      }
      const tag = nd.vis === 'stalled' ? (nd.idle !== null ? t('map.card.stalled_idle', { days: f0(nd.idle) }) : ST.stalled.label) : null;
      return (x, y) => kit.card(x, y, w, {
        vis: nd.vis, nid: nd.short, full: nd.id, key: nd.id, title: nd.title, owner: nd.vis === 'developing' || nd.vis === 'stalled' ? nd.owner : null,
        lines, tag, compact: true, titleSize: size, tmax, stuck: meta.stuck.has(n), opaque: true,
      });
    }

    /** Collapsed finished work as a tiny layered DAG. */
    function minimap(model, G, w, laneId) {
      const coll = [...G.coll].sort(compare);
      const Rin = G.R.filter(([a, b]) => G.coll.has(a) && G.coll.has(b));
      const L = layering(coll, Rin);
      const rows = orderLayers(L, Rin, 12);
      const maxl = rows.size ? Math.max(...rows.keys()) : 0;
      const maxr = rows.size ? Math.max(...[...rows.values()].map((v) => v.length)) : 1;
      const top = 44;
      const pad = 16;
      const dx = (w - 2 * pad) / Math.max(maxl, 1);
      const dy = Math.min(11.0, 70.0 / Math.max(maxr - 1, 1));
      const h = Math.floor(top + (maxr - 1) * dy + pad + 6);
      const pos = new Map();
      for (const [l, ns] of rows) {
        const off = ((maxr - ns.length) * dy) / 2;
        ns.forEach((k, i) => pos.set(k, [pad + l * dx, top + off + i * dy]));
      }
      const shortOf = (k) => model.nodes.get(k).short;
      const draw = (x, y) => {
        const o = [`<g class="pm-fold" data-panel-done="${esc(laneId)}" data-open="false"><title>${esc(t('map.minimap.title', { count: coll.length }))} · ${esc(t('map.minimap.sub', { first: shortOf(coll[0]), last: shortOf(coll[coll.length - 1]) }))}</title>`,
          `<rect x="${x}" y="${y}" width="${w}" height="${h}" rx="14" fill="rgba(31,174,116,.07)" stroke="rgba(31,174,116,.42)" stroke-width="1.2" stroke-dasharray="5 4"/>`,
          kit.chevron(x + w - 24, y + 21, false),
          `<circle cx="${x + 22}" cy="${y + 22}" r="9" fill="#1fae74"/>`,
          `<path d="M${x + 17.5},${y + 22.3} l3.2,3.3 l6.2,-6.8" fill="none" stroke="#fff" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/>`,
          text(x + 40, y + 20, t('map.minimap.title', { count: coll.length }), 12, '#12855c', 700),
          text(x + 40, y + 34, t('map.minimap.sub', { first: shortOf(coll[0]), last: shortOf(coll[coll.length - 1]) }), 9.2, MUTED, 500)];
        for (const [a, b] of Rin) {
          const [x1, y1] = pos.get(b);
          const [x2, y2] = pos.get(a);
          const mid = f1(x + (x1 + x2) / 2);
          o.push(`<path d="M${f1(x + x1)},${f1(y + y1)} C${mid},${f1(y + y1)} ${mid},${f1(y + y2)} ${f1(x + x2)},${f1(y + y2)}" fill="none" stroke="#9bd6bb" stroke-width=".9"/>`);
        }
        for (const [px, py] of pos.values()) o.push(`<circle cx="${f1(x + px)}" cy="${f1(y + py)}" r="3" fill="#1fae74" stroke="#fff" stroke-width=".8"/>`);
        o.push('</g>');
        return o.join('\n');
      };
      return [draw, h];
    }

    function blockStub(G, w, laneId) {
      const count = G.coll.size;
      return [(x, y) => kit.stub(x, y, w, t('map.done.title', { count }), t('map.done.accepted'), undefined, undefined, undefined,
        { attr: 'data-panel-done', value: laneId, open: false })[0], 48];
    }

    function lanePanel(model, G, { cw, gapX = 24, gapY = 46, critEnd = null, minimapOn = true, laneId = '' }) {
      const hcache = new Map();
      const drawers = new Map();
      const widths = new Map();
      const ids = [...G.shown].sort(compare);
      for (const n of ids) {
        const f = makeCard(model, G, n, cw);
        hcache.set(n, f(0, 0)[1]);
        drawers.set(n, f);
      }
      const allIds = [...ids];
      let blockDraw = null;
      if (G.coll.size && G.edges.some(([a]) => a === BLOCK)) {
        let bh;
        if (minimapOn && G.coll.size >= 12) {
          widths.set(BLOCK, 380);
          [blockDraw, bh] = minimap(model, G, 380, laneId);
        } else {
          widths.set(BLOCK, cw);
          [blockDraw, bh] = blockStub(G, cw, laneId);
        }
        hcache.set(BLOCK, bh);
        allIds.push(BLOCK);
      }
      const widthOf = (n) => (n === BLOCK ? widths.get(BLOCK) : cw);
      const heightOf = (n) => hcache.get(n);
      const cardFn = (n, x, y) => (n === BLOCK ? blockDraw(x, y) : drawers.get(n)(x, y)[0]);
      let crit = new Set();
      let path = [];
      if (critEnd && G.shown.has(critEnd)) ({ edges: crit, path } = critical(G, critEnd));
      const tmp = new Svg(0, 0);
      const { width: gw, height: gh } = drawDag(kit, tmp, 0, 0, allIds, G.edges, cardFn, widthOf, heightOf, { gapX, gapY, crit, bad: G.bad });
      const padX = 34;
      const headH = 74;
      return { tmp, gw, gh, pw: gw + 2 * padX, ph: gh + headH + 34, padX, headH, critPath: path, crit };
    }

    /** One lane panel: glass backdrop, a header that folds it, an optional finished-work toggle, then the drawing. */
    function placePanel(sv, ox, oy, lane, P, subtitle, hooks = {}) {
      const title = modelLib.laneName(lane, t);
      const fold = { attr: 'data-panel', value: lane.id, open: !hooks.folded };
      sv.add(`<rect x="${ox}" y="${oy}" width="${P.pw}" height="${P.ph}" rx="24" fill="#fff" fill-opacity=".42" stroke="rgba(255,255,255,.88)" filter="url(#shS)"/>`);
      const [, w0] = kit.laneHead(0, 0, title, lane.tag, lane.color, 150, fold);
      const [markup, w] = kit.laneHead(ox + 22 + w0 / 2, oy + 18, title, lane.tag, lane.color, 150, fold);
      sv.add(markup);
      const toggleW = hooks.doneToggle ? tw(hooks.doneToggle.label, 9.5) + 18 + 14 : 0;
      sv.add(text(ox + 22 + w + 16, oy + 44, fit(subtitle, 11, P.pw - w - 22 - 16 - 22 - toggleW), 11, MUTED, 500));
      if (hooks.doneToggle) {
        sv.add(kit.pill(ox + P.pw - 22, oy + 26, hooks.doneToggle.label, `data-panel-done="${esc(lane.id)}" data-open="${hooks.doneToggle.open}"`, { anchor: 'end' })[0]);
      }
      if (!P.tmp.b.length) return;
      const gx = ox + (P.pw - P.gw) / 2;
      sv.add(`<g transform="translate(${f1(gx)},${oy + P.headH})">`);
      sv.b.push(...P.tmp.b);
      sv.add('</g>');
    }

    function renderMainline(model, view = {}) {
      const { nodes, meta, cfg } = model;
      model.kidsShown = false;
      const fold = view.mainline || {};
      const foldedPanels = fold.collapsed || new Set();
      const showDone = fold.showDone || new Set();
      const laneMeta = new Map(cfg.overview.lanes.map((l) => [l.id, l]));
      const now = meta.generated_at;
      const built = [];
      for (const p of cfg.mainline_panels || []) {
        const ids = [...nodes.values()].filter((n) => n.lane === p.lane).map((n) => n.id);
        if (!ids.length) continue;
        const forced = Boolean(p.keep_done);
        const G = prepare(model, ids, forced || showDone.has(p.lane));
        const end = p.crit_end || autoEnd(G);
        const folded = foldedPanels.has(p.lane);
        // a folded panel keeps its header and counts; the drawing is skipped
        const P = folded
          ? { tmp: new Svg(0, 0), gw: 0, gh: 0, pw: 0, ph: 74, padX: 34, headH: 74, critPath: [], crit: new Set() }
          : lanePanel(model, G, { cw: 198, gapX: 26, gapY: 48, critEnd: end, laneId: p.lane });
        const hasDone = ids.some((k) => nodes.get(k).vis === 'done');
        built.push({ p, G, P, end, folded, doneToggle: !forced && hasDone ? { open: showDone.has(p.lane), label: t(showDone.has(p.lane) ? 'map.panel.hide_done' : 'map.panel.show_done') } : null });
      }
      const names = built.map(({ p }) => modelLib.laneName(laneMeta.get(p.lane), t));
      const title = cfg.titles.mainline || (built.length ? t('map.title.mainline_lanes', { lanes: names.join(t('map.join.and')) }) : t('map.title.mainline'));
      if (!built.length) return placeholder(title, t('map.mainline.none'));
      const W = Math.floor(Math.max(1200, Math.max(...built.map(({ P }) => P.pw)) + 2 * 76));
      for (const b of built) if (b.folded) b.P.pw = W - 152;
      const declared = built.reduce((s, { G }) => s + G.E.length, 0);
      const reduced = built.reduce((s, { G }) => s + G.E.length - G.R.length, 0);
      const sv = new Svg(W, 0);
      kit.header(sv, W / 2, 74, 'AWR · DEPENDENCY GRAPH', title,
        `${built.map(({ p, G }) => t('map.mainline.lane_count', { name: modelLib.laneName(laneMeta.get(p.lane), t), count: G.ids.length })).join(' · ')} · ${t('map.mainline.reduction', { declared, kept: declared - reduced })} · ${utc(now)} UTC`,
        t('map.mainline.note'));
      let y = 196;
      for (const { p, G, P, end, folded, doneToggle } of built) {
        const lid = p.lane;
        const cnt = counter(G.ids, (k) => nodes.get(k).vis);
        let crossCount = 0;
        for (const n of nodes.values()) for (const d of n.deps) if (nodes.has(d) && ((n.lane === lid) !== (nodes.get(d).lane === lid))) crossCount += 1;
        let sub = folded
          ? t('map.panel.folded', { done: cnt.done || 0, total: G.ids.length, open: G.open.size })
          : P.critPath.length
            ? t('map.panel.crit', { done: cnt.done || 0, total: G.ids.length, open: G.open.size, layers: P.critPath.length, end: nodes.get(end).short })
            : t('map.panel.parallel', { done: cnt.done || 0, total: G.ids.length, ready: cnt.ready || 0, waiting: cnt.waiting || 0 });
        sub += t('map.panel.cross', { count: crossCount }) + (crossCount === 0 ? t('map.panel.island') : '');
        placePanel(sv, P.pw < W - 152 ? (W - P.pw) / 2 : 76, y, laneMeta.get(lid), P, sub, { folded, doneToggle });
        y += P.ph + 28;
      }
      const present = new Set(built.flatMap(({ G }) => [...G.shown].map((k) => nodes.get(k).vis)));
      const visItems = ['done', 'developing', 'stalled', 'blocked', 'ready', 'draft', 'waiting'].filter((v) => present.has(v));
      const ly = y - 6;
      const critEnds = built.filter(({ P }) => P.critPath.length).map(({ end }) => nodes.get(end).short);
      const edgeKinds = [['dep', t('map.edge.dep_up')]];
      if (critEnds.length) edgeKinds.push(['crit', t('map.edge.crit_of', { ends: critEnds.join(t('map.join.list')) })]);
      const lg = kit.legend(sv, 76, ly, W - 152, visItems, edgeKinds);
      sv.add(text(82, lg + 4, t('map.mainline.note_reduced'), 9.5, FAINT, 500));
      let h = lg + 40;
      if (model.kidsShown) {
        sv.add(text(82, lg + 20, t('map.mainline.note_kids'), 9.5, FAINT, 500));
        h = lg + 56;
      }
      return frame(sv, W, Math.floor(h));
    }

    // ------------------------------------------------------------------ milestone lanes

    function renderExplore(model, view = {}) {
      const { nodes, meta, cfg } = model;
      model.kidsShown = false;
      const fold = view.explore || {};
      const foldedLanes = fold.collapsed || new Set();
      const showDone = Boolean(fold.showDone);
      const lanes = cfg.explore_lanes || [];
      const title = cfg.titles.explore || t('map.title.explore');
      if (!lanes.length) return placeholder(title, t('map.explore.no_lanes'));
      const byMs = new Map();
      lanes.forEach((l, i) => l.milestones.forEach((m) => byMs.set(m, i)));
      const now = meta.generated_at;
      const exIds = [...nodes.values()].filter((n) => byMs.has(n.milestone)).map((n) => n.id);
      const GE = prepare(model, exIds);
      const laneOf = (k) => byMs.get(nodes.get(k).milestone);
      const candidates = (showDone ? GE.ids : GE.ids.filter((k) => nodes.get(k).vis !== 'done')).slice().sort(compare);
      if (!candidates.length) return placeholder(title, t('map.explore.none'));
      const shown = candidates.filter((k) => !foldedLanes.has(laneOf(k)));
      const sset = new Set(shown);
      const edges = GE.R.filter(([a, b]) => sset.has(a) && sset.has(b)).map(([a, b]) => [b, a]); // [prerequisite, dependent]
      const layer = layering(shown, edges.map(([b, a]) => [a, b]));
      const pre = new Map(shown.map((k) => [k, []]));
      for (const [b, a] of edges) pre.get(a).push(b);
      const row = new Map();
      const nxt = new Map();
      for (const k of [...shown].sort((a, b) => layer.get(a) - layer.get(b) || laneOf(a) - laneOf(b) || compare(a, b))) {
        const r = Math.max(...pre.get(k).map((p) => row.get(p) + 1), nxt.get(laneOf(k)) || 0);
        row.set(k, r);
        nxt.set(laneOf(k), r + 1);
      }
      const LW = 146;
      const LG = 18;
      const card3 = new Map(shown.map((k) => [k, makeCard(model, GE, k, LW, 10, 3)]));
      const hh = new Map(shown.map((k) => [k, card3.get(k)(0, 0)[1]]));
      const nrows = shown.length ? Math.max(...row.values()) + 1 : 0;
      const rowh = Array.from({ length: nrows }, (_, r) => Math.max(0, ...shown.filter((k) => row.get(k) === r).map((k) => hh.get(k))));
      const GAPY = 30;
      const rowy = [];
      let yy = 0;
      for (const h of rowh) {
        rowy.push(yy);
        yy += h + GAPY;
      }
      const bodyH = Math.max(yy - GAPY, 24);
      const pitch = LW + LG;
      const innerW = lanes.length * pitch - LG;
      const PAD = 34;
      const HEAD = 118;
      const pw = innerW + 2 * PAD;
      const ph = HEAD + bodyH + 34;
      const W = Math.floor(Math.max(1240, pw + 2 * 76));
      const ce = counter(GE.ids, (k) => nodes.get(k).vis);
      const sv = new Svg(W, 0);
      kit.header(sv, W / 2, 74, 'AWR · DEPENDENCY GRAPH', title,
        t('map.explore.subtitle', { live: GE.live.size, declared: GE.E.length, kept: GE.R.length, time: utc(now) }), t('map.explore.note'));
      const ox = (W - pw) / 2;
      const oy = 196;
      sv.add(`<rect x="${ox}" y="${oy}" width="${pw}" height="${ph}" rx="24" fill="#fff" fill-opacity=".42" stroke="rgba(255,255,255,.88)" filter="url(#shS)"/>`);
      const gx = ox + PAD;
      const gy = oy + HEAD;
      const cxl = (i) => gx + i * pitch + LW / 2;
      lanes.forEach((lane, i) => {
        const x = gx + i * pitch;
        sv.add(`<rect x="${x - 5}" y="${gy - 12}" width="${LW + 10}" height="${bodyH + 24}" rx="18" fill="#fff" fill-opacity=".34" stroke="rgba(255,255,255,.85)"/>`);
        const goal = meta.goals[lane.goal];
        const tl = wrap((goal && goal.title) || lane.name, 8.6, LW - 40, 2);
        sv.add(`<g class="pm-fold" data-xlane="${i}" data-open="${!foldedLanes.has(i)}"><rect x="${x}" y="${oy + 22}" width="${LW}" height="62" rx="12" fill="#fff" fill-opacity=".9" stroke="rgba(255,255,255,.9)" filter="url(#shS)"/>`
          + `<rect x="${x + 12}" y="${oy + 34}" width="10" height="10" rx="3" fill="${lane.color}"/>`
          + text(x + 30, oy + 43.5, fit(lane.name, 12, LW - 54), 12, INK, 700) + tl.map((ln, j) => text(x + 12, oy + 58 + j * 11, ln, 8.6, MUTED, 500)).join('')
          + kit.chevron(x + LW - 20, oy + 36, !foldedLanes.has(i)) + '</g>');
        const idsL = GE.ids.filter((k) => laneOf(k) === i);
        const d = idsL.filter((k) => nodes.get(k).vis === 'done').length;
        const o = idsL.filter((k) => nodes.get(k).vis !== 'done' && nodes.get(k).vis !== 'cancelled').length;
        sv.add(text(x + 4, oy + 100, t('map.explore.open', { open: o }) + (d ? t('map.explore.done_suffix', { done: d }) : ''), 9.6, FAINT, 600));
      });
      const laneRows = new Map();
      for (const k of shown) {
        if (!laneRows.has(laneOf(k))) laneRows.set(laneOf(k), new Set());
        laneRows.get(laneOf(k)).add(row.get(k));
      }
      const bad = GE.bad;
      const px = (k) => cxl(laneOf(k));
      const pyTop = (k) => gy + rowy[row.get(k)];
      const pyBot = (k) => gy + rowy[row.get(k)] + hh.get(k);
      for (const [b, a] of [...edges].sort((p, q) => (bad.has(pairKey(p[0], p[1])) ? 1 : 0) - (bad.has(pairKey(q[0], q[1])) ? 1 : 0))) {
        const x1 = px(b);
        const y1 = pyBot(b);
        const x2 = px(a);
        const y2 = pyTop(a) - 1;
        const la = laneOf(b);
        const lb = laneOf(a);
        const kind = bad.has(pairKey(b, a)) ? 'cross' : la === lb ? 'dep' : 'xlane';
        let between = false;
        if (la === lb && row.get(a) > row.get(b) + 1) for (let rr = row.get(b) + 1; rr < row.get(a); rr++) if (laneRows.get(la).has(rr)) between = true;
        if (between) { // same lane with cards in between: run down the gutter
          const sx = x1 + LW / 2 + 6;
          sv.add(kit.pathEl(`M${f1(x1)},${f1(y1)} L${f1(x1)},${f1(y1 + 8)} L${f1(sx)},${f1(y1 + 8)} L${f1(sx)},${f1(y2 - 8)} L${f1(x2)},${f1(y2 - 8)} L${f1(x2)},${f1(y2)}`, kind, true, { from: b, to: a }));
          continue;
        }
        const dy = Math.max((y2 - y1) * 0.5, 14);
        sv.add(kit.pathEl(`M${f1(x1)},${f1(y1)} C${f1(x1)},${f1(y1 + dy)} ${f1(x2)},${f1(y2 - dy)} ${f1(x2)},${f1(y2)}`, kind, true, { from: b, to: a }));
      }
      for (const k of shown) sv.add(card3.get(k)(px(k) - LW / 2, pyTop(k))[0]);
      if (bad.size) {
        const badPairs = [...bad].map((key) => key.split('\u0000'));
        const cancelledPre = [...new Set(badPairs.map(([p]) => p))].sort(compare);
        const dependentsBad = [...new Set(badPairs.map(([, d]) => d))].sort(compare);
        const stHere = GE.ids.filter((k) => meta.stuck.has(k));
        const freeHere = [...GE.live].filter((k) => !meta.stuck.has(k)).sort(compare);
        const names = cancelledPre.map((k) => nodes.get(k).short).join(t('map.join.list'));
        const deps = dependentsBad.slice(0, 3).map((k) => nodes.get(k).short).join(t('map.join.list')) + (dependentsBad.length > 3 ? t('map.explore.etc') : '');
        const free = freeHere.length ? compress(freeHere.map((k) => nodes.get(k).short)) : t('map.explore.none_free');
        sv.add(text(ox + PAD, oy + 14, fit(t('map.explore.defect', { names, deps, stuck: stHere.length, free }), 10, pw - 2 * PAD), 10, RED, 650));
      }
      const ly = oy + ph + 22;
      const present = new Set(shown.map((k) => nodes.get(k).vis));
      const visItems = (showDone ? ['done'] : []).concat(['blocked', 'stalled', 'developing', 'ready', 'draft', 'waiting', 'cancelled']).filter((v) => present.has(v));
      const lg = kit.legend(sv, 76, ly, W - 152, visItems, [['dep', t('map.edge.same_lane')], ['xlane', t('map.edge.cross_lane')], ['cross', t('map.edge.to_cancelled')]], true);
      const cx0 = 76 + 6 + 40 + visItems.reduce((s, v) => s + 16 + tw(ST[v].label, 10) + 22, 0)
        + [t('map.edge.same_lane'), t('map.edge.cross_lane'), t('map.edge.to_cancelled')].reduce((s, label) => s + 38 + tw(label, 10) + 22, 0);
      sv.add(`<rect x="${cx0}" y="${ly + 17}" width="22" height="14" rx="5" fill="none" stroke="#e14b54" stroke-opacity=".6" stroke-dasharray="2.5 3"/>`);
      sv.add(text(cx0 + 30, ly + 27.5, t('map.legend.downstream'), 10, MUTED, 500));
      sv.add(text(82, lg + 4, t('map.explore.note_done', { done: ce.done || 0, rows: nrows }), 9.5, FAINT, 500));
      return frame(sv, W, Math.floor(lg + 40));
    }

    return { renderOverview, renderMainline, renderExplore, placeholder, compress, utc, prepare, critical, autoEnd };
  }

  const api = { createViews, utc, STATE_MARK, GROUP_ORDER };
  if (commonJS) module.exports = api;
  else {
    root.AWR_PROJECT_MAP = root.AWR_PROJECT_MAP || {};
    root.AWR_PROJECT_MAP.views = api;
  }
})(typeof globalThis !== 'undefined' ? globalThis : this);
