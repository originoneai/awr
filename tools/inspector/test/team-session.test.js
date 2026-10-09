'use strict';

const { test, beforeEach } = require('node:test');
const assert = require('node:assert/strict');
const { install } = require('./fixtures/dom-stub');
const { createTeamWeb } = require('../public/team-web');
const i18n = require('../public/i18n');
let ui;
const response = (body) => ({ ok: true, json: async () => body });
const project = { key: 'example', title: 'Example project' };
const work = { key: 'WORK-1', title: 'Example work', status: 'planned' };
const projects = { ok: true, projects: [project], session: { session_id: 'test-session' } };
const overview = { ok: true, works: [work], members: [] };
const node = (id) => document.getElementById(id);
const button = (id, label) => node(id).find((el) => el.tagName === 'BUTTON' && el.textContent === label);

const inboxIdentity = { actor_id: 'member', client_id: 'member-agent', role: 'reviewer' };
const inboxItem = (key = 'semantic-one') => {
  const query = { protocol_version: 1, op: 'review.inspect', work_id: work.key, workstream_id: 'stream', review_round_id: 'current-round' };
  return { item_key: key, work_id: work.key, workstream_id: 'stream', contract_hash: 'requirements',
    title: 'Review the current delivery', title_truncated: false, next_query: query,
    guidance: { code: 'review', when: 'The current review is open', because: ['Version-bound evidence is recorded'],
      action: { op: query.op, query }, recheck_on: 'Requirements, review or permissions change' } };
};
const inboxPage = (items = [], next = null) => ({ protocol_version: 1, scope_id: 'main',
  project_revision: '20', source_snapshot_id: 'source', coordinator_epoch: 'epoch',
  data: { identity: inboxIdentity, items, next_cursor: next, state_basis: 'current_persistent_facts',
    deduplicate_by: 'item_key', refresh_from_first_page_on_change: true, execution_authorized: false } });
const inboxOverview = { ...overview, works: [{ ...work, workstream_id: 'stream', detail_loaded: true }],
  interaction_mode: 'mcp', identity: inboxIdentity };

async function readyInbox(handler) {
  ui.state.layout = 'cards';
  const calls = mock((url, options) => url.includes('/projects?') ? projects
    : url.includes('/overview?') ? inboxOverview
      : url.endsWith('/logout') ? { ok: true } : handler(url, options));
  await ui.refresh();
  ui.state.layout = 'inbox';
  return calls;
}

function mock(handler) {
  const calls = [];
  global.fetch = async (url, options) => {
    calls.push({ url, options });
    return response(await handler(url, options));
  };
  return calls;
}

beforeEach(() => {
  install();
  i18n.setLocale('en');
  ui = createTeamWeb({ i18n, $: node });
});

test('successful login loads projects and work immediately and clears the credential field', async () => {
  const calls = mock((url) => url.endsWith('/login') ? { ok: true, session_id: 'test-session' }
    : url.includes('/projects?') ? projects : overview);
  ui.render();
  const input = node('teamAuth').find((el) => el.tagName === 'INPUT');
  input.value = 'synthetic-login-value';
  await button('teamAuth', 'Sign in').click();
  assert.equal(input.value, '');
  assert.deepEqual(ui.state.works, [work]);
  assert.match(node('teamProjects').textContent, /Example project/);
  assert.equal(node('view-team').dataset.teamState, 'signed-in');
  assert.equal(node('teamWorkspaceGrid').hidden, false);
  assert.equal(node('teamProjects').hidden, false);
  assert.equal(node('teamDetail').hidden, false);
  assert.equal(node('teamRaw').hidden, false);
  assert.equal(node('teamWorkspaceIntro').hidden, true);
  assert.equal(calls.length, 3);
  assert.ok(!JSON.stringify(ui.state).includes('synthetic-login-value'));
});

test('coordination reads through empty pages and deduplicates only server semantic keys', async () => {
  const first = inboxItem(), second = { ...inboxItem('semantic-two'), title: 'Coordinate another delivery' };
  const calls = await readyInbox(url => ({ ok: true, result: url.includes('cursor=last') ? inboxPage([first, second])
    : url.includes('cursor=empty-next') ? inboxPage([first], 'last') : inboxPage([], 'empty-next') }));
  await ui._loadInbox();
  assert.match(node('teamOverview').textContent, /This page has no current conditions. More pages remain/);
  assert.ok(button('teamOverview', 'Read next page'));
  await button('teamOverview', 'Read next page').click();
  await button('teamOverview', 'Read next page').click();
  assert.deepEqual(ui.state.inbox.items.map(v => v.item_key), ['semantic-one', 'semantic-two']);
  assert.equal(ui.state.inbox.next, null);
  assert.equal(button('teamOverview', 'Read next page'), null);
  assert.deepEqual(calls.filter(c => c.url.includes('/inbox?')).map(c => c.url), [
    '/api/team/inbox?project=example', '/api/team/inbox?project=example&cursor=empty-next', '/api/team/inbox?project=example&cursor=last']);
  assert.ok(!calls.some(c => c.url.includes('/action') || c.url.includes('/command')));
});

