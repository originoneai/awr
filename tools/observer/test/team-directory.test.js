'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const http = require('node:http');
const { createTeamBridge } = require('../server/team-bridge');
const { projectDirectory } = require('../server/directory');

function directory() {
  return { project_id: 'project-one', version: '1', configured: true, updated_at_ms: 1794600000000,
    plan: { organization: { id: 'company', name: 'Example company' },
      departments: [{ id: 'eng', name: 'Engineering' }, { id: 'research', name: 'Research' }],
      project_department_ids: ['eng'], members: [{ person_id: 'member-one', department_ids: ['eng'] },
        { person_id: 'member-two', department_ids: ['research'] }] },
    members: [{ member_ref: { project_id: 'project-one', person_id: 'member-one' }, display_name: 'Taylor', available: true, department_ids: ['eng'] },
      { member_ref: { project_id: 'project-one', person_id: 'member-two' }, display_name: 'Taylor', available: true, department_ids: ['research'] }],
    provenance: { actor_id: 'private-actor', client_id: 'private-client' }, raw_payload: 'private-diagnostics' };
}

test('the directory projection preserves same-name members and omits service metadata', () => {
  assert.deepEqual(projectDirectory(directory()), {
    configured: true, organization: 'Example company', departments: [{ key: 'eng', name: 'Engineering' }], updated_at_ms: 1794600000000,
    members: [{ name: 'Taylor', available: true, departments: [{ key: 'eng', name: 'Engineering' }] }, { name: 'Taylor', available: true, departments: [{ key: 'research', name: 'Research' }] }],
  });
  const source = directory();
  source.members[1].available = false;
  source.members[1].display_name = null;
  assert.equal(projectDirectory(source).members[1].available, false);
});

test('incomplete, mismatched and contradictory relationship sets fail closed', () => {
  const mutations = [
    (value) => { value.members.pop(); },
    (value) => { value.members[1] = value.members[0]; },
    (value) => { value.members[0].member_ref.project_id = 'another-project'; },
    (value) => { value.members[0].member_ref.person_id = 'unregistered-member'; },
    (value) => { value.members[0].department_ids = ['research']; },
    (value) => { value.plan.project_department_ids = ['missing']; },
    (value) => { value.plan.departments.push(value.plan.departments[0]); },
    (value) => { value.members[0].available = 'true'; },
    (value) => { value.members[0].available = false; },
    (value) => { value.version = 1; },
    (value) => { value.configured = false; },
    (value) => { value.plan.organization = null; },
  ];
  for (const mutate of mutations) {
    const value = directory();
    mutate(value);
    assert.equal(projectDirectory(value), null, mutate.toString());
  }
  assert.deepEqual(projectDirectory({ project_id: 'project-one', version: '0', configured: false, updated_at_ms: null,
    plan: { organization: null, departments: [], project_department_ids: [], members: [] }, members: [] }), {
    configured: false, organization: null, departments: [], updated_at_ms: null, members: [],
  });
});

test('same-name departments keep separate references in every relationship', () => {
  const source = directory();
  source.plan.departments[1].name = 'Engineering';
  source.plan.project_department_ids.push('research');
  const result = projectDirectory(source);
  assert.deepEqual(result.departments, [
    { key: 'eng', name: 'Engineering' }, { key: 'research', name: 'Engineering' },
  ]);
  assert.notEqual(result.members[0].departments[0].key, result.members[1].departments[0].key);
});

test('the bridge forwards only the signed-in read and returns product fields', async (t) => {
  let status = 200;
  let payload = { ok: true, data: directory() };
  const calls = [];
  const upstream = http.createServer(async (req, res) => {
    const chunks = [];
    for await (const chunk of req) chunks.push(chunk);
    calls.push({ path: req.url, cookie: req.headers.cookie, body: JSON.parse(Buffer.concat(chunks).toString()) });
    res.writeHead(status, { 'content-type': 'application/json' });
    res.end(JSON.stringify(payload));
  });
  await new Promise((resolve) => upstream.listen(0, '127.0.0.1', resolve));
  t.after(() => new Promise((resolve) => { upstream.close(resolve); upstream.closeAllConnections(); }));
  const bridge = createTeamBridge({ teamUrl: `http://127.0.0.1:${upstream.address().port}`, port: 0 });
  const call = (body) => bridge.routes['POST /api/team/directory'](null, body,
    { headers: { cookie: 'awr_web_session=synthetic-browser' } }, { getHeader: () => undefined, setHeader: () => {} });
  const result = await call({ project: 'customer' });
  assert.deepEqual(calls, [{ path: '/v1/web/projects/customer/directory/read', cookie: 'awr_web_session=synthetic-browser', body: { protocol_version: 1 } }]);
  assert.deepEqual(result, { ok: true, data: projectDirectory(directory()) });
  assert.equal(JSON.stringify(result).includes('private-'), false);
  assert.equal(JSON.stringify(result).includes('member-one'), false);
  for (const body of [{ project: 'customer', actor_id: 'other' }, { project: 'customer', operation: 'apply' }, { project: 'customer', bearer: 'private' }]) {
    assert.equal((await call(body)).error.code, 'InvalidInput');
  }
  assert.equal(calls.length, 1);
  payload = { ok: true, data: { ...directory(), members: [] } };
  assert.equal((await call({ project: 'customer' })).error.code, 'BadGateway');
  status = 403;
  payload = { code: 'Forbidden' };
  assert.equal((await call({ project: 'customer' })).error.code, 'Forbidden');
  status = 404;
  assert.equal((await call({ project: 'customer' })).error.code, 'Unsupported');
});
