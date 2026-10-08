'use strict';

/**
 * A fake awr for the project-map tests: answers the official commands behind the snapshot from a snapshot object, using the
 * JSON shapes of the real CLI (work graph, nav, search, event history, session list, doctor). `run(argv)` takes the full
 * argument list (including --project and --json) and returns {code, stdout, stderr} like a process would.
 *
 * Options:
 *   cachedSupported  false: reject `work graph --cached` the way an older awr does
 *   nodeActivity     true: work graph nodes carry last_event_at (newer awr)
 *   pageSize         cap on events per history page (to exercise paging); default: the requested limit
 *   revision         number or function(callIndex) giving the project_revision of each answer (to simulate drift)
 */

function createFakeCli(snapshot, options = {}) {
  const opts = Object.assign({ cachedSupported: true, nodeActivity: false }, options);
  const calls = [];
  let counter = 0;
  const revisionNow = () => {
    counter += 1;
    if (typeof opts.revision === 'function') return opts.revision(counter);
    return opts.revision === undefined ? snapshot.project.revision : opts.revision;
  };
  const error = (code, message, details) => ({ code: 1, stdout: '', stderr: JSON.stringify(details ? { code, message, details } : { code, message }) });
  const ok = (value, code = 0) => ({ code, stdout: JSON.stringify(value), stderr: '' });
  const flag = (args, name) => args.includes(name);
  const option = (args, name) => (args.includes(name) ? args[args.indexOf(name) + 1] : undefined);

  const eventsList = () => {
    const events = [];
    for (const n of snapshot.nodes) if (n.last_event_at) events.push({ id: `EV-W-${n.key}`, work_item_id: n.id, session_id: null, created_at: n.last_event_at });
    for (const s of snapshot.sessions) if (s.last_event_at) events.push({ id: `EV-S-${s.id}`, work_item_id: null, session_id: s.id, created_at: s.last_event_at });
    return events.sort((a, b) => a.created_at - b.created_at || (a.id < b.id ? -1 : 1));
  };

  function run(argv) {
    const args = [];
    for (let i = 0; i < argv.length; i++) {
      if (argv[i] === '--project') i += 1;
      else if (argv[i] !== '--json') args.push(argv[i]);
    }
    calls.push(args);
    const [a, b] = args;
    const revision = revisionNow();
    const project = snapshot.project;
    if (a === 'work' && b === 'graph') {
      if (flag(args, '--cached') && !opts.cachedSupported) return error('InvalidInput', "invalid input: error: unexpected argument '--cached' found");
      const limit = Number(option(args, '--limit') || 100);
      if (!(limit >= 1 && limit <= 1000)) return error('InvalidInput', 'graph limit must be 1..1000 and roots at most 100');
      if (snapshot.nodes.length > limit) return error('BudgetExceeded', `context budget exceeded: required ${snapshot.nodes.length}, budget ${limit}`, { budget: limit, required: snapshot.nodes.length });
      const source_ref = { source_id: 'S', locator: 'ledger', source_revision: 1, source_fingerprint: 'fp' };
      return ok({
        ok: true, project_revision: revision, complete: true, graph_valid: true, graph_fingerprint: snapshot.basis.work_graph.graph_fingerprint,
        freshness_basis: flag(args, '--cached') ? 'last_recorded_source_state' : 'source_refresh', read_only: flag(args, '--cached'),
        nodes: snapshot.nodes.map((n) => {
          const node = { key: n.key, id: n.id, title: n.title, status: n.status, archived: n.archived, ready: n.ready, paths: [], diagnostics: n.diagnostics, active_claims: n.claims, source_ref };
          if (opts.nodeActivity) node.last_event_at = n.last_event_at;
          return node;
        }),
        edges: snapshot.edges.map((e) => ({ from: e.dependent, to: e.prerequisite, required: e.required, source_ref })),
      });
    }
    if (a === 'nav') {
      const milestone = option(args, '--milestone');
      const members = snapshot.nodes.filter((n) => milestone === undefined || n.milestone === milestone);
      return ok({
        ok: true, protocol: snapshot.basis.nav.protocol, schema_version: snapshot.basis.nav.schema_version, freshness_basis: snapshot.basis.nav.freshness_basis, project_revision: revision,
        project: { id: project.id, name: project.name, external_key: project.key, revision },
        scope: { selection: members.map((n) => n.key).sort() },
        mainline_graph: {
          nodes: members.map((n) => ({ work_key: n.key, title: n.title, status: n.status, person_responsibility: n.owner, blocker: n.blocker, next_action: n.next_action, explainable_waits: n.waits })),
          edges: [],
        },
      });
    }
    if (a === 'search') {
      return ok({ project_revision: revision, truncated: Boolean(snapshot.basis.goals.truncated), hits: snapshot.goals.map((g) => ({ external_key: g.id, title: g.title, status: g.status })) });
    }
    if (a === 'event' && b === 'history') {
      const limit = Number(option(args, '--limit') || 20);
      const through = option(args, '--through-revision');
      if (through !== undefined && Number(through) > revision) return error('InvalidInput', 'event revision range is outside the current project/cursor bounds');
      const size = opts.pageSize ? Math.min(opts.pageSize, limit) : limit;
      const all = eventsList();
      let start = 0;
      const cursorText = option(args, '--cursor');
      if (cursorText) {
        const cursor = JSON.parse(cursorText);
        start = all.findIndex((e) => e.id === cursor.event_id) + 1;
      }
      const events = all.slice(start, start + size);
      const more = start + size < all.length;
      const last = events[events.length - 1];
      return ok({ project_revision: revision, events, next_cursor: more ? { project_id: project.id, project_revision: revision, created_at: last.created_at, event_id: last.id } : null });
    }
    if (a === 'session' && b === 'list') {
      const idOf = new Map(snapshot.nodes.map((n) => [n.key, n.id]));
      return ok({
        project_revision: revision, may_have_more: Boolean(snapshot.basis.sessions.truncated),
        sessions: snapshot.sessions.map((s) => ({ id: s.id, agent_id: s.agent_id, work_item_id: s.work_key ? idOf.get(s.work_key) : null, status: s.status, started_at: s.started_at, last_checkpoint_id: s.last_checkpoint_id })),
      });
    }
    if (a === 'doctor') {
      return ok({ project_revision: revision, ok: snapshot.doctor.ok, findings: snapshot.doctor.findings }, snapshot.doctor.findings.length ? 1 : 0);
    }
    return error('InvalidInput', `unrecognized subcommand: ${args.join(' ')}`);
  }

  /** The extractor's `call` over this fake, going through the same argument builders the export uses. */
  function call(name, params) {
    const commands = require('../../../public/project-map/commands.js');
    const { AwrCallError } = require('../../../public/project-map/extract.js');
    const result = run(['--project', '/project', '--json', ...commands.argvFor(name, params)]);
    const raw = result.stdout || result.stderr;
    const value = JSON.parse(raw);
    if (value && 'code' in value && 'message' in value && !('ok' in value)) return Promise.reject(new AwrCallError(value.code, value.message, value.details || null));
    return Promise.resolve(value);
  }

  return { run, call, calls };
}

module.exports = { createFakeCli };
