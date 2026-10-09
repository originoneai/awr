const { test, beforeEach } = require('node:test');
const assert = require('node:assert/strict');
const { webcrypto, createHash } = require('node:crypto');
const { install } = require('./fixtures/dom-stub');
const { createTeamAdmin } = require('../public/team-admin');
const { generateCredential } = require('../public/team-onboarding');
const i18n = require('../public/i18n');
let ui, calls, handler, copied;
const node = id => document.getElementById(id);
const find = (id, tag, text) => node(id).find(n => n.tagName === tag && (!text || n.textContent === text));
const click = (id, text) => { const b = find(id, 'BUTTON', text); assert.ok(b, text); return b.click(); };
const context = { project: 'test', session: 'session', mcpUrl: 'https://team.example/v1/projects/test/mcp',
  identity: { actor_id: 'admin', client_id: 'admin-client', can_manage_members: true, can_read_project_audit: true } };
const directory = { ok: true, data: { items: [{ actor_id: 'alex', display_name: 'Alex', kind: 'human', role: 'developer', independent_review: false,
  clients: [{ client_id: 'alex-agent', credentials: [], grants: [{ workstream_id: 'stream', authority_version: '1', read: true, write: true, manage: false, active: true }] }] }],
  workstreams: [{ id: 'stream', authority_version: '1', write: true }], next_cursor: null } };