test('coordination refresh removes resolved conditions without an acknowledgement or write', async () => {
  let resolved = false;
  const calls = await readyInbox(() => ({ ok: true, result: inboxPage(resolved ? [] : [inboxItem()]) }));
  await ui._loadInbox();
  assert.equal(ui.state.inbox.items.length, 1);
  resolved = true;
  await button('teamOverview', 'Refresh coordination').click();
  assert.equal(ui.state.inbox.items.length, 0);
  assert.match(node('teamOverview').textContent, /No current conditions in the pages read for your access/);
  assert.ok(!node('teamOverview').textContent.includes('Review the current delivery'));
  assert.ok(calls.every(c => !c.options?.method || c.options.method === 'GET'));
});

test('coordination partial failures retain marked observations but disable detail and cursor reuse', async () => {
  let fail = false;
  await readyInbox(url => url.includes('cursor=next') ? { ok: false, error: { code: 'ReadTimeout', message: 'Page timed out' } }
    : { ok: true, result: inboxPage(fail ? [] : [inboxItem()], fail ? null : 'next') });
  await ui._loadInbox(); await ui._loadInbox(true);
  assert.equal(ui.state.inbox.items.length, 1);
  assert.match(node('teamOverview').textContent, /Earlier pages are incomplete and details are disabled/);
  assert.equal(button('teamOverview', 'Read current details').disabled, true);
  assert.equal(ui.state.inbox.next, null);
  fail = true; await ui._loadInbox();
  assert.equal(ui.state.inbox.error, null); assert.deepEqual(ui.state.inbox.items, []);
});

for (const version of ['source_snapshot_id', 'project_revision', 'coordinator_epoch']) {
  test(`coordination refuses mixed ${version} pages and refreshes from the first page`, async () => {
    await readyInbox(url => ({ ok: true, result: url.includes('cursor=next')
      ? { ...inboxPage([inboxItem('changed')]), [version]: version === 'project_revision' ? '21' : 'changed' } : inboxPage([inboxItem()], 'next') }));
    await ui._loadInbox(); await ui._loadInbox(true);
    assert.equal(ui.state.inbox.error.code, 'SourceChanged');
    assert.deepEqual(ui.state.inbox.items, []);
    assert.equal(ui.state.inbox.detail, null); assert.equal(ui.state.inbox.next, null);
    assert.match(node('teamOverview').textContent, /Refresh from the first page/);
  });
}

test('coordination catches a cursor cycle even when intermediate pages are empty', async () => {
  await readyInbox(url => ({ ok: true, result: url.includes('cursor=one') ? inboxPage([], 'two') : inboxPage([], 'one') }));
  await ui._loadInbox(); await ui._loadInbox(true); await ui._loadInbox(true);
  assert.equal(ui.state.inbox.error.code, 'InvalidCursor'); assert.equal(ui.state.inbox.next, null);
});

for (const code of ['Unauthenticated', 'SessionExpired', 'Forbidden']) {
  test(`coordination ${code} clears protected data including earlier pages`, async () => {
    let denied = false;
    await readyInbox(() => denied ? { ok: false, error: { code, message: 'Current access refused' } }
      : { ok: true, result: inboxPage([inboxItem()], 'next') });
    await ui._loadInbox(); denied = true; await ui._loadInbox(true);
    assert.deepEqual(ui.state.inbox.items, []); assert.deepEqual(ui.state.works, []);
    assert.equal(ui.state.projectKey, null); assert.equal(ui.state.inbox.detail, null);
    assert.match(node('teamAuth').textContent, /Current access refused/);
  });
}

test('coordination rejects a different authenticated identity even on the same source', async () => {
  await readyInbox(() => ({ ok: true, result: { ...inboxPage([inboxItem()]),
    data: { ...inboxPage().data, identity: { ...inboxIdentity, client_id: 'other-agent' }, items: [inboxItem()] } } }));
  await ui._loadInbox();
  assert.equal(ui.state.error.code, 'Forbidden'); assert.equal(ui.state.projectKey, null);
  assert.deepEqual(ui.state.inbox.items, []);
});

test('a late coordination response cannot restore conditions after logout', async () => {
  let release; const gate = new Promise(resolve => { release = resolve; });
  await readyInbox(() => gate);
  const pending = ui._loadInbox();
  await button('teamAuth', 'Log out').click();
  release({ ok: true, result: inboxPage([inboxItem()]) }); await pending;
  assert.equal(ui.state.session, null); assert.deepEqual(ui.state.inbox.items, []);
});

