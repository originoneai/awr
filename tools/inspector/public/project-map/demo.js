/**
 * Synthetic demonstration project for the Project map: 20 work items that cover every visual state (done, in progress with a
 * live claim, stalled, blocked, ready, waiting, draft, cancelled with a chain stuck behind it), two lanes, a cross-lane
 * dependency, sessions and health findings. It is built from catalog texts so the demo reads in the selected UI language,
 * has a fixed clock, and is a valid snapshot like any other. Demo mode and the tests use it; it never touches a project.
 */
(function (root) {
  'use strict';

  const commonJS = typeof module !== 'undefined' && module.exports;
  const snapshotLib = commonJS ? require('./snapshot.js') : root.AWR_PROJECT_MAP.snapshot;

  const GENERATED_AT = '2026-01-15T00:00:00Z';
  const GENERATED_MS = Date.parse(GENERATED_AT);
  const DAY = 86400000;
  const HOUR = 3600000;

  /** @returns {{snapshot: object, config: object}} */
  function buildDemo(t) {
    const node = (key, name, status, deps = [], o = {}) => ({
      key, id: `demo-id-${key}`, title: t(`map.demo.${name}`), status, ready: false, archived: false, milestone: o.milestone || null,
      owner: o.owner || null, blocker: o.blocker ? t(`map.demo.${o.blocker}`) : null, next_action: null, waits: [],
      claims: o.claim ? [{ id: `claim-${key}`, agent_id: o.claim, session_id: `sess-${o.claim}`, acquired_at: GENERATED_MS - DAY, expires_at: GENERATED_MS + 3 * HOUR }] : [],
      diagnostics: (o.cancelled || []).map((k) => ({ code: 'dependency_not_completed', detail: 'required dependency has source status cancelled', work_item_key: k })),
      last_event_at: o.idle === undefined ? null : GENERATED_MS - Math.round(o.idle * DAY), _deps: deps,
    });
    const stuck = { cancelled: ['DEMO-A-011'] };
    const nodes = [
      node('DEMO-A-001', 'a001', 'completed', [], { milestone: 'M1' }),
      node('DEMO-A-002', 'a002', 'completed', ['DEMO-A-001'], { milestone: 'M1' }),
      node('DEMO-A-003', 'a003', 'completed', ['DEMO-A-001'], { milestone: 'M2' }),
      node('DEMO-A-004', 'a004', 'in_progress', ['DEMO-A-002', 'DEMO-A-003'], { milestone: 'M1', owner: 'demo-agent-1', claim: 'demo-agent-1', idle: 1 }),
      node('DEMO-A-005', 'a005', 'planned', ['DEMO-A-004'], { milestone: 'M1' }),
      node('DEMO-A-006', 'a006', 'planned', ['DEMO-A-005'], { milestone: 'M2' }),
      node('DEMO-A-007', 'a007', 'planned', ['DEMO-A-003'], { milestone: 'M2' }),
      node('DEMO-A-008', 'a008', 'blocked', ['DEMO-A-003'], { milestone: 'M2', blocker: 'a008_blocker' }),
      node('DEMO-A-009', 'a009', 'in_progress', [], { milestone: 'M1', owner: 'demo-agent-2', idle: 20 }),
      node('DEMO-A-010', 'a010', 'draft'),
      node('DEMO-A-011', 'a011', 'cancelled', ['DEMO-A-001'], { milestone: 'M1' }),
      node('DEMO-A-012', 'a012', 'planned', ['DEMO-A-011', 'DEMO-A-002'], { milestone: 'M1', ...stuck }),
      node('DEMO-A-013', 'a013', 'planned', ['DEMO-A-012'], { milestone: 'M2', ...stuck }),
      node('DEMO-A-014', 'a014', 'planned', ['DEMO-A-013', 'DEMO-A-007'], { milestone: 'M2', ...stuck }),
      node('DEMO-B-001', 'b001', 'completed', [], { milestone: 'M1' }),
      node('DEMO-B-002', 'b002', 'in_progress', ['DEMO-B-001'], { milestone: 'M1', owner: 'demo-agent-3', claim: 'demo-agent-3', idle: 0.2 }),
      node('DEMO-B-003', 'b003', 'planned', ['DEMO-B-002', 'DEMO-A-002'], { milestone: 'M2' }),
      node('DEMO-B-004', 'b004', 'planned', [], { milestone: 'M2' }),
      node('DEMO-C-001', 'c001', 'planned'),
      node('DEMO-C-002', 'c002', 'completed'),
    ].sort((a, b) => (a.key < b.key ? -1 : a.key > b.key ? 1 : 0));
    const edges = nodes.flatMap((n) => n._deps.map((d) => ({ dependent: n.key, prerequisite: d, required: true })))
      .sort((a, b) => (a.dependent < b.dependent ? -1 : a.dependent > b.dependent ? 1 : a.prerequisite < b.prerequisite ? -1 : a.prerequisite > b.prerequisite ? 1 : 0));
    for (const n of nodes) delete n._deps;
    const sessions = [
      { id: 'sess-demo-agent-1', agent_id: 'demo-agent-1', work_key: 'DEMO-A-004', status: 'active', started_at: GENERATED_MS - DAY, last_event_at: GENERATED_MS - DAY, last_checkpoint_id: null },
      { id: 'sess-demo-agent-3', agent_id: 'demo-agent-3', work_key: 'DEMO-B-002', status: 'active', started_at: GENERATED_MS - Math.round(DAY / 2), last_event_at: GENERATED_MS - Math.round(DAY / 5), last_checkpoint_id: 'cp-1' },
    ];
    const findings = [
      { code: 'active_session', severity: 'info', object_kind: 'session', object_id: 'sess-demo-agent-1', message: 'Session is active' },
      { code: 'expired_claim', severity: 'warning', object_kind: 'claim', object_id: 'claim-old', message: 'Active claim expired' },
      { code: 'orphan_session', severity: 'error', object_kind: 'session', object_id: 'sess-old', message: 'Bound work item is terminal' },
    ];
    const content = {
      schema: snapshotLib.SCHEMA, version: snapshotLib.VERSION,
      project: { key: 'demo-project', name: t('map.demo.project'), id: 'demo-id', revision: 100 },
      basis: {
        work_graph: { graph_fingerprint: `sha256:${'0'.repeat(64)}`, node_count: nodes.length, edge_count: edges.length },
        nav: { protocol: 'awr-mainline-nav', schema_version: 1, freshness_basis: 'last_recorded_source_state' },
        milestones_resolved: ['M1', 'M2'], events: { through_revision: 100, count: 0, pages: 0, source: 'demo' },
        sessions: { count: sessions.length, truncated: false }, goals: { truncated: false },
      },
      nodes, edges,
      goals: [{ id: 'DEMO-G-ONE', title: t('map.demo.goal_one'), status: 'active' }, { id: 'DEMO-G-TWO', title: t('map.demo.goal_two'), status: 'active' }],
      sessions, doctor: { ok: false, findings },
      unavailable: snapshotLib.UNAVAILABLE.map((u) => ({ field: u.field, reason: u.reason })),
    };
    const config = {
      version: 1, stale_days: 7, project_goal: t('map.demo.project_goal'),
      overview: {
        match_order: ['ALPHA', 'BETA'], default_lane: 'GAMMA',
        lanes: [
          { id: 'ALPHA', name: t('map.demo.lane_a'), tag: 'DEMO-A', color: '#3b82f6', key_regex: '^DEMO-A-' },
          { id: 'BETA', name: t('map.demo.lane_b'), tag: 'DEMO-B', color: '#8b5cf6', key_regex: '^DEMO-B-' },
          { id: 'GAMMA', name: t('map.demo.lane_c'), tag: 'DEMO-C', color: '#22a06b' },
        ],
      },
      mainline_panels: [{ lane: 'ALPHA' }, { lane: 'BETA', keep_done: true }],
      explore_lanes: [
        { goal: 'DEMO-G-ONE', name: t('map.demo.explore_one'), color: '#3b82f6', milestones: ['M1'] },
        { goal: 'DEMO-G-TWO', name: t('map.demo.explore_two'), color: '#8b5cf6', milestones: ['M2'] },
      ],
    };
    return { snapshot: snapshotLib.seal(content, GENERATED_AT), config };
  }

  const api = { buildDemo, GENERATED_AT };
  if (commonJS) module.exports = api;
  else {
    root.AWR_PROJECT_MAP = root.AWR_PROJECT_MAP || {};
    root.AWR_PROJECT_MAP.demo = api;
  }
})(typeof globalThis !== 'undefined' ? globalThis : this);