function previewResponse(plan) {
  const desired = structuredClone(plan);
  if (desired.credential) delete desired.credential.secret_hash;
  return { ok: true, data: { applied: false, state_digest: 'state', plan_digest: 'plan', desired } };
}
function lastApply() { return calls.findLast(c => c.body?.operation === 'apply')?.body.payload; }
function savedResponse(payload = lastApply(), replayed = false) {
  const plan = payload.plan;
  return { ok: true, data: { replayed, receipt: {
    protocol: 'awr-project-admin-access-v1', request_id: payload.request_id,
    admin_actor_id: context.identity.actor_id, admin_client_id: context.identity.client_id,
    subject_actor_id: plan.subject.id, subject_client_id: plan.subject_client_id,
    before_digest: payload.expected_state, plan_digest: payload.expected_plan,
    after_digest: 'after', project_revision: '12', desired: previewResponse(plan).data.desired,
  } } };
}
beforeEach(() => {
  install(); i18n.setLocale('en'); calls = []; copied = '';
  window.crypto = webcrypto; window.navigator = { clipboard: { writeText: async text => { copied = text; } } };
  navigator.clipboard = window.navigator.clipboard;
  handler = async (op, payload) => op === 'inspect' ? directory : op === 'preview'
    ? previewResponse(payload.plan) : savedResponse(payload);
  ui = createTeamAdmin({ $: node, i18n, onAuthError: () => ui.reset(), api: async (url, options) => {
    const b = options ? JSON.parse(options.body) : null; calls.push({ url, body: b });
    return handler(b && b.operation, b && b.payload, url);
  } });
  ui.setContext(context);
});
test('credential generation matches the server hash and is fresh each time', async () => {
  const a = await generateCredential(webcrypto), b = await generateCredential(webcrypto);
  assert.match(a.bearer, /^awr1\.member-[0-9a-f]{24}\.[0-9a-f]{64}$/);
  assert.notEqual(a.bearer, b.bearer);
  assert.equal(a.credential.secret_hash, 'sha256:' + createHash('sha256').update('awr-team-credential-v1:' + a.bearer).digest('hex'));
});
test('ordinary members have only project and personal activity tabs', async () => {
  ui.setContext({ ...context, identity: { can_manage_members: false, can_read_project_audit: false } });
  assert.equal(find('teamConsoleTabs', 'BUTTON', 'Members'), null);
  handler = async () => ({ ok: true, data: { scope: 'self', items: [], next_cursor: null } });
  await click('teamConsoleTabs', 'Activity');
  assert.match(node('teamConsolePanel').textContent, /Your activity/);
  assert.equal(node('teamConsolePanel').find(n => n.placeholder === 'Member ID (optional)'), null);
});
test('one-time generic instructions contain only the new personal bearer and never send it to the bridge', async () => {
  await click('teamConsoleTabs', 'Members'); await click('teamConsolePanel', 'Generate connection instructions');
  const plan = calls.find(c => c.body && c.body.operation === 'preview').body.payload.plan;
  assert.ok(plan.grants.every(g => g.attest_execution === false && g.reconcile_execution === false), 'explicit server grant fields must be present without special executor authority');
  assert.equal(calls.filter(c => c.body && c.body.operation === 'apply').length, 1);
  const area = find('teamConsolePanel', 'TEXTAREA'); assert.ok(area);
  assert.match(area.value, /Authorization: Bearer awr1\.member-/);
  assert.match(area.value, /work.next/); assert.ok(!area.value.includes('Codex'));
  assert.ok(!JSON.stringify(calls).includes('awr1.'));
  await click('teamConsolePanel', 'Copy instructions for this member'); assert.equal(copied, area.value);
  await click('teamConsolePanel', 'Done');
  assert.equal(find('teamConsolePanel', 'TEXTAREA'), null);
  assert.ok(!JSON.stringify(ui).includes('awr1.'));
});
test('unknown issuance checks the original ID before exact retry and keeps one credential', async () => {
  await click('teamConsoleTabs', 'Members');
  const previous = handler;
  handler = async (op, payload) => op === 'apply' ? { ok: false, error: { code: 'BridgeUnreachable' } }
    : op === 'outcome' ? { ok: true, data: { outcome: 'unknown' } } : previous(op, payload);
  await click('teamConsolePanel', 'Generate connection instructions');
  assert.equal(find('teamConsolePanel', 'TEXTAREA'), null);
  assert.equal(find('teamConsolePanel', 'BUTTON', 'Try saving again'), null);
  await click('teamConsolePanel', 'Check whether it was saved');
  handler = async () => savedResponse(lastApply(), true);
  await click('teamConsolePanel', 'Try saving again');
  const applies = calls.filter(c => c.body && c.body.operation === 'apply');
  assert.equal(applies.length, 2); assert.deepEqual(applies[0].body, applies[1].body);
  assert.ok(find('teamConsolePanel', 'TEXTAREA'));
});
test('logout or project switch clears one-time credentials and ignores late issuance', async () => {
  await click('teamConsoleTabs', 'Members');
  const previous = handler;
  let resolve, signal;
  const arrived = new Promise(r => { signal = r; });
  handler = (op, payload) => op === 'apply' ? new Promise(r => { resolve = r; signal(); }) : previous(op, payload);
  const applying = click('teamConsolePanel', 'Generate connection instructions');
  await arrived;
  ui.reset(); resolve(savedResponse()); await applying;
  assert.equal(find('teamConsolePanel', 'TEXTAREA'), null); assert.equal(node('teamConsoleTabs').hidden, true);
});
test('committed access removal refreshes the directory without stale member controls', async () => {
  await click('teamConsoleTabs', 'Members');
  await click('teamConsolePanel', 'Remove member');
  handler = async op => op === 'inspect'
    ? { ok: true, data: { ...directory.data, items: [] } }
    : savedResponse();
  await click('teamConsolePanel', 'Remove this member');
  assert.equal(find('teamConsolePanel', 'BUTTON', 'Remove member'), null);
  assert.equal(find('teamConsolePanel', 'BUTTON', 'Add member').disabled, false);
  assert.equal(calls.at(-1).body.operation, 'inspect');
});
test('untrusted names stay text and locale switch uses translated labels', async () => {
  directory.data.items[0].display_name = '<img src=x onerror=alert(1)>';
  await click('teamConsoleTabs', 'Members');
  assert.ok(find('teamConsolePanel', 'H3', '<img src=x onerror=alert(1)>'));
  i18n.setLocale('zh-CN'); ui.render();
  assert.ok(find('teamConsoleTabs', 'BUTTON', '成员'));
  directory.data.items[0].display_name = 'Alex';
});