test('a late coordination response cannot restore a previous project after refresh', async () => {
  let release; const gate = new Promise(resolve => { release = resolve; });
  await readyInbox(() => gate);
  const pending = ui._loadInbox();
  mock(url => url.includes('/projects?') ? { ...projects, projects: [{ key: 'another' }] }
    : url.includes('/overview?') ? { ...inboxOverview, works: [] } : { ok: true, result: inboxPage() });
  await ui.refresh(); release({ ok: true, result: inboxPage([inboxItem()]) }); await pending;
  assert.equal(ui.state.projectKey, 'another'); assert.deepEqual(ui.state.inbox.items, []);
});

test('coordination detail uses the exact review selector and keeps delivery dimensions separate', async () => {
  const item = inboxItem();
  const detail = { ...inboxPage(), workstream_id: 'stream', data: { work_id: work.key,
    review: { state: 'approved' }, integration: { state: 'unknown' }, publication: { phase: 'conflict' } } };
  const calls = await readyInbox(url => url.endsWith('/inbox-detail')
    ? { ok: true, query: item.next_query, result: detail } : { ok: true, result: inboxPage([item]) });
  await ui._loadInbox(); await button('teamOverview', 'Read current details').click();
  const sent = calls.find(c => c.url.endsWith('/inbox-detail'));
  assert.deepEqual(JSON.parse(sent.options.body), { project: project.key, query: item.next_query });
  assert.match(node('teamOverview').textContent, /Approved for the recorded review basis/);
  assert.match(node('teamOverview').textContent, /Outcome unknown; inspect the original request/);
  assert.match(node('teamOverview').textContent, /Source conflict requires resolution/);
  assert.ok(!calls.some(c => c.url.includes('/command') || c.url.includes('/action')));
  assert.equal(node('teamDetail').hidden, true);
});

for (const mutation of ['wrong_query', 'wrong_source', 'wrong_revision', 'wrong_epoch', 'wrong_stream']) {
  test(`coordination refuses the ${mutation} detail without showing its facts`, async () => {
    const item = inboxItem(), result = { ...inboxPage(), workstream_id: 'stream', data: { private_marker: 'Do not display' } };
    const query = { ...item.next_query };
    if (mutation === 'wrong_query') query.review_round_id = 'other';
    if (mutation === 'wrong_source') result.source_snapshot_id = 'other';
    if (mutation === 'wrong_revision') result.project_revision = '19';
    if (mutation === 'wrong_epoch') result.coordinator_epoch = 'other';
    if (mutation === 'wrong_stream') result.workstream_id = 'other';
    await readyInbox(url => url.endsWith('/inbox-detail') ? { ok: true, query, result } : { ok: true, result: inboxPage([item]) });
    await ui._loadInbox(); await ui._readInboxItem(item);
    assert.equal(ui.state.inbox.error.code, 'SourceChanged'); assert.equal(ui.state.inbox.detail, null);
    assert.ok(!node('teamOverview').textContent.includes('Do not display'));
  });
}

test('coordination detail failure stays a read failure and absent stages stay unreported', async () => {
  const item = inboxItem(); let fail = true;
  await readyInbox(url => url.endsWith('/inbox-detail') ? fail
    ? { ok: false, error: { code: 'ReadTimeout', message: 'Current detail timed out' } }
    : { ok: true, query: item.next_query, result: { ...inboxPage(), workstream_id: 'stream', data: {} } }
    : { ok: true, result: inboxPage([item]) });
  await ui._loadInbox(); await ui._readInboxItem(item);
  assert.match(node('teamOverview').textContent, /ReadTimeout.*Current detail timed out/);
  assert.equal(ui.state.inbox.detail.project_revision, undefined);
  fail = false; await ui._readInboxItem(item);
  assert.equal(ui.state.inbox.detail.error, undefined);
  assert.match(node('teamOverview').textContent, /This read does not include this fact/);
});

test('background refresh updates coordination from the first page and clears resolved items', async () => {
  let resolved = false;
  const calls = await readyInbox(() => ({ ok: true, result: inboxPage(resolved ? [] : [inboxItem()]) }));
  await ui._loadInbox(); resolved = true; await ui._refreshProgress();
  assert.deepEqual(ui.state.inbox.items, []);
  assert.equal(calls.filter(c => c.url.includes('/inbox?')).length, 2);
  assert.ok(calls.filter(c => c.url.includes('/inbox?')).every(c => !c.url.includes('cursor=')));
});

