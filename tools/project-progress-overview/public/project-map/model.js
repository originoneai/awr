/**
 * Display configuration and the renderer data model.
 *
 * `loadModel(snapshot, config)` turns a validated snapshot and a display configuration into the plain structures the SVG
 * views draw: per work item a visual class (done, developing, stalled, blocked, ready, waiting, draft, cancelled), its lane,
 * its dependencies and its idle time. Lanes, the stale threshold and the dependency panels are presentation decisions, not
 * project data, so they come from the configuration; without one they are derived from the snapshot alone.
 * Nothing here reads a file, a ledger or a database.
 */
(function (root) {
  'use strict';

  const commonJS = typeof module !== 'undefined' && module.exports;
  const snapshotLib = commonJS ? require('./snapshot.js') : root.AWR_PROJECT_MAP.snapshot;

  const DAY_MS = 86400000;
  const PALETTE = ['#3b82f6', '#8b5cf6', '#22a06b', '#0ea5c6', '#6366f1', '#8a94a6'];
  const MAX_LANES = 6;
  const ALL_LANE = { id: 'ALL', name: null, name_key: 'map.lane.all', tag: '', color: PALETTE[0] };
  const OTHER_LANE = { id: 'OTHER', name: null, name_key: 'map.lane.other', tag: '', color: PALETTE[5] };

  class ConfigError extends Error {
    constructor(message) {
      super(`display configuration invalid: ${message}`);
      this.name = 'ConfigError';
    }
  }

  const compare = snapshotLib.compare;
  const isObject = (v) => v !== null && typeof v === 'object' && !Array.isArray(v);
  const escapeRegex = (s) => s.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  const COLOR = /^#[0-9a-fA-F]{3,8}$/;
  const TEXT_LIMIT = 200;

  function text(value, where, { required = false } = {}) {
    if (value === undefined || value === null) {
      if (required) throw new ConfigError(`${where} is required`);
      return undefined;
    }
    if (typeof value !== 'string' || value.length > TEXT_LIMIT) throw new ConfigError(`${where} must be text of at most ${TEXT_LIMIT} characters`);
    return value;
  }

  function stringList(value, where) {
    if (value === undefined || value === null) return [];
    if (!Array.isArray(value) || value.some((v) => typeof v !== 'string' || !v || v.length > TEXT_LIMIT)) {
      throw new ConfigError(`${where} must be a list of non-empty text`);
    }
    return [...value];
  }

  /** Split a key into its alphanumeric segments and the separators between them. */
  function familyOf(key) {
    const parts = key.split(/([-_./:])/);
    const segments = parts.filter((_, i) => i % 2 === 0);
    if (segments.length >= 3) return parts.slice(0, 3).join('');
    if (segments.length === 2) return parts[0];
    const letters = /^[A-Za-z]+/.exec(key);
    return letters ? letters[0] : key;
  }

  /** Lanes derived from the keys alone: the commonest key families, the rest in one lane. */
  function deriveLanes(keys) {
    const counts = new Map();
    for (const key of keys) {
      const family = familyOf(key);
      counts.set(family, (counts.get(family) || 0) + 1);
    }
    const families = [...counts.entries()].sort((a, b) => b[1] - a[1] || compare(a[0], b[0])).map(([family]) => family);
    if (!families.length) return { lanes: [{ ...ALL_LANE }], match_order: [], default_lane: ALL_LANE.id };
    const named = families.length > MAX_LANES ? families.slice(0, MAX_LANES - 1) : families;
    const used = new Set();
    const lanes = named.map((family, i) => {
      let id = family.toUpperCase().replace(/[^A-Z0-9]+/g, '_').replace(/^_+|_+$/g, '') || 'LANE';
      while (used.has(id)) id += `_${i}`;
      used.add(id);
      return { id, name: family, tag: `${family}-*`, color: PALETTE[i % PALETTE.length], key_regex: `^${escapeRegex(family)}(?![A-Za-z])` };
    });
    const match_order = lanes.map((l) => l.id);
    if (families.length > MAX_LANES) {
      const id = used.has(OTHER_LANE.id) ? `${OTHER_LANE.id}_REST` : OTHER_LANE.id;
      lanes.push({ ...OTHER_LANE, id });
      return { lanes, match_order, default_lane: id };
    }
    // Every key matches its own family, so the default lane only catches keys of a configuration that left some out.
    return { lanes, match_order, default_lane: lanes[lanes.length - 1].id };
  }

  function normalizeLane(raw, i) {
    if (!isObject(raw)) throw new ConfigError(`overview.lanes[${i}] must be an object`);
    const id = text(raw.id, `overview.lanes[${i}].id`, { required: true });
    const name = text(raw.name, `overview.lanes[${i}].name`, { required: true });
    const color = text(raw.color, `overview.lanes[${i}].color`, { required: true });
    if (!COLOR.test(color)) throw new ConfigError(`overview.lanes[${i}].color must be a hex color such as #3b82f6`);
    const lane = { id, name, tag: text(raw.tag, `overview.lanes[${i}].tag`) || '', color };
    const regex = text(raw.key_regex, `overview.lanes[${i}].key_regex`);
    if (regex !== undefined) {
      try {
        new RegExp(regex); // eslint-disable-line no-new
      } catch (e) {
        throw new ConfigError(`overview.lanes[${i}].key_regex is not a valid expression`);
      }
      lane.key_regex = regex;
    }
    return lane;
  }

  /**
   * Validate a user configuration (any subset of the documented fields) and fill every missing part from the snapshot.
   * The result is fully populated and safe to embed: colors are hex, text is bounded, expressions compile.
   */
  function normalizeConfig(user, snapshot) {
    const raw = user === undefined || user === null ? {} : user;
    if (!isObject(raw)) throw new ConfigError('the configuration must be an object');
    if (raw.version !== undefined && raw.version !== 1) throw new ConfigError('version must be 1');
    const stale = raw.stale_days === undefined ? 7 : raw.stale_days;
    if (!Number.isInteger(stale) || stale < 1 || stale > 3650) throw new ConfigError('stale_days must be an integer from 1 to 3650');
    const keys = snapshot.nodes.map((n) => n.key);
    const cfg = {
      version: 1,
      stale_days: stale,
      project_goal: text(raw.project_goal, 'project_goal') || '',
      titles: {},
      strip_prefixes: stringList(raw.strip_prefixes, 'strip_prefixes'),
      resolve_milestones_extra: stringList(raw.resolve_milestones_extra, 'resolve_milestones_extra'),
    };
    if (raw.titles !== undefined) {
      if (!isObject(raw.titles)) throw new ConfigError('titles must be an object');
      for (const name of ['page', 'overview', 'mainline', 'explore']) {
        const value = text(raw.titles[name], `titles.${name}`);
        if (value) cfg.titles[name] = value;
      }
    }

    const overview = raw.overview === undefined ? {} : raw.overview;
    if (!isObject(overview)) throw new ConfigError('overview must be an object');
    if (overview.lanes === undefined) {
      cfg.overview = deriveLanes(keys);
    } else {
      if (!Array.isArray(overview.lanes) || overview.lanes.length < 1 || overview.lanes.length > 12) {
        throw new ConfigError('overview.lanes must list 1 to 12 lanes');
      }
      const lanes = overview.lanes.map(normalizeLane);
      const ids = new Set(lanes.map((l) => l.id));
      if (ids.size !== lanes.length) throw new ConfigError('overview.lanes ids must be unique');
      const order = overview.match_order === undefined ? lanes.filter((l) => l.key_regex).map((l) => l.id) : stringList(overview.match_order, 'overview.match_order');
      for (const id of order) {
        const lane = lanes.find((l) => l.id === id);
        if (!lane) throw new ConfigError(`overview.match_order names an unknown lane: ${id}`);
        if (!lane.key_regex) throw new ConfigError(`lane ${id} is in match_order but has no key_regex`);
      }
      const fallback = overview.default_lane === undefined ? lanes[lanes.length - 1].id : text(overview.default_lane, 'overview.default_lane');
      if (!ids.has(fallback)) throw new ConfigError(`overview.default_lane names an unknown lane: ${fallback}`);
      cfg.overview = { lanes, match_order: order, default_lane: fallback };
    }

    const laneIds = new Set(cfg.overview.lanes.map((l) => l.id));
    if (raw.mainline_panels === undefined) {
      cfg.mainline_panels = null; // derived from the data below, once lanes are known
    } else {
      if (!Array.isArray(raw.mainline_panels) || raw.mainline_panels.length > 12) throw new ConfigError('mainline_panels must be a list of at most 12 panels');
      cfg.mainline_panels = raw.mainline_panels.map((panel, i) => {
        if (!isObject(panel)) throw new ConfigError(`mainline_panels[${i}] must be an object`);
        const lane = text(panel.lane, `mainline_panels[${i}].lane`, { required: true });
        if (!laneIds.has(lane)) throw new ConfigError(`mainline_panels[${i}].lane names an unknown lane: ${lane}`);
        const out = { lane };
        const end = text(panel.crit_end, `mainline_panels[${i}].crit_end`);
        if (end) out.crit_end = end;
        if (panel.keep_done !== undefined) {
          if (typeof panel.keep_done !== 'boolean') throw new ConfigError(`mainline_panels[${i}].keep_done must be true or false`);
          out.keep_done = panel.keep_done;
        }
        return out;
      });
    }

    if (raw.explore_lanes === undefined) {
      cfg.explore_lanes = [];
    } else {
      if (!Array.isArray(raw.explore_lanes) || raw.explore_lanes.length > 24) throw new ConfigError('explore_lanes must be a list of at most 24 lanes');
      const claimed = new Set();
      cfg.explore_lanes = raw.explore_lanes.map((lane, i) => {
        if (!isObject(lane)) throw new ConfigError(`explore_lanes[${i}] must be an object`);
        const color = text(lane.color, `explore_lanes[${i}].color`, { required: true });
        if (!COLOR.test(color)) throw new ConfigError(`explore_lanes[${i}].color must be a hex color such as #3b82f6`);
        const milestones = stringList(lane.milestones, `explore_lanes[${i}].milestones`);
        if (!milestones.length) throw new ConfigError(`explore_lanes[${i}].milestones must name at least one milestone`);
        for (const m of milestones) {
          if (claimed.has(m)) throw new ConfigError(`milestone ${m} belongs to more than one explore lane`);
          claimed.add(m);
        }
        return { goal: text(lane.goal, `explore_lanes[${i}].goal`) || '', name: text(lane.name, `explore_lanes[${i}].name`, { required: true }), color, milestones };
      });
    }
    return cfg;
  }

  /** Milestones the extractor has to resolve: the explore lanes' plus the explicit extras. */
  function milestonesToResolve(cfg) {
    const names = new Set(cfg.resolve_milestones_extra);
    for (const lane of cfg.explore_lanes) for (const m of lane.milestones) names.add(m);
    return [...names].sort(compare);
  }

  function shortKey(key, cfg) {
    for (const prefix of cfg.strip_prefixes || []) if (prefix && key.startsWith(prefix) && key.length > prefix.length) return key.slice(prefix.length);
    return key;
  }

  function laneName(lane, t) {
    return lane.name !== null && lane.name !== undefined ? lane.name : t(lane.name_key);
  }

  function parseGeneratedAt(value) {
    if (typeof value !== 'string' || !/^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\dZ$/.test(value)) throw new snapshotLib.SnapshotError('generated_at must be YYYY-MM-DDTHH:MM:SSZ');
    return Date.parse(value);
  }

  /** `{nodes: Map<key, node>, meta}` for the views. The generation time of the snapshot is the renderers' only clock. */
  function loadModel(snapshot, cfgInput) {
    snapshotLib.validate(snapshot);
    const cfg = normalizeConfig(cfgInput, snapshot);
    const generatedMs = parseGeneratedAt(snapshot.generated_at);
    const laneRules = cfg.overview.match_order.map((id) => [id, new RegExp(cfg.overview.lanes.find((l) => l.id === id).key_regex)]);
    const laneOfKey = (key) => {
      for (const [id, rule] of laneRules) {
        const m = rule.exec(key);
        if (m && m.index === 0) return id;
      }
      return cfg.overview.default_lane;
    };

    const prerequisites = new Map();
    for (const e of snapshot.edges) {
      if (!prerequisites.has(e.dependent)) prerequisites.set(e.dependent, []);
      prerequisites.get(e.dependent).push(e.prerequisite);
    }
    const statusOf = new Map(snapshot.nodes.map((n) => [n.key, n.status]));
    const nodes = new Map();
    for (const n of snapshot.nodes) {
      const deps = [...(prerequisites.get(n.key) || [])].sort(compare);
      const idle = n.last_event_at ? (generatedMs - n.last_event_at) / DAY_MS : null;
      let vis;
      if (n.status === 'cancelled') vis = 'cancelled';
      else if (n.status === 'completed') vis = 'done';
      else if (n.status === 'blocked') vis = 'blocked';
      else if (n.status === 'in_progress' || n.status === 'claimed') vis = idle === null || idle >= cfg.stale_days ? 'stalled' : 'developing';
      else if (n.status === 'draft') vis = 'draft';
      else vis = deps.some((d) => statusOf.get(d) !== 'completed') ? 'waiting' : 'ready';
      const cancelled = n.status === 'cancelled';
      nodes.set(n.key, {
        id: n.key, short: shortKey(n.key, cfg), title: n.title, status: n.status, vis, lane: laneOfKey(n.key), deps,
        owner: cancelled ? null : (n.claims.length ? n.claims[0].agent_id : n.owner),
        live: n.claims.length > 0 && !cancelled, idle: cancelled ? null : idle, blocker: cancelled ? null : n.blocker, milestone: n.milestone,
      });
    }
    const cancelled = new Set([...nodes.values()].filter((n) => n.vis === 'cancelled').map((n) => n.id));
    const stuck = new Set();
    for (let changed = true; changed;) {
      changed = false;
      for (const n of nodes.values()) {
        if (n.vis === 'done' || n.vis === 'cancelled' || stuck.has(n.id)) continue;
        if (n.deps.some((d) => cancelled.has(d) || stuck.has(d))) {
          stuck.add(n.id);
          changed = true;
        }
      }
    }
    const severities = new Map();
    const codes = {};
    for (const f of snapshot.doctor.findings) {
      severities.set(f.code, f.severity);
      codes[f.code] = (codes[f.code] || 0) + 1;
    }
    const count = (severity) => snapshot.doctor.findings.filter((f) => f.severity === severity).length;
    const claims = {};
    for (const n of snapshot.nodes) if (n.claims.length) claims[n.key] = n.claims[0].agent_id;

    // Lanes the data fills but the configuration did not choose for a panel: the largest with an internal dependency.
    let panels = cfg.mainline_panels;
    if (panels === null) {
      const stats = cfg.overview.lanes.map((lane) => {
        const members = [...nodes.values()].filter((n) => n.lane === lane.id);
        const inner = members.reduce((sum, n) => sum + n.deps.filter((d) => nodes.get(d).lane === lane.id).length, 0);
        const open = members.some((n) => n.vis !== 'done' && n.vis !== 'cancelled');
        return { lane: lane.id, size: members.length, inner, open };
      });
      panels = stats.filter((s) => s.open && s.inner > 0).sort((a, b) => b.size - a.size || compare(a.lane, b.lane)).slice(0, 3).map((s) => ({ lane: s.lane }));
    }

    const meta = {
      sessions_active: snapshot.sessions.length,
      revision: snapshot.project.revision,
      claims,
      cancelled: [...cancelled].sort(compare),
      stuck,
      doctor: { error: count('error'), warning: count('warning'), info: count('info'), codes, severity_by_code: Object.fromEntries(severities) },
      goals: Object.fromEntries(snapshot.goals.map((g) => [g.id, g])),
      unavailable: snapshot.unavailable,
      sessions: snapshot.sessions,
      generated_at: generatedMs,
      fingerprint: snapshot.fingerprint,
      project: snapshot.project,
      config: { ...cfg, mainline_panels: panels },
    };
    return { nodes, meta, cfg: meta.config, raw: new Map(snapshot.nodes.map((n) => [n.key, n])), snapshot };
  }

  const api = { ConfigError, PALETTE, familyOf, deriveLanes, normalizeConfig, milestonesToResolve, shortKey, laneName, loadModel, escapeRegex };
  if (commonJS) module.exports = api;
  else {
    root.AWR_PROJECT_MAP = root.AWR_PROJECT_MAP || {};
    root.AWR_PROJECT_MAP.model = api;
  }
})(typeof globalThis !== 'undefined' ? globalThis : this);