function inputName(text) {
  const label = node('teamConsolePanel').find(n => n.tagName === 'LABEL' && n.children[0]?.textContent === 'Name');
  const input = label.find(n => n.tagName === 'INPUT');
  input.value = text;
  for (const fn of input.listeners.input || []) fn({ target: input });
  return input;
}

test('a name is sufficient to create a member and personal handoff in one explicit action', async () => {
  await click('teamConsoleTabs', 'Members'); await click('teamConsolePanel', 'Add member');
  assert.equal(node('teamConsolePanel').find(n => n.tagName === 'LABEL' && n.textContent.includes('Member ID')), null);
  inputName('New colleague');
  const submit = find('teamConsolePanel', 'BUTTON', 'Add member and generate connection instructions');
  assert.match(submit.className, /primary/);
  await Promise.all([submit.click(), submit.click()]);
  const previews = calls.filter(c => c.body?.operation === 'preview');
  const applies = calls.filter(c => c.body?.operation === 'apply');
  assert.equal(previews.length, 1); assert.equal(applies.length, 1);
  const plan = applies[0].body.payload.plan;
  assert.match(plan.subject.id, /^member-[0-9a-f-]{36}$/);
  assert.equal(plan.subject.display_name, 'New colleague');
  assert.equal(plan.subject_client_id, plan.subject.id + '-agent');
  assert.equal(plan.credential_project_scoped, true);
  assert.equal(plan.role, 'developer');
  assert.deepEqual(plan.grants, [{ workstream_id: 'stream', authority_version: '1', read: true, write: true, manage: false, attest_execution: false, reconcile_execution: false }]);
  assert.ok(find('teamConsolePanel', 'TEXTAREA'));
  assert.match(node('teamConsolePanel').textContent, /New colleague has been added/);
  assert.equal(find('teamConsolePanel', 'BUTTON', 'Confirm changes'), null);
  assert.ok(!JSON.stringify(calls).includes('awr1.'));
});

test('blank names and empty work areas show actionable errors without writes', async () => {
  await click('teamConsoleTabs', 'Members'); await click('teamConsolePanel', 'Add member');
  inputName('   '); await click('teamConsolePanel', 'Add member and generate connection instructions');
  assert.match(node('teamConsolePanel').textContent, /Enter a member name/);
  inputName('Alex');
  const scopes = node('teamConsolePanel').find(n => n.tagName === 'FIELDSET' && n.children[0]?.textContent === 'Where they can work');
  const checkbox = scopes.find(n => n.tagName === 'INPUT'); checkbox.checked = false;
  for (const fn of checkbox.listeners.change) fn({ target: checkbox });
  await click('teamConsolePanel', 'Add member and generate connection instructions');
  assert.match(node('teamConsolePanel').textContent, /Select at least one work area/);
  assert.equal(calls.filter(c => c.body?.operation !== 'inspect').length, 0);
});

test('submitting the form with Enter follows the same creation path', async () => {
  await click('teamConsoleTabs', 'Members'); await click('teamConsolePanel', 'Add member');
  inputName('Keyboard user');
  const form = find('teamConsolePanel', 'FORM');
  for (const fn of form.listeners.submit) await fn({ preventDefault() {} });
  assert.equal(calls.filter(c => c.body?.operation === 'apply').length, 1);
  assert.ok(find('teamConsolePanel', 'TEXTAREA'));
});

test('network exceptions during saving retain one recoverable request and never expose an uncommitted secret', async () => {
  await click('teamConsoleTabs', 'Members');
  const previous = handler;
  handler = (op, payload) => { if (op === 'apply') throw Error('lost connection'); return previous(op, payload); };
  await click('teamConsolePanel', 'Generate connection instructions');
  assert.equal(find('teamConsolePanel', 'TEXTAREA'), null);
  assert.ok(find('teamConsolePanel', 'BUTTON', 'Check whether it was saved'));
  handler = async () => ({ ok: true, data: { outcome: 'committed', receipt: savedResponse().data.receipt } });
  await click('teamConsolePanel', 'Check whether it was saved');
  assert.ok(find('teamConsolePanel', 'TEXTAREA'));
  assert.equal(calls.filter(c => c.body?.operation === 'apply').length, 1);
});