test('late coordination details cannot restore protected facts after workspace identity changes', async () => {
  const item = inboxItem(); let release;
  const gate = new Promise(resolve => { release = resolve; });
  await readyInbox(url => url.endsWith('/inbox-detail') ? gate : { ok: true, result: inboxPage([item]) });
  await ui._loadInbox(); const pending = ui._readInboxItem(item);
  mock(url => url.includes('/projects?') ? projects
    : url.includes('/overview?') ? { ...inboxOverview, identity: { ...inboxIdentity, client_id: 'new-agent' } }
      : { ok: true, result: { ...inboxPage(), data: { ...inboxPage().data, identity: { ...inboxIdentity, client_id: 'new-agent' } } } });
  await ui.refresh();
  release({ ok: true, query: item.next_query, result: { ...inboxPage(), workstream_id: 'stream', data: { secret_marker: 'Old protected detail' } } });
  await pending;
  assert.equal(ui.state.raw.identity.client_id, 'new-agent'); assert.equal(ui.state.inbox.detail, null);
  assert.ok(!node('teamOverview').textContent.includes('Old protected detail'));
});

test('coordination shows actual integration and source publication reads without inventing current confirmation', async () => {
  const query = { protocol_version: 1, op: 'delivery.source.status', work_id: work.key, workstream_id: 'stream' };
  const item = { ...inboxItem(), next_query: query, guidance: { ...inboxItem().guidance,
    code: 'source_publication', action: { op: query.op, query } } };
  const result = { ...inboxPage(), workstream_id: 'stream', data: { pending_publication_id: 'original-request',
    history: [{ phase: 'conflict', source_current: false }, { phase: 'confirmed', source_current: false }], history_truncated: true } };
  await readyInbox(url => url.endsWith('/inbox-detail') ? { ok: true, query, result } : { ok: true, result: inboxPage([item]) });
  await ui._loadInbox(); await ui._readInboxItem(item);
  assert.match(node('teamOverview').textContent, /source publication request remains pending/);
  assert.match(node('teamOverview').textContent, /Source conflict requires resolution/);
  assert.match(node('teamOverview').textContent, /Historical record; current confirmation not established/);
  assert.match(node('teamOverview').textContent, /Some delivery facts were omitted/);
  assert.ok(!node('teamOverview').textContent.includes('Current source confirmed'));

  const integrationQuery = { protocol_version: 1, op: 'delivery.integration.inspect', work_id: work.key,
    workstream_id: 'stream', request_id: 'existing-request' };
  const integrationItem = { ...item, next_query: integrationQuery };
  mock(url => url.includes('/inbox?') ? { ok: true, result: inboxPage([integrationItem]) }
    : { ok: true, query: integrationQuery, result: { ...result, data: { state: 'dispatched' } } });
  await ui._loadInbox(); await ui._readInboxItem(integrationItem);
  assert.match(node('teamOverview').textContent, /Requested; outcome pending/);
  assert.ok(!node('teamOverview').textContent.includes('Integration confirmation recorded'));
});

test('coordination bounds item display and sparse pages without claiming all work was read', async () => {
  let page = 0;
  await readyInbox(() => ({ ok: true, result: inboxPage(Array.from({ length: 20 }, (_, n) => inboxItem('key-' + (page * 20 + n))), 'next-' + ++page) }));
  await ui._loadInbox();
  for (let n = 0; n < 4; n++) await ui._loadInbox(true);
  assert.equal(ui.state.inbox.items.length, 100);
  const prior = page; await ui._loadInbox(true); assert.equal(page, prior);
  assert.match(node('teamOverview').textContent, /display limit was reached; more pages remain/);
  assert.equal(button('teamOverview', 'Read next page'), null);

  page = 0;
  await readyInbox(() => ({ ok: true, result: inboxPage([], 'sparse-' + ++page) }));
  await ui._loadInbox();
  for (let n = 0; n < 99; n++) await ui._loadInbox(true);
  assert.equal(page, 100); await ui._loadInbox(true); assert.equal(page, 100);
  assert.match(node('teamOverview').textContent, /display limit was reached; more pages remain/);
  assert.equal(button('teamOverview', 'Read next page'), null);
});

test('newer same-source coordination details retain their receipt and suspend old page conditions', async () => {
  const item = inboxItem();
  const detail = { ...inboxPage(), project_revision: '21', workstream_id: 'stream', data: { review: { state: 'invalidated' } } };
  const calls = await readyInbox(url => url.endsWith('/inbox-detail') ? { ok: true, query: item.next_query, result: detail }
    : { ok: true, result: inboxPage([item], 'next') });
  await ui._loadInbox(); await ui._readInboxItem(item);
  assert.equal(ui.state.inbox.error, null); assert.equal(ui.state.inbox.detail.project_revision, '21');
  assert.equal(ui.state.inbox.page.project_revision, '20'); assert.equal(ui.state.inbox.detailNewer, true);
  assert.match(node('teamOverview').textContent, /Previous review no longer applies/);
  assert.match(node('teamOverview').textContent, /detail read contains newer facts from the same source/);
  assert.equal(button('teamOverview', 'Read next page').disabled, true);
  assert.equal(button('teamOverview', 'Read current details').disabled, true);
  const previous = calls.length;
  await ui._loadInbox(true); await ui._readInboxItem(item); assert.equal(calls.length, previous);
  await ui._loadInbox(); assert.equal(ui.state.inbox.detailNewer, undefined);
  assert.equal(ui.state.inbox.detail, null); assert.equal(button('teamOverview', 'Read next page').disabled, false);
});

