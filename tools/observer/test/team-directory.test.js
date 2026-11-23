'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const http = require('node:http');
const path = require('node:path');
const { createTeamBridge } = require('../server/team-bridge');
const { projectDirectory } = require('../server/directory');

// Byte-recorded replies from directory_mcp.rs; no hand-written upstream shape.
const readSample = (name) => JSON.parse(fs.readFileSync(path.join(__dirname, 'fixtures', name), 'utf8'));
const hierarchy = readSample('directory-web-read-hierarchy.json');
const flat = readSample('directory-web-read-flat.json');
const unconfigured = readSample('directory-web-read-unconfigured.json');
const identities = readSample('directory-web-read-identities.json');
const clone = (value) => structuredClone(value);

test('the real directory projection joins people by stable ID, including same names and multiple groups', () => {
  const engineering = { key: 'engineering', name: 'Engineering', parentKey: null };
  const research = { key: 'research', name: 'Research', parentKey: null };
  const result = projectDirectory(identities.data);
  assert.deepEqual(result, {
    configured: true,
    organization: 'Example company',
    departments: [engineering, research],
    allDepartments: [engineering, research],
    updated_at_ms: identities.data.updated_at_ms,
    members: [
      { personId: 'directory-person', name: 'Project coordinator', available: true, departments: [engineering, research] },
      { personId: 'directory-person-two', name: 'Project coordinator', available: true, departments: [research] },
    ],
  });
  assert.equal(result.members[0].name, result.members[1].name);
  assert.notEqual(result.members[0].personId, result.members[1].personId);
  assert.deepEqual(result.members[0].departments.map((department) => department.key), ['engineering', 'research']);
  assert.equal(JSON.stringify(result).includes('provenance'), false);
  assert.equal(JSON.stringify(result).includes('member_ref'), false);
});

test('real hierarchical, flat and unconfigured replies retain their parent relationships', () => {
  const engineeringNode = { key: 'engineering', name: 'Engineering', parentKey: null };
  const platform = { key: 'platform', name: 'Platform', parentKey: 'engineering' };
  const runtime = { key: 'runtime', name: 'Runtime', parentKey: 'platform' };
  const scheduling = { key: 'scheduling', name: 'Scheduling', parentKey: 'runtime' };
  const hierarchicalResult = projectDirectory(hierarchy.data);
  assert.deepEqual(hierarchicalResult.departments, [scheduling]);
  assert.deepEqual(hierarchicalResult.allDepartments, [engineeringNode, platform, runtime, scheduling]);
  assert.deepEqual(hierarchicalResult.members, [{
    personId: 'directory-person', name: 'Project coordinator', available: true, departments: [scheduling],
  }]);
  const engineering = { key: 'engineering', name: 'Engineering', parentKey: null };
  assert.deepEqual(projectDirectory(flat.data).departments, [engineering]);
  assert.deepEqual(projectDirectory(flat.data).allDepartments, [engineering]);
  assert.deepEqual(projectDirectory(unconfigured.data), {
    configured: false, organization: null, departments: [], allDepartments: [], updated_at_ms: null, members: [],
  });
});

test('incomplete, mismatched and contradictory relationship sets fail closed', () => {
  const mutations = [
    (value) => { value.members.pop(); },
    (value) => { value.members[1] = value.members[0]; },
    (value) => { value.members[0].member_ref.project_id = 'another-project'; },
    (value) => { value.members[0].member_ref.person_id = 'unregistered-member'; },
    (value) => { value.members[0].department_ids = ['engineering']; },
    (value) => { value.plan.project_department_ids = ['missing']; },
    (value) => { value.plan.departments.push(value.plan.departments[0]); },
    (value) => { value.members[0].available = 'true'; },
    (value) => { value.members[0].available = false; },
    (value) => { value.version = 1; },
    (value) => { value.configured = false; },
    (value) => { value.plan.organization = null; },
  ];
  for (const mutate of mutations) {
    const value = clone(identities.data);
    mutate(value);
    assert.equal(projectDirectory(value), null, mutate.toString());
  }
});