test('a rejected save preserves the member draft for correction', async () => {
  await click('teamConsoleTabs', 'Members'); await click('teamConsolePanel', 'Add member'); inputName('Keep this name');
  const previous = handler;
  handler = (op, payload) => op === 'apply' ? { ok: false, error: { code: 'PreconditionsChanged' } } : previous(op, payload);
  await click('teamConsolePanel', 'Add member and generate connection instructions');
  assert.equal(find('teamConsolePanel', 'TEXTAREA'), null);
  await click('teamConsolePanel', 'Back to edit');
  assert.equal(find('teamConsolePanel', 'INPUT').value, 'Keep this name');
});

test('revoking administrator access clears a prepared personal credential', async () => {
  await click('teamConsoleTabs', 'Members'); await click('teamConsolePanel', 'Generate connection instructions');
  assert.ok(find('teamConsolePanel', 'TEXTAREA'));
  ui.setContext({ ...context, identity: { can_manage_members: false, can_read_project_audit: false } });
  assert.equal(find('teamConsolePanel', 'TEXTAREA'), null);
  assert.equal(find('teamConsoleTabs', 'BUTTON', 'Members'), null);
});

test('replacing a credential describes the impact and waits for explicit confirmation', async () => {
  const data = structuredClone(directory);
  data.data.items[0].clients[0].credentials = [{ id: 'existing', project_scoped: true }];
  const previous = handler;
  handler = (op, payload) => op === 'inspect' ? data : previous(op, payload);
  await click('teamConsoleTabs', 'Members'); await click('teamConsolePanel', 'Replace credential');
  assert.equal(calls.filter(c => c.body?.operation === 'apply').length, 0);
  assert.match(node('teamConsolePanel').textContent, /old credential will stop working/);
  assert.equal(find('teamConsolePanel', 'PRE'), null);
  await click('teamConsolePanel', 'Replace this credential');
  const plan = calls.find(c => c.body?.operation === 'apply').body.payload.plan;
  assert.deepEqual(plan.revoke_project_credentials, ['existing']);
  assert.ok(plan.credential && find('teamConsolePanel', 'TEXTAREA'));
});

test('members without work areas are directed to permissions before credential generation', async () => {
  const data = structuredClone(directory); data.data.items[0].clients[0].grants = [];
  handler = async () => data;
  await click('teamConsoleTabs', 'Members');
  assert.equal(find('teamConsolePanel', 'BUTTON', 'Generate connection instructions').disabled, true);
  assert.match(node('teamConsolePanel').textContent, /Choose work areas/);
  assert.equal(find('teamConsolePanel', 'BUTTON', 'Edit access').disabled, false);
});

test('audit filters display names but send the authoritative member identity', async () => {
  const previous = handler;
  handler = (op, payload) => op === null ? { ok: true, data: { items: [{ actor_id: 'alex', action: 'claim.acquire', result: 'succeeded' }], next_cursor: null } } : previous(op, payload);
  await click('teamConsoleTabs', 'Activity');
  const picker = node('teamConsolePanel').find(n => n.tagName === 'SELECT' && n.getAttribute('aria-label') === 'Member');
  assert.equal(picker.find(n => n.value === 'alex').textContent, 'Alex');
  picker.value = 'alex'; for (const fn of picker.listeners.change) fn();
  await click('teamConsolePanel', 'Apply filters');
  assert.match(calls.at(-1).url, /member_actor_id=alex/);
  assert.match(node('teamConsolePanel').textContent, /Claim a task/);
  assert.match(node('teamConsolePanel').textContent, /Succeeded/);
});


test('credential maintenance preserves an authenticated Agent review policy', async () => {
  const data = structuredClone(directory);
  Object.assign(data.data.items[0], { kind: 'agent', agent_review: true, business_roles: ['developer', 'reviewer'] });
  const previous = handler;
  handler = (op, payload) => op === 'inspect' ? data : previous(op, payload);
  await click('teamConsoleTabs', 'Members');
  await click('teamConsolePanel', 'Generate connection instructions');
  const plan = calls.find(c => c.body?.operation === 'preview').body.payload.plan;
  assert.equal(plan.agent_review, true, 'issuing a credential must not revoke the existing Agent review opt-in');
  assert.deepEqual(plan.business_roles, ['developer', 'reviewer']);
});