test('background polling does not discard later coordination pages or a detail receipt', async () => {
  const item = inboxItem();
  const calls = await readyInbox(url => url.endsWith('/inbox-detail')
    ? { ok: true, query: item.next_query, result: { ...inboxPage(), workstream_id: 'stream', data: {} } }
    : { ok: true, result: url.includes('cursor=next') ? inboxPage([item]) : inboxPage([], 'next') });
  await ui._loadInbox(); await ui._loadInbox(true);
  const prior = calls.length; await ui._refreshProgress(); assert.equal(calls.length, prior);
  assert.match(node('teamOverview').textContent, /Automatic updates pause while you read later pages/);
  await ui._readInboxItem(item);
  const receipt = ui.state.inbox.detail;
  await ui._refreshProgress(); assert.equal(ui.state.inbox.detail, receipt);
  await ui._loadInbox(); assert.equal(ui.state.inbox.detail, null);
  await ui._refreshProgress(); assert.ok(calls.length > prior);
});

for (const read of ['next-page', 'detail']) {
  test(`an already-started project poll cannot replace a foreground coordination ${read} read`, async () => {
    const item = inboxItem();
    await readyInbox(() => ({ ok: true, result: inboxPage([item], 'next') }));
    await ui._loadInbox();
    let release, started;
    const gate = new Promise(resolve => { release = resolve; });
    const entered = new Promise(resolve => { started = resolve; });
    const calls = mock(url => {
      if (url.includes('/overview?')) { started(); return gate; }
      if (url.endsWith('/inbox-detail')) return { ok: true, query: item.next_query,
        result: { ...inboxPage(), workstream_id: 'stream', data: { review: { state: 'awaiting_review' } } } };
      return { ok: true, result: url.includes('cursor=next') ? inboxPage([item]) : inboxPage([item], 'next') };
    });
    const pending = ui._refreshProgress();
    await entered;
    if (read === 'next-page') await button('teamOverview', 'Read next page').click();
    else await button('teamOverview', 'Read current details').click();
    const observed = ui.state.inbox;
    release(inboxOverview); await pending;
    assert.equal(ui.state.inbox, observed);
    assert.equal(calls.filter(call => call.url.includes('/inbox?')).length, read === 'next-page' ? 1 : 0);
    assert.equal(ui.state.refreshing, false);
    assert.match(node('teamOverview').textContent, /Automatic updates pause while you read later pages/);
  });
}

test('failed login displays an error instead of silently returning to the form', async () => {
  mock(() => ({ ok: false, error: { code: 'Forbidden', message: 'access denied' } }));
  ui.render();
  await button('teamAuth', 'Sign in').click();
  assert.match(node('teamAuth').textContent, /Forbidden.*access denied/);
  assert.equal(ui.state.session, null);
});

test('an anonymous refresh preserves a credential draft until the user submits it', async () => {
  let release;
  const pending = new Promise((resolve) => { release = resolve; });
  mock((url) => url.includes('/projects?') ? pending : { ok: false });
  const refreshing = ui.refresh();
  const input = node('teamAuth').find((el) => el.tagName === 'INPUT');
  input.value = 'synthetic-login-draft';
  let focusRestored = 0;
  document.activeElement = input;
  input.focus = () => { focusRestored++; };
  release({ ok: false, error: { code: 'Unauthenticated', message: 'cookie required' } });
  await refreshing;
  assert.equal(node('teamAuth').find((el) => el.tagName === 'INPUT'), input);
  assert.equal(input.value, 'synthetic-login-draft');
  assert.equal(focusRestored, 1);
  const calls = mock((url) => url.endsWith('/login') ? { ok: true, session_id: 'test-session' }
    : url.includes('/projects?') ? projects : overview);
  await button('teamAuth', 'Sign in').click();
  assert.equal(JSON.parse(calls[0].options.body).bearer, 'synthetic-login-draft');
  assert.equal(input.value, '');
  assert.ok(!JSON.stringify(ui.state).includes('synthetic-login-draft'));
  assert.deepEqual(ui.state.works, [work]);
});

for (const label of ['Log out', 'Revoke sessions']) {
  test(`${label} removes project data, selected detail and receipts`, async () => {
    mock((url) => url.includes('/projects?') ? projects : url.includes('/overview?') ? overview : { ok: true });
    await ui.refresh();
    ui.state.selected = work.key;
    ui.state.lastReceipts.saved = 'receipt';
    ui.render();
    await button('teamAuth', label).click();
    assert.equal(ui.state.session, null);
    assert.equal(ui.state.projectKey, null);
    assert.deepEqual(ui.state.projects, []);
    assert.deepEqual(ui.state.works, []);
    assert.equal(ui.state.selected, null);
    assert.equal(ui.state.raw, null);
    assert.equal(node('rawTeamBody').textContent, '');
    assert.equal(Object.keys(ui.state.lastReceipts).length, 0);
    assert.ok(!node('teamDetail').textContent.includes('Example work'));
    assert.equal(node('view-team').dataset.teamState, 'signed-out');
    for (const id of ['teamWorkspaceGrid', 'teamProjects', 'teamDetail', 'teamRaw']) {
      assert.equal(node(id).hidden, true, `${id} must be hidden after signing out`);
    }
  });
}

