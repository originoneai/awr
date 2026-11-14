'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');
const http = require('node:http');
const { createTeamBridge } = require('../server/team-bridge');
const at = 1793534400000;

function item(position = 1, status = 'proposed', expires = null) {
  const open = ['proposed', 'inspected'].includes(status), elapsed = expires !== null && expires <= at;
  const pending = open && !elapsed, expired = status === 'timed_out' || open && elapsed;
  return { handoff_id: `handoff-${position}`, handoff_position: position, work_id: 'work-one', work_title: 'Customer portal',
    kind: 'execution', status, pending, expired, group: pending ? 'pending' : 'history',
    effective_status: expired ? 'expired' : status, version: '1', from_person_id: 'author', to_person_id: 'receiver',
    from_display_name: 'Author', to_display_name: 'Receiver', created_at_unix_ms: 100, updated_at_unix_ms: 100,
    expires_at_unix_ms: expires };
}

function reply(items, { total = items.length, cursor = null, state = 'pending', work = 'work-one', recipient = null } = {}) {
  return { protocol_version: 1, scope_id: 'main', source_snapshot_id: 'current-plan', project_revision: '12',
    coordinator_epoch: 'current', workstream_id: 'development', authority_version: '1',
    data: { work_id: work, recipient_person_id: recipient, handoff_state: state, items, total, next_cursor: cursor,
      truncated: cursor !== null, count_scope: 'authorized_matches', count_limited: true, hidden_count: null,
      execution_authorized: false, as_of: { at_unix_ms: at, source_snapshot_id: 'current-plan', project_revision: '12',
        handoffs_revision: 'a'.repeat(32) } } };
}

async function fixture(t, respond) {
  const calls = [];
  const upstream = http.createServer(async (req, res) => {
    const chunks = [];
    for await (const chunk of req) chunks.push(chunk);
    const query = JSON.parse(Buffer.concat(chunks).toString());
    calls.push({ path: req.url, query, cookie: req.headers.cookie });
    const { status = 200, body } = respond(query);
    res.writeHead(status, { 'content-type': 'application/json' });
    res.end(JSON.stringify(body));
  });
  await new Promise((resolve) => upstream.listen(0, '127.0.0.1', resolve));
  t.after(() => new Promise((resolve) => { upstream.close(resolve); upstream.closeAllConnections(); }));
  const bridge = createTeamBridge({ teamUrl: `http://127.0.0.1:${upstream.address().port}`, port: 0 });
  const read = (parameters) => bridge.routes['GET /api/team/handoffs'](
    new URL(`http://localhost/api/team/handoffs?${parameters}`), null,
    { headers: { cookie: 'awr_web_session=synthetic-handoff-reader' } },
    { getHeader: () => undefined, setHeader: () => {} },
  );
  return { read, calls };
}

const scope = 'project=portal&work_id=work-one&workstream_id=development';

test('handoff metadata refuses package and raw bodies at every response level', async (t) => {
  let body;
  const { read } = await fixture(t, () => ({ body }));
  for (const [level, select] of [
    ['response', (r) => r], ['data', (r) => r.data],
    ['as-of', (r) => r.data.as_of], ['item', (r) => r.data.items[0]],
  ]) {
    for (const field of ['package_json', 'body_json']) {
      body = reply([item()]);
      select(body)[field] = { details: 'must-not-leak' };
      const result = await read(scope);
      assert.equal(result.ok, false, `${level}.${field}`);
      assert.equal(result.error.code, 'BadGateway');
      assert.equal(result.result, undefined);
      assert.equal(JSON.stringify(result).includes('must-not-leak'), false);
    }
  }
  for (const field of ['project_status', 'selection_basis']) {
    body = reply([item()]);
    body[field] = { package_json: 'must-not-leak' };
    const result = await read(scope);
    assert.equal(result.ok, false, field);
    assert.equal(result.result, undefined);
  }
});

test('handoff pages preserve authoritative positions for opaque Unicode identities', async (t) => {
  const rows = [{ ...item(1), handoff_id: '\ue000' }, { ...item(2), handoff_id: '😀' }];
  const { read } = await fixture(t, () => ({ body: reply(rows) }));
  const result = await read(scope);
  assert.equal(result.ok, true, JSON.stringify(result));
  assert.deepEqual(result.result.data.items, rows);
});

test('handoff discovery preserves member identity, bounded pages, recipients and conservative counts', async (t) => {
  const pages = [reply([item()], { total: 2, cursor: 'page-two', state: 'all', recipient: 'receiver' }),
    reply([item(2, 'accepted')], { total: 2, state: 'all', recipient: 'receiver' })];
  const { read, calls } = await fixture(t, (q) => ({ body: pages[q.cursor ? 1 : 0] }));
  for (const [index, cursor] of ['', '&cursor=page-two'].entries()) {
    const result = await read(`${scope}&recipient_person_id=receiver&handoff_state=all&limit=1&max_context_bytes=65536${cursor}`);
    assert.equal(result.ok, true, JSON.stringify(result));
    assert.deepEqual(result.result.data, pages[index].data);
    assert.deepEqual(calls[index], { path: '/v1/web/projects/portal/query', cookie: 'awr_web_session=synthetic-handoff-reader',
      query: { protocol_version: 1, op: 'handoff.list', work_id: 'work-one', workstream_id: 'development',
        recipient_person_id: 'receiver', handoff_state: 'all', limit: 1, max_context_bytes: 65536,
        ...(index ? { cursor: 'page-two' } : {}) } });
  }
});