function useDirectory(data) {
  const previous = handler;
  handler = (op, payload, url) => op === 'inspect' ? data : previous(op, payload, url);
}
function changeInput(input, checked) {
  assert.ok(input); input.checked = checked;
  for (const fn of input.listeners.change || []) fn({ target: input });
}
function chooseDuty(duty, checked) {
  changeInput(node('teamConsolePanel').find(n => n.tagName === 'INPUT' && n.value === duty), checked);
}
function togglePermission(label, checked) {
  const row = node('teamConsolePanel').find(n => n.tagName === 'LABEL' && n.textContent === label);
  changeInput(row && row.find(n => n.tagName === 'INPUT'), checked);
}
function toggleWorkArea(name, checked) {
  const scopes = node('teamConsolePanel').find(n => n.tagName === 'FIELDSET' && n.children[0]?.textContent === 'Where they can work');
  const row = scopes.find(n => n.tagName === 'LABEL' && n.textContent === name);
  changeInput(row && row.find(n => n.tagName === 'INPUT'), checked);
}

for (const [begin, confirm] of [
  ['Generate connection instructions', null], ['Replace credential', 'Replace this credential'],
  ['Disable credential', 'Disable this credential'],
]) test(`${begin} preserves duties, both review flags, assignment and exact active grants`, async () => {
  const data = structuredClone(directory), member = data.data.items[0];
  Object.assign(member, { kind: 'agent', role: 'maintainer', independent_review: true, agent_review: true,
    business_roles: ['supervisor', 'reviewer', 'deliverer'], assignment_grant: true });
  Object.assign(member.clients[0], { credentials: [{ id: 'existing', project_scoped: true }], grants: [
    { workstream_id: 'stream', authority_version: '1', read: true, write: false, manage: false, active: true },
    { workstream_id: 'old', authority_version: '4', read: true, write: true, manage: false, active: false },
  ] });
  useDirectory(data);
  await click('teamConsoleTabs', 'Members'); await click('teamConsolePanel', begin);
  if (confirm) await click('teamConsolePanel', confirm);
  const plan = calls.find(c => c.body?.operation === 'apply').body.payload.plan;
  assert.equal(plan.role, 'maintainer'); assert.equal(plan.independent_review, true); assert.equal(plan.agent_review, true);
  assert.equal(plan.assignment_grant, true); assert.deepEqual(plan.business_roles, member.business_roles);
  assert.deepEqual(plan.grants, [{ workstream_id: 'stream', authority_version: '1', read: true, write: false,
    manage: false, attest_execution: false, reconcile_execution: false }]);
});

test('explicit supervisor and review duties do not imply assignment or review opt-ins', async () => {
  await click('teamConsoleTabs', 'Members'); await click('teamConsolePanel', 'Add member'); inputName('Supervisor');
  chooseDuty('developer', false); chooseDuty('supervisor', true); chooseDuty('reviewer', true);
  await click('teamConsolePanel', 'Add member and generate connection instructions');
  const plan = lastApply().plan;
  assert.equal(plan.role, 'maintainer'); assert.deepEqual(plan.business_roles, ['supervisor', 'reviewer']);
  assert.equal(plan.assignment_grant, false); assert.equal(plan.independent_review, false); assert.equal(plan.agent_review, false);
});

test('assignment requires an explicit opt-in and a compatible access template', async () => {
  await click('teamConsoleTabs', 'Members'); await click('teamConsolePanel', 'Add member'); inputName('Supervisor');
  togglePermission('Allow task assignment to members', true);
  await click('teamConsolePanel', 'Add member and generate connection instructions');
  assert.equal(lastApply(), undefined); assert.match(node('teamConsolePanel').textContent, /Assignment requires/);
  chooseDuty('supervisor', true); await click('teamConsolePanel', 'Add member and generate connection instructions');
  assert.equal(lastApply().plan.assignment_grant, true); assert.equal(lastApply().plan.role, 'maintainer');
});