test('a denied refresh clears stale data and renders the error', async () => {
  mock((url) => url.includes('/projects?') ? projects : overview);
  await ui.refresh();
  mock(() => ({ ok: false, error: { code: 'SessionExpired', message: 'expired' } }));
  await ui.refresh();
  assert.equal(ui.state.session, null);
  assert.deepEqual(ui.state.works, []);
  assert.deepEqual(ui.state.projects, []);
  assert.match(node('teamAuth').textContent, /SessionExpired/);
});

test('the first anonymous visit is a sign-in state and raw JSON omits cookie identifiers', async () => {
  mock(() => ({ ok: false, error: { code: 'Unauthenticated', message: 'cookie required' } }));
  await ui.refresh();
  assert.equal(ui.state.error, null);
  assert.equal(node('view-team').dataset.teamState, 'signed-out');
  for (const id of ['teamWorkspaceGrid', 'teamProjects', 'teamDetail', 'teamRaw']) {
    assert.equal(node(id).hidden, true, `${id} must be hidden for an anonymous visitor`);
  }
  assert.equal(node('teamWorkspaceIntro').hidden, false);
  mock((url) => url.includes('/projects?') ? projects
    : { ...overview, session: { session_id: 'secret-cookie-identifier' } });
  await ui.refresh();
  assert.match(node('rawTeamBody').textContent, /Example work/);
  assert.ok(!node('rawTeamBody').textContent.includes('secret-cookie-identifier'));
});

test('an authenticated account without projects shows access guidance with no empty controls', async () => {
  mock(() => ({ ...projects, projects: [] }));
  await ui.refresh();
  assert.equal(node('view-team').dataset.teamState, 'signed-in');
  assert.equal(node('teamWorkspaceGrid').hidden, false);
  assert.equal(node('teamProjects').hidden, true);
  assert.equal(node('teamDetail').hidden, true);
  assert.equal(node('teamRaw').hidden, true);
  assert.match(node('teamOverview').textContent, /Ask your team administrator for access/);
  assert.equal(button('teamAuth', 'Sign in'), null);
  assert.equal(button('teamOverview', 'List'), null);
  assert.ok(button('teamAuth', 'Log out'));
});

test('an overview failure is visible and never leaves old work on screen', async () => {
  mock((url) => url.includes('/projects?') ? projects : overview);
  await ui.refresh();
  mock((url) => url.includes('/projects?') ? projects : { ok: false, error: { code: 'Forbidden', message: 'grant revoked' } });
  await ui.refresh();
  assert.deepEqual(ui.state.works, []);
  assert.match(node('teamAuth').textContent, /grant revoked/);
});

test('a late refresh cannot restore data after logout', async () => {
  mock((url) => url.includes('/projects?') ? projects : overview);
  await ui.refresh();
  let resolve;
  const gate = new Promise((r) => { resolve = r; });
  mock((url) => url.includes('/projects?') ? gate : { ok: true });
  const pending = ui.refresh();
  await button('teamAuth', 'Log out').click();
  resolve(projects);
  await pending;
  assert.equal(ui.state.session, null);
  assert.deepEqual(ui.state.projects, []);
  assert.deepEqual(ui.state.works, []);
});

test('a late project response cannot replace the latest selection', async () => {
  let resolve;
  const gate = new Promise((r) => { resolve = r; });
  mock(() => gate);
  const old = ui.refresh();
  mock((url) => url.includes('/projects?') ? { ...projects, projects: [{ key: 'new' }] }
    : { ok: true, works: [{ key: 'NEW' }] });
  await ui.refresh();
  resolve(projects);
  await old;
  assert.equal(ui.state.projectKey, 'new');
  assert.deepEqual(ui.state.works, [{ key: 'NEW' }]);
});

test('failed action shows the server error and creates no success receipt', async () => {
  mock((url) => url.includes('/projects?') ? projects : overview);
  await ui.refresh();
  ui.state.selected = work.key;
  ui.render();
  mock(() => ({ ok: false, error: { code: 'InvalidInput', message: 'command required' } }));
  await button('teamDetail', 'Accept responsibility').click();
  assert.match(node('teamAuth').textContent, /InvalidInput.*command required/);
  assert.equal(Object.keys(ui.state.lastReceipts).length, 0);
});