test('missing, self, cyclic and over-deep department parents reject the whole real reply', () => {
  const mutations = {
    missing: (value) => { value.plan.departments.find((item) => item.id === 'runtime').parent_id = 'missing'; },
    self: (value) => { value.plan.departments.find((item) => item.id === 'engineering').parent_id = 'engineering'; },
    cycle: (value) => { value.plan.departments.find((item) => item.id === 'engineering').parent_id = 'scheduling'; },
    pairCycle: (value) => {
      value.plan.departments.find((item) => item.id === 'platform').parent_id = 'runtime';
      value.plan.departments.find((item) => item.id === 'runtime').parent_id = 'platform';
    },
    overDeep: (value) => { value.plan.departments.push({ id: 'too-deep', name: 'Too deep', parent_id: 'scheduling' }); },
    wrongType: (value) => { value.plan.departments.find((item) => item.id === 'runtime').parent_id = 7; },
  };
  for (const [fault, mutate] of Object.entries(mutations)) {
    const value = clone(hierarchy.data);
    mutate(value);
    assert.equal(projectDirectory(value), null, fault);
  }
});

test('same-name departments keep separate references in every relationship', () => {
  const source = clone(identities.data);
  source.plan.departments[1].name = 'Engineering';
  const result = projectDirectory(source);
  assert.deepEqual(result.departments, [
    { key: 'engineering', name: 'Engineering', parentKey: null },
    { key: 'research', name: 'Engineering', parentKey: null },
  ]);
  assert.notEqual(result.members[0].departments[0].key, result.members[0].departments[1].key);
});

test('the optional recorded duty is projected only when present and malformed duties fail closed', () => {
  const source = clone(hierarchy.data);
  assert.equal(Object.hasOwn(projectDirectory(source).members[0], 'duty'), false);
  source.members[0].duty = 'Reviewer';
  assert.equal(projectDirectory(source).members[0].duty, 'Reviewer');
  source.members[0].duty = 7;
  assert.equal(projectDirectory(source), null);
});

test('the bridge forwards only the signed-in read, returns IDs and never caches across authorization changes', async (t) => {
  let status = 200;
  let payload = clone(hierarchy);
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
  const call = (body, cookie = 'awr_web_session=synthetic-browser') => bridge.routes['POST /api/team/directory'](null, body,
    { headers: { cookie } }, { getHeader: () => undefined, setHeader: () => {} });
  assert.equal((await call({})).error.code, 'Unsupported', 'a live service has no organization directory read');
  assert.equal(calls.length, 0, 'organization fallback never guesses a service endpoint');
  const result = await call({ project: 'reader-project' });
  assert.deepEqual(calls, [{ path: '/v1/web/projects/reader-project/directory/read', cookie: 'awr_web_session=synthetic-browser', body: { protocol_version: 1 } }]);
  assert.deepEqual(result, { ok: true, data: projectDirectory(hierarchy.data) });
  assert.equal(result.data.members[0].personId, 'directory-person');
  for (const field of ['provenance', 'authorization_binding', 'source_snapshot_id', 'member_ref', 'person_id']) {
    assert.equal(JSON.stringify(result).includes(field), false, field);
  }
  for (const body of [{ project: 'reader-project', actor_id: 'other' }, { project: 'reader-project', operation: 'apply' }, { project: 'reader-project', bearer: 'private' }]) {
    assert.equal((await call(body)).error.code, 'InvalidInput');
  }
  assert.equal(calls.length, 1);
  payload = { ok: true, data: { ...clone(hierarchy.data), members: [] } };
  assert.equal((await call({ project: 'reader-project' })).error.code, 'BadGateway');
  status = 403;
  payload = { code: 'Forbidden' };
  const revoked = await call({ project: 'reader-project' });
  assert.equal(revoked.error.code, 'Forbidden');
  assert.equal(JSON.stringify(revoked).includes('directory-person'), false);
  status = 404;
  assert.equal((await call({ project: 'reader-project' })).error.code, 'Unsupported');
  status = 200;
  payload = clone(hierarchy);
  const otherSession = await call({ project: 'reader-project' }, 'awr_web_session=another-synthetic-browser');
  assert.equal(otherSession.ok, true);
  assert.equal(calls.length, 5, 'every session and authorization change reaches the service; no directory result is cached');
});