test('administrator duties do not silently include development, review or delivery', async () => {
  await click('teamConsoleTabs', 'Members'); await click('teamConsolePanel', 'Add member'); inputName('Manager');
  chooseDuty('developer', false); chooseDuty('administrator', true);
  await click('teamConsolePanel', 'Add member and generate connection instructions');
  assert.deepEqual(lastApply().plan.business_roles, ['administrator']); assert.equal(lastApply().plan.role, 'project_admin');
  assert.equal(lastApply().plan.independent_review, false); assert.equal(lastApply().plan.assignment_grant, false);
});

test('empty duties and observer review do not create invalid or elevated members', async () => {
  await click('teamConsoleTabs', 'Members'); await click('teamConsolePanel', 'Add member'); inputName('Observer');
  chooseDuty('developer', false); await click('teamConsolePanel', 'Add member and generate connection instructions');
  assert.match(node('teamConsolePanel').textContent, /Select at least one responsibility/); assert.equal(lastApply(), undefined);
  chooseDuty('observer', true); togglePermission('Allow independent review of other members’ work', true);
  await click('teamConsolePanel', 'Add member and generate connection instructions'); assert.equal(lastApply(), undefined);
  togglePermission('Allow independent review of other members’ work', false);
  await click('teamConsolePanel', 'Add member and generate connection instructions');
  assert.equal(lastApply().plan.role, 'reader'); assert.equal(lastApply().plan.grants[0].write, false);
});

test('legacy policies and read-only scope rights survive unrelated edits and new scope selection', async () => {
  const data = structuredClone(directory), member = data.data.items[0]; member.role = 'worker';
  member.clients[0].grants[0].write = false;
  data.data.workstreams.push({ id: 'new', title: 'New work', authority_version: '3', write: true });
  useDirectory(data); await click('teamConsoleTabs', 'Members'); await click('teamConsolePanel', 'Edit access');
  assert.match(node('teamConsolePanel').textContent, /Leave responsibilities unchanged/);
  toggleWorkArea('New work', true); await click('teamConsolePanel', 'Save permissions');
  const plan = lastApply().plan;
  assert.equal(plan.role, 'worker'); assert.equal(Object.hasOwn(plan, 'business_roles'), false);
  assert.equal(plan.grants[0].write, false); assert.equal(plan.grants[1].write, true);
});

test('explicit Agent review and assignment revocations are sent without erasing unrelated flags', async () => {
  const data = structuredClone(directory), member = data.data.items[0];
  Object.assign(member, { kind: 'agent', role: 'maintainer', business_roles: ['supervisor', 'reviewer'],
    independent_review: true, agent_review: true, assignment_grant: true });
  useDirectory(data); await click('teamConsoleTabs', 'Members'); await click('teamConsolePanel', 'Edit access');
  togglePermission('Allow this Agent to review other members’ work', false);
  togglePermission('Allow task assignment to members', false); await click('teamConsolePanel', 'Save permissions');
  assert.equal(lastApply().plan.agent_review, false); assert.equal(lastApply().plan.assignment_grant, false);
  assert.equal(lastApply().plan.independent_review, true); assert.deepEqual(lastApply().plan.business_roles, member.business_roles);
});

test('explicit policy changes are caller-bounded and retain the selected responsibilities', async () => {
  const data = structuredClone(directory), member = data.data.items[0];
  Object.assign(member, { role: 'project_admin', business_roles: ['administrator'] });
  Object.assign(member.clients[0].grants[0], { write: true, manage: true }); data.data.workstreams[0].write = false;
  useDirectory(data); await click('teamConsoleTabs', 'Members'); await click('teamConsolePanel', 'Edit access');
  chooseDuty('administrator', false); chooseDuty('developer', true);
  await click('teamConsolePanel', 'Save permissions');
  assert.equal(lastApply().plan.role, 'developer'); assert.deepEqual(lastApply().plan.business_roles, ['developer']);
  assert.equal(lastApply().plan.grants[0].write, false); assert.equal(lastApply().plan.grants[0].manage, false);
});