test('transport failure explains unknown outcome without replaying the operation', async () => {
  let calls = 0;
  global.fetch = async () => { calls++; throw new Error('offline'); };
  ui.state.works = [work];
  ui.state.selected = work.key;
  ui.render();
  await button('teamDetail', 'Accept responsibility').click();
  assert.equal(calls, 1);
  assert.match(node('teamAuth').textContent, /BridgeUnreachable/);
  assert.match(node('teamAuth').textContent, /Inspect the original operation/);
});

test('live work uses authorized details and directs unsupported writes to MCP', async () => {
  const liveWork = { ...work, workstream_id: 'stream', detail_loaded: false };
  const detail = { ...liveWork, detail_loaded: true, runtime_available: false,
    status: null, acceptance: ['A real contract criterion'], depends_on: [],
    dependency_export_unavailable: true, context_complete: false,
    completeness_reasons: ['dependency_export_unavailable'] };
  const calls = mock((url) => url.includes('/projects?') ? projects
    : url.includes('/overview?') ? { ...overview, works: [liveWork], interaction_mode: 'mcp' }
    : { ok: true, work: detail });
  await ui.refresh();
  const card = node('teamOverview').find((el) => el.tagName === 'ARTICLE');
  assert.equal(card.getAttribute('tabindex'), '0');
  await card.click();
  assert.match(node('teamDetail').textContent, /A real contract criterion/);
  assert.match(node('teamDetail').textContent, /No execution recorded/);
  assert.match(node('teamDetail').textContent, /Readiness cannot be confirmed/);
  assert.match(node('teamDetail').textContent, /MCP/);
  assert.ok(!button('teamDetail', 'Accept responsibility'));
  assert.equal(button('teamOverview', 'Personal'), null);
  assert.match(calls[2].url, /workstream=stream/);
});

test('live browsing and copied Agent instructions do not create sessions or claims', async () => {
  const endpoint = 'https://team.example/v1/projects/example/mcp';
  const liveWork = { ...work, workstream_id: 'stream', detail_loaded: true,
    context_complete: true, acceptance: ['Deliver the feature'], depends_on: [] };
  const calls = mock((url) => url.includes('/projects?') ? projects
    : { ...overview, works: [liveWork], interaction_mode: 'mcp', mcp_url: endpoint });
  const original = Object.getOwnPropertyDescriptor(global, 'navigator');
  const copied = [];
  Object.defineProperty(global, 'navigator', { configurable: true,
    value: { clipboard: { writeText: async (text) => copied.push(text) } } });
  try {
    await ui.refresh();
    await ui._selectWork(work.key);
    assert.equal(button('teamDetail', 'Claim task'), null);
    assert.equal(button('teamDetail', 'Refresh my claim'), null);
    assert.match(node('teamDetail').textContent, /Agent refreshes tasks, claims work/);
    await button('teamAuth', 'Copy MCP connection details').click();
    await button('teamAuth', 'Copy project instruction').click();
    await button('teamDetail', 'Copy task brief').click();
    assert.match(copied[0], /Transport: Streamable HTTP/);
    assert.match(copied[0], /Authorization: Bearer <PERSONAL_ACCESS_CREDENTIAL>/);
    assert.doesNotMatch(copied[0], /codex|AWR_TEAM_BEARER/);
    assert.doesNotMatch(node('teamAuth').textContent, /Codex|Other MCP clients/);
    assert.match(copied[1], /Web sign-in is not required/);
    assert.match(copied[2], /WORK-1 in workstream stream/);
    assert.match(copied[2], /session owned by this identity and client/);
    assert.match(copied[2], /does not claim the task or authorize execution/);
    assert.ok(copied.every((text) => text.includes(endpoint) && !text.includes('test-session')));
    await ui.refresh();
    assert.ok(calls.every(({ options }) => !options.method || options.method === 'GET'));
  } finally {
    if (original) Object.defineProperty(global, 'navigator', original);
    else delete global.navigator;
  }
});

