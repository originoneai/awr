'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');
const http = require('node:http');
const { createTeamBridge } = require('../server/team-bridge');

const field = (value, reason = null) => ({ value, as_of_unix_ms: value == null ? null : 1793534400000, reason });

test('live overview maps actual list summaries and preserves missing reasons without per-work reads', async (t) => {
  const calls = [];
  const reported = {
    version: 1,
    owner: field({ person_id: 'member-maya', display_name: 'Maya' }),
    phase: field('testing'), runtime: field('in_progress'),
    outcome: field({ summary: 'Sign-in works; checking expiry', completed: ['Sign-in screen'], integration: { state: 'prepared', current_binding: true } }),
    blocker: field({ kind: 'reported', text: 'Waiting for the review environment' }),
    next_action: field('Check interrupted renewal'),
    report: field({ origin: { actor_name: 'Development agent' }, contract_matches_current: true, stale: false }),
  };
  const absent = Object.fromEntries(['owner', 'phase', 'runtime', 'outcome', 'blocker', 'next_action', 'report']
    .map((key) => [key, field(null, key === 'owner' ? 'unassigned' : 'no_session')]));
  absent.version = 1;
  const server = http.createServer(async (req, res) => {
    res.setHeader('content-type', 'application/json');
    if (req.url === '/v1/web/session') {
      res.end(JSON.stringify({ session_id: 'summary-reader', expires_at_ms: Date.now() + 60000 }));
      return;
    }
    const chunks = [];
    for await (const chunk of req) chunks.push(chunk);
    const query = JSON.parse(Buffer.concat(chunks).toString());
    calls.push(query.op);
    const data = query.op === 'workstreams.list'
      ? { items: [{ id: 'development', title: 'Development' }], total: 1, next_cursor: null }
      : query.op === 'capabilities' ? { identity: { actor_id: 'observer' } }
        : { items: [
          { work_id: 'sign-in', title: 'Finish sign-in', contract_hash: 'current', summary: reported },
          { work_id: 'guide', title: 'Write the help guide', summary: absent, status: 'completed', owner_person: 'Untrusted fallback' },
        ], total: 2, next_cursor: null };
    res.end(JSON.stringify({ protocol_version: 1, project_revision: '12', source_snapshot_id: 'source-12', coordinator_epoch: 'current', data }));
  });
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
  t.after(() => new Promise((resolve) => { server.close(resolve); server.closeAllConnections(); }));
  const bridge = createTeamBridge({ teamUrl: `http://127.0.0.1:${server.address().port}`, port: 0 });
  const result = await bridge.routes['GET /api/team/overview'](
    new URL('http://localhost/api/team/overview?project=portal'), null,
    { headers: { cookie: 'awr_web_session=synthetic-summary-session' } },
    { getHeader: () => undefined, setHeader: () => {} },
  );
  assert.equal(result.ok, true);
  assert.deepEqual(calls, ['workstreams.list', 'work.list', 'capabilities']);
  assert.deepEqual(result.works[0], {
    key: 'sign-in', title: 'Finish sign-in', owner_person: 'Maya', agent: 'Development agent',
    outcome: 'Sign-in works; checking expiry', status: 'in_progress', blocker: 'Waiting for the review environment',
    next_step: 'Check interrupted renewal', observation_summary: reported,
    depends_on: [], hidden_deps: [], capabilities: {}, contract_hash: 'current', detail_loaded: false,
    workstream_id: 'development',
  });
  assert.equal(result.works[1].owner_person, null);
  assert.equal(result.works[1].status, 'unknown');
  assert.equal(result.works[1].next_step, null);
  assert.deepEqual(result.works[1].observation_summary, absent);
});