for (const special of ['attest_execution', 'reconcile_execution']) test(`${special} is never silently removed by credential maintenance or editing`, async () => {
  const data = structuredClone(directory); data.data.items[0].clients[0].grants[0][special] = true;
  useDirectory(data); await click('teamConsoleTabs', 'Members'); await click('teamConsolePanel', 'Generate connection instructions');
  assert.match(node('teamConsolePanel').textContent, /special execution permissions/);
  assert.equal(calls.some(c => c.body?.operation === 'preview'), false);
  await click('teamConsolePanel', 'Edit access'); await click('teamConsolePanel', 'Save permissions');
  assert.equal(calls.some(c => c.body?.operation === 'preview'), false);
});

test('unknown legacy roles are not promoted unless duties are explicitly selected', async () => {
  const data = structuredClone(directory); data.data.items[0].role = 'future-role'; useDirectory(data);
  await click('teamConsoleTabs', 'Members'); await click('teamConsolePanel', 'Edit access');
  await click('teamConsolePanel', 'Save permissions'); assert.equal(lastApply(), undefined);
  assert.match(node('teamConsolePanel').textContent, /existing policy cannot be edited safely/);
  chooseDuty('developer', true); await click('teamConsolePanel', 'Save permissions');
  assert.equal(lastApply().plan.role, 'developer'); assert.deepEqual(lastApply().plan.business_roles, ['developer']);
});

for (const data of [null, {}, { outcome: 'future' }, { outcome: 'committed' }, { outcome: 'unknown', receipt: {} }])
  test(`unrecognized save outcome ${JSON.stringify(data)} cannot expose a credential or enable retry`, async () => {
    const previous = handler;
    handler = (op, payload) => op === 'apply' ? { ok: false, error: { code: 'BridgeUnreachable' } }
      : op === 'outcome' ? { ok: true, data } : previous(op, payload);
    await click('teamConsoleTabs', 'Members'); await click('teamConsolePanel', 'Generate connection instructions');
    await click('teamConsolePanel', 'Check whether it was saved');
    assert.equal(find('teamConsolePanel', 'TEXTAREA'), null); assert.equal(find('teamConsolePanel', 'BUTTON', 'Try saving again'), null);
    assert.ok(find('teamConsolePanel', 'BUTTON', 'Check whether it was saved'));
  });

for (const field of ['request_id', 'subject_actor_id', 'subject_client_id', 'admin_actor_id', 'admin_client_id', 'before_digest', 'plan_digest'])
  test(`a mismatched ${field} cannot confirm a save`, async () => {
    const previous = handler;
    handler = (op, payload) => {
      if (op !== 'apply') return previous(op, payload);
      const response = savedResponse(payload); response.data.receipt[field] = 'other'; return response;
    };
    await click('teamConsoleTabs', 'Members'); await click('teamConsolePanel', 'Generate connection instructions');
    assert.equal(find('teamConsolePanel', 'TEXTAREA'), null); assert.ok(find('teamConsolePanel', 'BUTTON', 'Check whether it was saved'));
  });

test('malformed preview never reaches apply and a missing committed credential remains unknown', async () => {
  const previous = handler;
  handler = (op, payload) => op === 'preview' ? { ok: true, data: {} } : previous(op, payload);
  await click('teamConsoleTabs', 'Members'); await click('teamConsolePanel', 'Generate connection instructions');
  assert.equal(lastApply(), undefined); assert.equal(find('teamConsolePanel', 'TEXTAREA'), null);
  handler = (op, payload) => {
    if (op !== 'apply') return previous(op, payload);
    const response = savedResponse(payload); delete response.data.receipt.desired.credential; return response;
  };
  await click('teamConsolePanel', 'Generate connection instructions');
  assert.equal(find('teamConsolePanel', 'TEXTAREA'), null); assert.ok(find('teamConsolePanel', 'BUTTON', 'Check whether it was saved'));
});

