/**
 * Snapshot extraction from the official `awr --json` commands, and nothing else.
 *
 * The caller supplies `call(name, params)`, which runs one read-only command and resolves to its parsed JSON (or rejects with
 * an AwrCallError). In the Inspector page `call` goes through the bridge; in the static export it spawns the CLI. No ledger,
 * YAML file or state database is read anywhere. Commands used:
 *   workGraph  work graph [--limit N] [--cached]   nodes, readiness, active claims, dependency edges
 *   nav        nav --cached [--milestone M]        owner, blocker, next action, waits, project identity, milestone members
 *   goals      search --type goal --cached         goal titles
 *   events     event history (paged)               last activity per work item and per session (skipped when work graph has it)
 *   sessions   session list --active               sessions
 *   doctor     doctor                              health findings
 * Every answer must report the same project revision; a revision change during a run is retried a bounded number of times
 * and then fails with RevisionDrift, so a snapshot never mixes two states.
 */
(function (root) {
  'use strict';

  const commonJS = typeof module !== 'undefined' && module.exports;
  const snapshotLib = commonJS ? require('./snapshot.js') : root.AWR_PROJECT_MAP.snapshot;
  const modelLib = commonJS ? require('./model.js') : root.AWR_PROJECT_MAP.model;

  const GRAPH_CAP = 1000; // work graph refuses --limit above this
  const TRANSIENT = new Set(['RevisionConflict', 'MutationConflict', 'SourceConflict', 'SourceStale', 'BridgeBusy', 'NoOutput']);
  const MAX_EVENT_PAGES = 500;

  class AwrCallError extends Error {
    constructor(code, message, details = null) {
      super(`${code}: ${message}`);
      this.name = 'AwrCallError';
      this.code = code;
      this.detail = message;
      this.details = details;
    }
  }

  const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
  const compare = snapshotLib.compare;

  /** Run `fn(item)` over `items` with at most `limit` in flight; results keep the order of `items`. */
  async function mapLimit(items, limit, fn) {
    const results = new Array(items.length);
    let next = 0;
    async function worker() {
      while (next < items.length) {
        const i = next;
        next += 1;
        results[i] = await fn(items[i]);
      }
    }
    await Promise.all(Array.from({ length: Math.min(limit, items.length) }, worker));
    return results;
  }

  /** Adaptive node limit: start at the default and follow BudgetExceeded.details.required (the exact node count). */
  async function readWorkGraph(call, cached) {
    let limit = 100;
    for (let attempt = 0; attempt < 3; attempt++) {
      try {
        return await call('workGraph', { limit, cached });
      } catch (e) {
        if (cached && e instanceof AwrCallError && e.code === 'InvalidInput' && /--cached/.test(e.detail)) {
          throw new AwrCallError('CachedUnsupported', 'this awr version cannot read the work graph without refreshing it (no work graph --cached)');
        }
        if (!(e instanceof AwrCallError) || e.code !== 'BudgetExceeded' || !e.details || e.details.required === undefined) throw e;
        const need = Number(e.details.required);
        if (!Number.isInteger(need) || need < 1) throw e;
        if (need > GRAPH_CAP) throw new AwrCallError('ProjectTooLarge', `the project has ${need} work items; work graph serves at most ${GRAPH_CAP} per call`);
        limit = need;
      }
    }
    throw new AwrCallError('LimitNotConverging', 'work graph kept reporting BudgetExceeded');
  }

  async function extractOnce(call, cfg, options) {
    const progress = options.onProgress || (() => {});
    // The graph read is the only call that may refresh the projection; everything after it is a recorded-state read.
    progress('workGraph');
    const graph = await readWorkGraph(call, options.cached);
    if (!graph.complete || !graph.graph_valid) {
      throw new AwrCallError('GraphIncomplete', `work graph complete=${graph.complete} valid=${graph.graph_valid}`);
    }
    const R = graph.project_revision;
    const same = (label, value) => {
      if (value.project_revision !== R) throw new AwrCallError('RevisionDrift', `${label} reported revision ${value.project_revision}, work graph ${R}`);
      return value;
    };
    progress('goals');
    const goalsRaw = same('search', await call('goals', {}));
    progress('nav');
    const nav = same('nav', await call('nav', {}));
    const navNodes = new Map(nav.mainline_graph.nodes.map((n) => [n.work_key, n]));
    const graphKeys = new Set(graph.nodes.map((n) => n.key));
    const mismatch = [...new Set([...navNodes.keys(), ...graphKeys])].filter((k) => navNodes.has(k) !== graphKeys.has(k)).sort(compare);
    if (mismatch.length) throw new AwrCallError('NodeSetMismatch', `nav and work graph disagree on the node set: ${mismatch.slice(0, 5).join(', ')}`);

    const milestones = modelLib.milestonesToResolve(cfg);
    progress('milestones');
    const milestoneOf = new Map();
    const resolved = await mapLimit(milestones, options.concurrency || 3, async (m) => {
      const v = same(`nav --milestone ${m}`, await call('nav', { milestone: m }));
      return [m, v.scope.selection];
    });
    for (const [m, keys] of resolved) {
      for (const k of keys) {
        if (milestoneOf.has(k) && milestoneOf.get(k) !== m) throw new AwrCallError('MilestoneConflict', `${k} is selected by both ${milestoneOf.get(k)} and ${m}`);
        milestoneOf.set(k, m);
      }
    }

    // Activity: the graph carries it when the CLI is new enough; otherwise it is read from the event history.
    const haveNodeActivity = graph.nodes.length > 0 && graph.nodes.every((n) => Object.prototype.hasOwnProperty.call(n, 'last_event_at'));
    const lastByWork = new Map();
    const lastBySession = new Map();
    let eventCount = 0;
    let pages = 0;
    if (!haveNodeActivity) {
      progress('events');
      const seen = new Set();
      let cursor = null;
      for (;;) {
        const page = same('event history', await call('events', { limit: 1000, through: R, cursor }));
        pages += 1;
        for (const e of page.events) {
          if (seen.has(e.id)) throw new AwrCallError('CursorLoop', `event ${e.id} returned twice`);
          seen.add(e.id);
          for (const [table, key] of [[lastByWork, e.work_item_id], [lastBySession, e.session_id]]) {
            if (key && e.created_at > (table.get(key) || 0)) table.set(key, e.created_at);
          }
        }
        cursor = page.next_cursor || null;
        if (!cursor) break;
        if (pages > MAX_EVENT_PAGES) throw new AwrCallError('TooManyPages', 'event history did not terminate');
      }
      eventCount = seen.size;
    }
    progress('sessions');
    const sl = same('session list', await call('sessions', { limit: 100 }));
    progress('doctor');
    const doctor = same('doctor', await call('doctor', {}));

    const keyOfId = new Map(graph.nodes.map((n) => [n.id, n.key]));
    const nodes = [...graph.nodes].sort((a, b) => compare(a.key, b.key)).map((gn) => {
      const nn = navNodes.get(gn.key);
      if (nn.status !== gn.status) throw new AwrCallError('StatusMismatch', `${gn.key}: work graph ${gn.status} vs nav ${nn.status}`);
      const last = haveNodeActivity ? gn.last_event_at : lastByWork.get(gn.id);
      return {
        key: gn.key, id: gn.id, title: gn.title, status: gn.status, ready: gn.ready, archived: gn.archived,
        milestone: milestoneOf.get(gn.key) || null, owner: nn.person_responsibility ?? null, blocker: nn.blocker ?? null, next_action: nn.next_action ?? null,
        waits: (nn.explainable_waits || []).map((w) => ({ kind: w.kind, summary: w.summary, release_condition: w.release_condition })),
        claims: [...gn.active_claims].sort((a, b) => compare(a.id, b.id)).map((c) => ({ id: c.id, agent_id: c.agent_id, session_id: c.session_id, acquired_at: c.acquired_at, expires_at: c.expires_at })),
        diagnostics: gn.diagnostics.map((d) => ({ code: d.code, detail: d.detail, work_item_key: d.work_item_key ?? null })),
        last_event_at: last === undefined ? null : last,
      };
    });
    const edges = graph.edges.map((e) => ({ dependent: e.from, prerequisite: e.to, required: e.required }))
      .sort((a, b) => compare(a.dependent, b.dependent) || compare(a.prerequisite, b.prerequisite));
    const sessions = sl.sessions.map((s) => ({
      id: s.id, agent_id: s.agent_id, work_key: keyOfId.get(s.work_item_id) ?? null, status: s.status, started_at: s.started_at,
      last_event_at: lastBySession.get(s.id) ?? null, last_checkpoint_id: s.last_checkpoint_id ?? null,
    })).sort((a, b) => a.started_at - b.started_at || compare(a.id, b.id));
    const goals = goalsRaw.hits.map((h) => ({ id: h.external_key, title: h.title, status: h.status })).sort((a, b) => compare(a.id, b.id));
    const findings = doctor.findings.map((f) => ({ code: f.code, severity: f.severity, object_kind: f.object_kind, object_id: f.object_id, message: f.message }))
      .sort((a, b) => compare(a.code, b.code) || compare(a.object_id, b.object_id));
    return {
      schema: snapshotLib.SCHEMA, version: snapshotLib.VERSION,
      project: { key: nav.project.external_key, name: nav.project.name, id: nav.project.id, revision: R },
      basis: {
        work_graph: { graph_fingerprint: graph.graph_fingerprint, node_count: nodes.length, edge_count: edges.length },
        nav: { protocol: nav.protocol, schema_version: nav.schema_version, freshness_basis: nav.freshness_basis },
        milestones_resolved: milestones,
        events: { through_revision: R, count: eventCount, pages, source: haveNodeActivity ? 'work_graph' : 'event_history' },
        sessions: { count: sessions.length, truncated: Boolean(sl.may_have_more) },
        goals: { truncated: Boolean(goalsRaw.truncated) },
      },
      nodes, edges, goals, sessions,
      doctor: { ok: doctor.ok, findings },
      unavailable: snapshotLib.UNAVAILABLE.map((u) => ({ field: u.field, reason: u.reason })),
    };
  }

  /** Retry transient failures of one command a few times (the project may be written concurrently). */
  function withRetry(call, { retries = 3, delayMs = 1000, wait = sleep } = {}) {
    return async (name, params) => {
      let last;
      for (let attempt = 1; attempt <= retries; attempt++) {
        try {
          return await call(name, params);
        } catch (e) {
          if (!(e instanceof AwrCallError) || !TRANSIENT.has(e.code)) throw e;
          last = e;
          if (attempt < retries) await wait(delayMs * attempt);
        }
      }
      throw last;
    };
  }

  /**
   * Take one snapshot. `now` is the ISO time recorded as `generated_at` (default: the current UTC time, to the second).
   * Options: `cached` (read the recorded state only; fails when the CLI has no `work graph --cached`), `attempts`
   * (whole-run retries on RevisionDrift), `wait`, `onProgress(step, attempt)`.
   */
  async function extractSnapshot({ call, config, now, cached = false, attempts = 8, driftWaitMs = 2000, wait = sleep, onProgress, concurrency = 3 }) {
    const cfg = modelLib.normalizeConfig(config, { nodes: [] });
    const guarded = withRetry(call, { wait });
    const generatedAt = now || new Date().toISOString().replace(/\.\d{3}Z$/, 'Z');
    let last;
    for (let attempt = 1; attempt <= attempts; attempt++) {
      try {
        const content = await extractOnce(guarded, cfg, { cached, onProgress, concurrency });
        return snapshotLib.seal(content, generatedAt);
      } catch (e) {
        if (!(e instanceof AwrCallError) || e.code !== 'RevisionDrift') throw e;
        last = e;
        if (attempt < attempts) {
          if (onProgress) onProgress('retry', attempt + 1);
          await wait(driftWaitMs);
        }
      }
    }
    throw new AwrCallError('RevisionDrift', `the project revision kept changing for ${attempts} attempts: ${last.detail}`);
  }

  const api = { AwrCallError, extractSnapshot, readWorkGraph, withRetry, mapLimit, GRAPH_CAP };
  if (commonJS) module.exports = api;
  else {
    root.AWR_PROJECT_MAP = root.AWR_PROJECT_MAP || {};
    root.AWR_PROJECT_MAP.extract = api;
  }
})(typeof globalThis !== 'undefined' ? globalThis : this);