test('handoff history distinguishes accepted, withdrawn, timed-out and elapsed proposals', async (t) => {
  const items = [item(1, 'accepted', 1), item(2, 'cancelled'), item(3, 'timed_out'), item(4, 'proposed', 1)];
  const body = reply(items, { state: 'history' });
  const { read } = await fixture(t, () => ({ body }));
  const result = await read(`${scope}&handoff_state=history`);
  assert.equal(result.ok, true, JSON.stringify(result));
  assert.deepEqual(result.result.data.items.map((r) => r.effective_status), ['accepted', 'cancelled', 'expired', 'expired']);
  assert.equal(result.result.data.items[3].status, 'proposed');
});

test('handoff metadata refuses scope drift, inconsistent expiry and package disclosures', async (t) => {
  let body;
  const { read } = await fixture(t, () => ({ body }));
  const mutations = [
    ['wrong work', (r) => { r.data.work_id = 'must-not-leak'; }],
    ['missing workstream', (r) => { delete r.workstream_id; }],
    ['wrong workstream', (r) => { r.workstream_id = 'must-not-leak'; }],
    ['missing authority', (r) => { delete r.authority_version; }],
    ['wrong row work', (r) => { r.data.items[0].work_id = 'must-not-leak'; }],
    ['wrong recipient echo', (r) => { r.data.recipient_person_id = 'must-not-leak'; }],
    ['wrong view', (r) => { r.data.handoff_state = 'history'; }],
    ['missing count limitation', (r) => { delete r.data.count_limited; }],
    ['hidden count', (r) => { r.data.hidden_count = 2; }],
    ['wrong as-of', (r) => { r.data.as_of.project_revision = 'old'; }],
    ['unknown kind', (r) => { r.data.items[0].kind = 'must-not-leak'; }],
    ['unknown status', (r) => { r.data.items[0].status = 'must-not-leak'; }],
    ['invented pending', (r) => { r.data.items[0].pending = false; }],
    ['hidden expiry', (r) => { r.data.items[0].expires_at_unix_ms = 1; }],
    ['wrong grouping', (r) => { r.data.items[0].group = 'history'; }],
    ['wrong display state', (r) => { r.data.items[0].effective_status = 'accepted'; }],
    ['package payload', (r) => { r.data.items[0].package_json = { details: 'must-not-leak' }; }],
    ['raw body', (r) => { r.data.items[0].body_json = { details: 'must-not-leak' }; }],
  ];
  for (const [name, mutate] of mutations) {
    body = reply([item()]);
    mutate(body);
    const result = await read(scope);
    assert.equal(result.ok, false, name);
    assert.equal(result.error.code, 'BadGateway', name);
    assert.equal(result.result, undefined, name);
    assert.equal(JSON.stringify(result).includes('must-not-leak'), false, name);
  }
});

test('handoff page coverage refuses gaps, duplicates and short terminal collections', async (t) => {
  let body;
  const { read } = await fixture(t, () => ({ body }));
  for (const [name, value, parameters] of [
    ['missing prefix', reply([item(2)], { total: 2 }), ''],
    ['missing tail', reply([item()], { total: 2 }), ''],
    ['gap', reply([item(), item(3)], { total: 3 }), ''],
    ['duplicate', reply([item(), { ...item(2), handoff_id: 'handoff-1' }]), ''],
    ['short nonterminal page', reply([item()], { total: 3, cursor: 'next' }), '&limit=2'],
    ['empty nonempty list', reply([], { total: 1 }), ''],
  ]) {
    body = value;
    const result = await read(scope + parameters);
    assert.equal(result.ok, false, name);
    assert.equal(result.error.code, 'BadGateway', name);
  }
  body = reply([]);
  assert.equal((await read(scope)).ok, true);
  body = reply([item(2)], { total: 2 });
  assert.equal((await read(scope + '&limit=1&cursor=next')).ok, true);
});

test('handoff reads support project discovery and preserve authorization and recovery errors', async (t) => {
  let status = 200, body = reply([item()], { work: null });
  delete body.workstream_id;
  delete body.authority_version;
  const { read, calls } = await fixture(t, () => ({ status, body }));
  assert.equal((await read('project=portal')).ok, true);
  for (const parameters of [`${scope}&session_id=other`, `${scope}&actor_id=forged`, `${scope}&handoff_state=accepted`,
    `${scope}&recipient_person_id=`, `${scope}&limit=1.5`, `${scope}&limit=101`, `${scope}&project=other`,
    `${scope}&max_context_bytes=262145`, `${scope}&op=handoff.accept`]) {
    assert.equal((await read(parameters)).error.code, 'InvalidInput', parameters);
  }
  assert.equal(calls.length, 1);
  for (const code of ['Forbidden', 'CursorExpired', 'ResponseTooLarge']) {
    status = 403; body = { code, message: 'Read refused' };
    const result = await read(scope);
    assert.equal(result.ok, false);
    assert.equal(result.error.code, code);
    assert.equal(result.result, undefined);
  }
});