for (const patch of [
  { actor_id: 'other-admin' }, { client_id: 'other-client' }, { can_read_project_audit: false },
  { role: 'maintainer' }, { membership_action_ceiling: ['work.read'] },
]) test(`identity/permission change ${JSON.stringify(patch)} clears protected handoff`, async () => {
  await click('teamConsoleTabs', 'Members'); await click('teamConsolePanel', 'Generate connection instructions');
  assert.ok(find('teamConsolePanel', 'TEXTAREA'));
  ui.setContext({ ...context, identity: { ...context.identity, ...patch } });
  assert.equal(find('teamConsolePanel', 'TEXTAREA'), null);
});

test('a stable refresh retains a committed handoff regardless of identity key order', async () => {
  await click('teamConsoleTabs', 'Members'); await click('teamConsolePanel', 'Generate connection instructions');
  const text = find('teamConsolePanel', 'TEXTAREA').value;
  ui.setContext({ ...context, identity: Object.fromEntries(Object.entries(context.identity).reverse()) });
  assert.equal(find('teamConsolePanel', 'TEXTAREA').value, text);
});

test('audit permission revocation discards a late project-wide activity response', async () => {
  const previous = handler; let resolve, ready;
  const started = new Promise(r => { ready = r; });
  handler = (op, payload, url) => op === null ? new Promise(r => { resolve = r; ready(); }) : previous(op, payload, url);
  const loading = click('teamConsoleTabs', 'Activity'); await started;
  ui.setContext({ ...context, identity: { ...context.identity, can_read_project_audit: false } });
  resolve({ ok: true, data: { items: [{ actor_id: 'private-member', action: 'private-action' }] } }); await loading;
  assert.ok(!node('teamConsolePanel').textContent.includes('private-member'));
  assert.ok(!node('teamConsolePanel').textContent.includes('private-action'));
});

test('membership ceiling is localized as a limit and is separate from duties', async () => {
  const data = structuredClone(directory);
  Object.assign(data.data.items[0], { business_roles: ['administrator'], membership_action_ceiling: ['work.read', 'access.manage_project', 'audit.read_project'] });
  useDirectory(data); await click('teamConsoleTabs', 'Members');
  assert.ok(find('teamConsolePanel', 'SUMMARY', 'Membership permission limits'));
  assert.match(node('teamConsolePanel').textContent, /not permission to perform every action/);
  assert.match(node('teamConsolePanel').textContent, /Read project work/); assert.match(node('teamConsolePanel').textContent, /Administrator/);
});


test('refresh verification hides a committed handoff and restores it only for the same permissions', async () => {
  await click('teamConsoleTabs', 'Members'); await click('teamConsolePanel', 'Generate connection instructions');
  const text = find('teamConsolePanel', 'TEXTAREA').value;
  ui.suspend(); assert.equal(find('teamConsolePanel', 'TEXTAREA'), null); assert.equal(node('teamConsoleTabs').hidden, true);
  ui.setContext(context); assert.equal(find('teamConsolePanel', 'TEXTAREA').value, text);
  ui.suspend(); ui.setContext({ ...context, identity: { ...context.identity, can_read_project_audit: false } });
  assert.equal(find('teamConsolePanel', 'TEXTAREA'), null);
});

test('a refresh during an uncertain apply retains the original request for inspection', async () => {
  await click('teamConsoleTabs', 'Members');
  const previous = handler; let resolve, ready;
  const started = new Promise(r => { ready = r; });
  handler = (op, payload) => op === 'apply' ? new Promise(r => { resolve = r; ready(); }) : previous(op, payload);
  const saving = click('teamConsolePanel', 'Generate connection instructions'); await started;
  ui.suspend(); ui.setContext(context); resolve(savedResponse()); await saving;
  assert.equal(find('teamConsolePanel', 'TEXTAREA'), null);
  assert.ok(find('teamConsolePanel', 'BUTTON', 'Check whether it was saved'));
  handler = async () => ({ ok: true, data: { outcome: 'committed', receipt: savedResponse().data.receipt } });
  await click('teamConsolePanel', 'Check whether it was saved');
  assert.ok(find('teamConsolePanel', 'TEXTAREA'));
  assert.equal(calls.filter(c => c.body?.operation === 'apply').length, 1);
  assert.equal(calls.at(-1).body.payload.request_id, lastApply().request_id);
});
