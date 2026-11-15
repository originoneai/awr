'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');
const http = require('node:http');
const { createTeamBridge } = require('../server/team-bridge');

async function planningBridge(t, response) {
  const calls = [];
  const upstream = http.createServer(async (req, res) => {
    const chunks = [];
    for await (const chunk of req) chunks.push(chunk);
    const query = JSON.parse(Buffer.concat(chunks).toString());
    calls.push(query);
    res.setHeader('content-type', 'application/json');
    if (response.code) res.statusCode = 403;
    res.end(JSON.stringify(response.code ? response : {
      protocol_version: 1, scope_id: 'main', source_snapshot_id: 'current-plan',
      project_revision: '12', coordinator_epoch: 'current',
      ...(query.workstream_id ? { workstream_id: query.workstream_id, authority_version: '1' } : {}),
      data: response,
    }));
  });
  await new Promise((resolve) => upstream.listen(0, '127.0.0.1', resolve));
  t.after(() => new Promise((resolve) => { upstream.close(resolve); upstream.closeAllConnections(); }));
  const bridge = createTeamBridge({ teamUrl: `http://127.0.0.1:${upstream.address().port}`, port: 0 });
  const read = (parameters) => bridge.routes['GET /api/team/plans'](
    new URL(`http://localhost/api/team/plans?${parameters}`), null,
    { headers: { cookie: 'awr_web_session=synthetic-planning-reader' } },
    { getHeader: () => undefined, setHeader: () => {} },
  );
  return { read, calls };
}

test('planning route preserves recorded decisions, current differences and unknown transitions', async (t) => {
  const data = { items: [{
    candidate_id: 'proposal-one', state: 'published', author: { display_name: 'Maya' },
    current_diff: { counts: { added: 0, removed: 0, changed: 1, unchanged: 0 }, items: [{
      title: 'Clarify sign-in requirements', kind: 'changed', fields: [{ field: 'title',
        before: { value: 'Sign-in requirements', reason: null }, after: { value: 'Clarify sign-in requirements', reason: null } }],
    }] },
    approval: { value: { display_name: 'Kai', at_unix_ms: 1793534400000 }, reason: null },
    publication: { value: { display_name: 'Maya', source_writeback_pending: true }, reason: null },
    activation: { value: null, reason: 'not_applied' },
    rejection: { value: null, reason: 'transition_not_supported' },
  }], total: 1, next_cursor: null, truncated: false, execution_authorized: false };
  const { read, calls } = await planningBridge(t, data);
  for (const selector of ['', '&workstream_id=development']) {
    const result = await read(`project=portal&limit=5&max_context_bytes=65536${selector}`);
    assert.equal(result.ok, true);
    assert.deepEqual(result.result.data, data);
    assert.equal(result.freshness.authoritative.status, 'measured');
  }
  assert.deepEqual(calls, [
    { protocol_version: 1, op: 'planning.candidates', limit: 5, max_context_bytes: 65536 },
    { protocol_version: 1, op: 'planning.candidates', workstream_id: 'development', limit: 5, max_context_bytes: 65536 },
  ]);
});

test('planning route refuses forged controls and preserves upstream revocation without fixture fallback', async (t) => {
  const { read, calls } = await planningBridge(t, { code: 'Forbidden', message: 'Access denied' });
  for (const input of ['project=portal&actor_id=forged', 'project=portal&limit=1.5',
    'project=portal&project=other', 'project=portal&limit=101', 'project=portal&work_id=hidden',
    'project=portal&operation=planning.reject']) {
    assert.equal((await read(input)).error.code, 'InvalidInput');
  }
  assert.equal(calls.length, 0);
  const refused = await read('project=portal');
  assert.equal(refused.ok, false);
  assert.equal(refused.error.code, 'Forbidden');
  assert.equal(refused.result, undefined);
  assert.equal(calls.length, 1);
});

test('planning reads preserve an explicit returned-plan decision without exposing a write route', async (t) => {
  const data={items:[{candidate_id:'returned-proposal',state:'rejected',
    rejection:{value:{actor_id:'reviewer',display_name:'Kai',at_unix_ms:1793534400000,
      reason:'Clarify the acceptance criteria'},reason:null}}],total:1,next_cursor:null,
    truncated:false,execution_authorized:false};
  const {read,calls}=await planningBridge(t,data);
  const result=await read('project=portal');
  assert.deepEqual(result.result.data,data);
  assert.deepEqual(calls,[{protocol_version:1,op:'planning.candidates'}]);
});