for (const locale of ['en', 'zh-CN']) {
  test(`neutral delivery stages and current guidance remain distinct in ${locale}`, async () => {
    i18n.setLocale(locale);
    const query = { protocol_version: 1, op: 'delivery.integration.inspect', work_id: work.key,
      workstream_id: 'stream', request_id: 'original-integration' };
    const liveWork = { ...work, workstream_id: 'stream', detail_loaded: true, context_complete: true,
      status: 'in_progress', owner_person: 'Responsible member', agent: 'Execution Agent', client_id: 'actual-client',
      contract_hash: 'requirements-version', snapshot: { project_revision: '44', source_snapshot_id: 'source-version',
        queried_at_unix_ms: 1234, consistency: 'repeatable_read' },
      guidance: { code: 'integration_unknown', when: 'Integration has no confirmed outcome',
        because: ['Original request is unsettled'], action: { op: query.op, query }, recheck_on: 'Integration observation changes' },
      collaboration: { candidate: { digest: 'neutral-binding', current: true, selection_version: '2', required_checks: ['search'] },
        verification: [{ check: 'search', run_id: 'search-run', outcome: 'passed' }],
        review: { state: 'approved' }, integration: { state: 'unknown' }, publication: { phase: 'source_written' }, facts_truncated: true } };
    const calls = mock((url) => url.includes('/projects?') ? projects
      : { ...overview, works: [liveWork], interaction_mode: 'mcp' });
    await ui.refresh(); await ui._selectWork(work.key);
    const text = node('teamDetail').textContent;
    for (const value of ['Responsible member', 'Execution Agent', 'actual-client', 'neutral-binding', 'source-version',
      'requirements-version', 'Integration has no confirmed outcome', 'Original request is unsettled', 'Integration observation changes'])
      assert.ok(text.includes(value), value);
    for (const key of ['feedback.review_approved', 'feedback.integration_unknown', 'feedback.publication_source_written',
      'feedback.check_passed', 'feedback.delivery_dimensions', 'feedback.delivery_truncated', 'feedback.guidance_integration_unknown'])
      assert.ok(text.includes(i18n.t(key)), key);
    const selector = node('teamDetail').find(e => e.tagName === 'PRE' && e.textContent.includes('original-integration'));
    assert.deepEqual(JSON.parse(selector.textContent), query);
    assert.equal(button('teamDetail', 'Claim task'), null);
    assert.equal(button('teamDetail', 'Accept responsibility'), null);
    assert.ok(calls.every(({ options }) => !options.method || options.method === 'GET'));
  });
}

test('missing, stale and new delivery states cannot imply successful delivery', async () => {
  for (const collaboration of [null, { candidate: { current: false }, verification: [], review: null,
    integration: { state: 'new-provider-state' }, publication: null }]) {
    const liveWork = { ...work, workstream_id: 'stream', detail_loaded: true, context_complete: true, collaboration };
    mock(url => url.includes('/projects?') ? projects : { ...overview, works: [liveWork], interaction_mode: 'mcp' });
    await ui.refresh(); await ui._selectWork(work.key);
    const text = node('teamDetail').textContent;
    assert.ok(text.includes(i18n.t(collaboration ? 'feedback.binding_changed_help' : 'feedback.delivery_unavailable')));
    assert.ok(!text.includes(i18n.t('feedback.review_approved')));
    assert.ok(!text.includes(i18n.t('feedback.publication_confirmed')));
    if (collaboration) assert.ok(text.includes('Unrecognized recorded state: new-provider-state'));
  }
});

test('guidance query disclosure survives refresh only for the same identity and source binding', async () => {
  const liveWork = { ...work, workstream_id: 'stream', detail_loaded: true, context_complete: true,
    contract_hash: 'current', snapshot: { source_snapshot_id: 'source', coordinator_epoch: 'epoch' },
    guidance: { code: 'assignment', action: { query: { op: 'work.prepare', work_id: work.key } } } };
  mock(url => url.includes('/projects?') ? projects : { ...overview, works: [liveWork], interaction_mode: 'mcp' });
  await ui.refresh(); await ui._selectWork(work.key);
  const disclosure = () => node('teamDetail').find(e => e.tagName === 'DETAILS'
    && e.textContent.includes('Read selector for the connected Agent'));
  const initial = disclosure(); initial.open = true;
  initial.listeners.toggle[0]();
  ui.render(); assert.equal(disclosure().open, true);
  liveWork.snapshot = { ...liveWork.snapshot, source_snapshot_id: 'new-source' };
  await ui.refresh(); assert.equal(disclosure().open, false);
  assert.ok(!node('teamDetail').textContent.includes('GitHub PR'));
});

test('late details for a previously selected work never replace the latest selection', async () => {
  const a = { ...work, workstream_id: 'stream' };
  const b = { key: 'WORK-2', title: 'Second work', workstream_id: 'stream' };
  let release;
  const gate = new Promise((resolve) => { release = resolve; });
  const detail = (item) => ({ ok: true, work: { ...item, detail_loaded: true,
    acceptance: [item.title], depends_on: [], context_complete: true } });
  mock((url) => url.includes('/projects?') ? projects
    : url.includes('/overview?') ? { ok: true, works: [a, b], interaction_mode: 'mcp' }
    : url.includes('work=WORK-1') ? gate : detail(b));
  await ui.refresh();
  const pending = node('teamOverview').find((el) => el.tagName === 'ARTICLE' && el.dataset.key === 'WORK-1').click();
  await node('teamOverview').find((el) => el.tagName === 'ARTICLE' && el.dataset.key === 'WORK-2').click();
  release(detail(a));
  await pending;
  assert.equal(ui.state.selected, 'WORK-2');
  assert.ok(!node('teamDetail').textContent.includes('Example work'));
});
