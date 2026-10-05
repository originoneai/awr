'use strict';
const { test, beforeEach } = require('node:test');
const assert = require('node:assert/strict');
const { install } = require('./fixtures/dom-stub');
const { model, visualStatus } = require('../public/team-network');
const { createTeamWeb } = require('../public/team-web');
const i18n = require('../public/i18n');
const node = id => document.getElementById(id);
let ui;
beforeEach(() => {
  install(); i18n.setLocale('en'); ui = createTeamWeb({ i18n, $: node });
});

test('network renders only explicit visible edges and keeps empty authorized streams', () => {
  const works = [
    { key: 'A', workstream_id: 'one' },
    { key: 'B', workstream_id: 'one', depends_on: [{ key: 'A', visible: true }, { key: 'A', visible: true }] },
    { key: 'C', workstream_id: 'two', depends_on: [{ key: 'B', visible: true }, { key: 'HIDDEN', visible: false }, { key: 'MISSING', visible: true }] },
    { key: 'D', workstream_id: 'two', depends_on: [{ key: 'A' }] },
  ];
  const graph = model(works, [{ id: 'one', title: 'Backend' }, { id: 'two' }, { id: 'empty' }], 'Unspecified');
  assert.deepEqual(graph.edges, [['A', 'B', 'local'], ['B', 'C', 'cross']]);
  assert.equal(graph.lanes.length, 3);
  assert.equal(graph.lanes[2].works.length, 0);
  assert.equal(visualStatus({}), 'unknown');
  assert.equal(visualStatus({ status: 'unclaimed' }), 'unknown');
});

test('visible prerequisites precede consumers; cycles remain finite and data is unchanged', () => {
  const works = [
    { key: 'consumer', depends_on: [{ key: 'upstream', visible: true }] },
    { key: 'upstream' },
    { key: 'cycle-a', depends_on: [{ key: 'cycle-b', visible: true }] },
    { key: 'cycle-b', depends_on: [{ key: 'cycle-a', visible: true }] },
  ];
  const original = JSON.stringify(works);
  const graph = model(works, [], 'Unspecified');
  assert.deepEqual(graph.lanes[0].works.map(w => w.key), ['upstream', 'consumer', 'cycle-a', 'cycle-b']);
  assert.equal(JSON.stringify(works), original);
  assert.equal(graph.edges.length, 3);
});

function live(handler, count = 4) {
  const works = Array.from({ length: count }, (_, i) => ({ key: 'W-' + i, workstream_id: 'stream', contract_hash: 'current' }));
  global.fetch = async (url) => ({ ok: true, json: async () => {
    if (url.includes('/projects?')) return { ok: true, projects: [{ key: 'project' }], session: { session_id: 'test' } };
    if (url.includes('/overview?')) return { ok: true, works, workstreams: [{ id: 'stream', title: 'Stream' }], interaction_mode: 'mcp' };
    return handler(url, works);
  } });
}
const detail = work => ({ ok: true, work: { ...work, detail_loaded: true, status: null,
  acceptance: ['Verify the contract'], depends_on: [], context_complete: true } });

test('background refresh displays changed facts while retaining selection and layout', async () => {
  let revision = 1;
  live(async (url, works) => {
    const work = works.find(w => w.key === new URL(url, 'http://test').searchParams.get('work'));
    return { ok: true, work: { ...detail(work).work, next_step: 'Step ' + revision,
      observation_available: true, claimant: 'Member', session_id: 'session', attention: revision === 2 ? 'lease_expired' : null } };
  });
  await ui.refresh();
  await ui._selectWork('W-2');
  ui.state.layout = 'list';
  revision = 2;
  await ui._refreshProgress();
  assert.equal(ui.state.selected, 'W-2');
  assert.equal(ui.state.layout, 'list');
  assert.equal(ui._selectedWork().next_step, 'Step 2');
  assert.match(node('teamDetail').textContent, /Claim expired/);
  assert.match(node('teamDetail').textContent, /Member/);
  assert.match(node('teamDetail').textContent, /No recorded usage/);
  assert.equal(visualStatus(ui._selectedWork()), 'blocked');
});

test('background refresh does not disturb member forms, overlap, or restore revoked data', async () => {
  let reads = 0, release;
  live(async (url, works) => { reads++; return detail(works[0]); }, 1);
  await ui.refresh();
  node('teamWorkspaceGrid').hidden = true;
  await ui._refreshProgress();
  assert.equal(reads, 1);
  node('teamWorkspaceGrid').hidden = false;
  const gate = new Promise(resolve => { release = resolve; });
  let calls = 0;
  global.fetch = async () => { calls++; await gate; return { ok: true, json: async () => ({ ok: false, error: { code: 'Forbidden' } }) }; };
  const first = ui._refreshProgress();
  await ui._refreshProgress();
  assert.equal(calls, 1);
  release(); await first;
  assert.equal(ui.state.works.length, 0);
  assert.equal(ui.state.selected, null);
});

test('failed background refresh keeps the last snapshot visibly stale', async () => {
  live(async (_url, works) => detail(works[0]), 1);
  await ui.refresh();
  const fetched = ui.state.lastRefreshedAt;
  global.fetch = async () => { throw new Error('disconnected'); };
  await ui._refreshProgress();
  assert.equal(ui.state.lastRefreshedAt, fetched);
  assert.equal(ui.state.works.length, 1);
  assert.match(node('teamOverview').textContent, /Refresh failed/);
});

test('one failed detail read does not prevent healthy peers from receiving new reports', async () => {
  let revision = 1;
  live((url, works) => {
    const item = works.find(w => w.key === new URL(url, 'http://test').searchParams.get('work'));
    if (revision === 2 && item.key === 'W-1') return { ok: false, error: { code: 'SourceChanged', message: 'Refresh the contract' } };
    return { ok: true, work: { ...detail(item).work, next_step: 'Revision ' + revision } };
  }, 2);
  await ui.refresh();
  revision = 2;
  await ui._refreshProgress();
  assert.equal(ui.state.works[0].next_step, 'Revision 2');
  assert.equal(ui.state.error, null);
  assert.equal(ui.state.works[1].detail_loaded, undefined);
  assert.equal(ui.state.works[1].detail_error.code, 'SourceChanged');
  assert.match(node('teamOverview').textContent, /Some task details could not be refreshed/);
  await ui._selectWork('W-1');
  assert.match(node('teamDetail').textContent, /does not mean the Agent has not reported/);
  assert.doesNotMatch(node('teamDetail').textContent, /Not reported|No recorded usage/);
});

test('an unread task is not presented as an unreported Agent or missing usage', async () => {
  live((url, works) => detail(works.find(w => w.key === new URL(url, 'http://test').searchParams.get('work'))), 65);
  await ui.refresh();
  ui.state.selected = 'W-64';
  ui.render();
  assert.match(node('teamDetail').textContent, /Details have not been read/);
  assert.doesNotMatch(node('teamDetail').textContent, /Not reported|No recorded usage|No execution recorded/);
});

test('foreground selection hydrates its own object when sharing a background detail request', async () => {
  live((url, works) => detail(works.find(w => w.key === new URL(url, 'http://test').searchParams.get('work'))), 65);
  await ui.refresh();
  ui.state.selected = 'W-64';
  let release, started;
  const ready = new Promise(resolve => { started = resolve; });
  const gate = new Promise(resolve => { release = resolve; });
  let selectedReads = 0;
  live(async (url, works) => {
    const item = works.find(w => w.key === new URL(url, 'http://test').searchParams.get('work'));
    if (item.key === 'W-64') { selectedReads++; started(); await gate; }
    return { ok: true, work: { ...detail(item).work, next_step: 'Fresh foreground report' } };
  }, 65);
  const polling = ui._refreshProgress();
  await ready;
  const foreground = ui._selectWork('W-64');
  release();
  await Promise.all([polling, foreground]);
  assert.equal(selectedReads, 1);
  assert.equal(ui._selectedWork().detail_loaded, true);
  assert.equal(ui._selectedWork().next_step, 'Fresh foreground report');
  assert.match(node('teamDetail').textContent, /Fresh foreground report/);
  assert.equal(ui.state.detailLoading, false);
});

test('a same-key detail from a different contract stays unread and can recover on a fresh read', async () => {
  let stale = true;
  live((_url, works) => ({ ok: true, work: { ...detail(works[0]).work,
    contract_hash: stale ? 'older-contract' : 'current', next_step: 'Verified contract report' } }), 1);
  await ui.refresh();
  assert.ok(!ui._selectedWork().detail_loaded);
  assert.equal(ui._selectedWork().detail_error.code, 'InvalidResponse');
  assert.doesNotMatch(node('teamDetail').textContent, /Verified contract report/);
  stale = false;
  await node('teamDetail').find(el => el.tagName === 'BUTTON' && el.textContent === 'Retry reading details').click();
  assert.equal(ui._selectedWork().detail_loaded, true);
  assert.equal(ui._selectedWork().detail_error, undefined);
  assert.match(node('teamDetail').textContent, /Verified contract report/);
});

test('retrying a changed source first reads the current contract instead of repeating the stale selector', async () => {
  let changed = false;
  let overviewReads = 0;
  global.fetch = async url => ({ ok: true, json: async () => {
    if (url.includes('/projects?')) return { ok: true, projects: [{ key: 'project' }], session: { session_id: 'test' } };
    if (url.includes('/overview?')) {
      overviewReads++;
      return { ok: true, interaction_mode: 'mcp', workstreams: [{ id: 'stream' }],
        works: [{ key: 'W-0', workstream_id: 'stream', contract_hash: changed ? 'new' : 'old' }] };
    }
    const hash = new URL(url, 'http://test').searchParams.get('contract');
    if (hash !== 'new') return { ok: false, error: { code: 'SourceChanged' } };
    return detail({ key: 'W-0', workstream_id: 'stream', contract_hash: 'new' });
  } });
  await ui.refresh();
  assert.equal(ui._selectedWork().detail_error.code, 'SourceChanged');
  changed = true;
  await node('teamDetail').find(el => el.tagName === 'BUTTON' && el.textContent === 'Retry reading details').click();
  assert.equal(overviewReads, 2);
  assert.equal(ui._selectedWork().contract_hash, 'new');
  assert.equal(ui._selectedWork().detail_loaded, true);
  assert.equal(ui._selectedWork().detail_error, undefined);
});

for (const locale of ['en', 'zh-CN']) {
  test(`unread participants stay unknown in both graph and list in ${locale}`, async () => {
    i18n.setLocale(locale);
    live(() => ({ ok: false, error: { code: 'BridgeUnreachable' } }), 1);
    await ui.refresh();
    for (const layout of ['cards', 'list']) {
      ui.state.layout = layout;
      ui.render();
      const item = node('teamOverview').find(el => el.dataset.key === 'W-0');
      assert.ok(item);
      assert.doesNotMatch(item.textContent, /Owner not reported|负责人未报告/);
      assert.match(item.textContent, /Not inspected|待读取/);
    }
  });

  test(`detail read failure is localized and distinct from missing reports in ${locale}`, async () => {
    i18n.setLocale(locale);
    live(() => ({ ok: false, error: { code: 'BridgeUnreachable' } }), 1);
    await ui.refresh();
    assert.match(node('teamDetail').textContent, /does not mean the Agent has not reported|不代表 Agent 未报告/);
    assert.doesNotMatch(node('teamDetail').textContent, /No recorded usage|无用量记录|ui\.team_/);
    assert.equal(node('teamDetail').find(el => el.tagName === 'DL'), null);
    assert.ok(node('teamDetail').find(el => el.tagName === 'BUTTON' && /Retry reading details|重新读取详情/.test(el.textContent)));
  });
}

test('editing that starts during a poll prevents replacement of the visible snapshot', async () => {
  live(async (_url, works) => detail(works[0]), 1);
  await ui.refresh();
  const before = ui.state.works, timestamp = ui.state.lastRefreshedAt;
  let release;
  const gate = new Promise(resolve => { release = resolve; });
  live(async (_url, works) => { await gate; return detail(works[0]); }, 1);
  const refreshing = ui._refreshProgress();
  await new Promise(resolve => setImmediate(resolve));
  document.activeElement = { tagName: 'INPUT', value: 'Unsaved member' };
  release(); await refreshing;
  assert.equal(ui.state.works, before);
  assert.equal(ui.state.lastRefreshedAt, timestamp);
  assert.equal(document.activeElement.value, 'Unsaved member');
  document.activeElement = null;
});

test('selecting outside the polling batch retains foreground detail when the poll finishes', async () => {
  live(async (url, works) => detail(works.find(w => w.key === new URL(url, 'http://test').searchParams.get('work'))), 65);
  await ui.refresh();
  let release;
  const gate = new Promise(resolve => { release = resolve; });
  live(async (url, works) => {
    const key = new URL(url, 'http://test').searchParams.get('work');
    if (key !== 'W-64') await gate;
    return { ok: true, work: { ...detail(works.find(w => w.key === key)).work, next_step: 'Current ' + key } };
  }, 65);
  const polling = ui._refreshProgress();
  await new Promise(resolve => setImmediate(resolve));
  await ui._selectWork('W-64');
  release(); await polling;
  assert.equal(ui.state.selected, 'W-64');
  assert.equal(ui._selectedWork().detail_loaded, true);
  assert.equal(ui._selectedWork().next_step, 'Current W-64');
  assert.equal(ui.state.detailLoading, false);
  await ui._refreshProgress();
  assert.equal(ui._selectedWork().next_step, 'Current W-64');
});

test('graph hydration bounds concurrency and batches without inventing runtime or progress', async () => {
  let active = 0, maximum = 0, reads = 0;
  live(async (url, works) => {
    reads++; active++; maximum = Math.max(maximum, active);
    await new Promise(resolve => setImmediate(resolve)); active--;
    return detail(works.find(w => w.key === new URL(url, 'http://test').searchParams.get('work')));
  }, 65);
  await ui.refresh();
  assert.equal(reads, 60); assert.equal(maximum, 4);
  assert.equal(ui.state.works.filter(w => w.detail_loaded).length, 60);
  assert.match(node('teamOverview').textContent, /Details read: 60 \/ 65/);
  assert.doesNotMatch(node('teamOverview').textContent, /100%|0%/);
  assert.match(node('teamDetail').textContent, /No execution recorded/);
  assert.match(node('teamDetail').textContent, /Not reported/);
  await ui._loadGraphDetails();
  assert.equal(reads, 65);
});

test('logout invalidates pending hydration and never restores protected data', async () => {
  let release, started;
  const ready = new Promise(resolve => { started = resolve; });
  const gate = new Promise(resolve => { release = resolve; });
  live(async (url, works) => {
    if (url.endsWith('/logout')) return { ok: true };
    started(); await gate; return detail(works[0]);
  });
  const refreshing = ui.refresh(); await ready;
  await node('teamAuth').find(el => el.tagName === 'BUTTON' && el.textContent === 'Log out').click();
  release(); await refreshing;
  assert.equal(ui.state.session, null);
  assert.deepEqual(ui.state.works, []);
  assert.deepEqual(ui.state.streams, []);
  assert.equal(node('rawTeamBody').textContent, '');
});

test('an expired hydration response invalidates other concurrent responses', async () => {
  live(async (url, works) => {
    if (url.includes('work=W-0')) return { ok: false, error: { code: 'SessionExpired', message: 'expired' } };
    await new Promise(resolve => setImmediate(resolve)); return detail(works[1]);
  });
  await ui.refresh();
  assert.equal(ui.state.session, null); assert.deepEqual(ui.state.works, []);
  assert.match(node('teamAuth').textContent, /SessionExpired/);
});

test('denied and mismatched details remain unread with visible errors', async () => {
  live(() => ({ ok: false, error: { code: 'SourceChanged', message: 'refresh' } }), 1);
  await ui.refresh();
  assert.ok(!ui.state.works[0].detail_loaded);
  assert.equal(ui.state.works[0].detail_error.code, 'SourceChanged');
  assert.match(node('teamDetail').textContent, /does not mean the Agent has not reported/);
  live((_url, works) => detail({ ...works[0], workstream_id: 'wrong' }), 1);
  await ui.refresh();
  assert.ok(!ui.state.works[0].detail_loaded);
  assert.equal(ui.state.works[0].detail_error.code, 'InvalidResponse');
  assert.match(node('teamDetail').textContent, /does not mean the Agent has not reported/);
});

test('project content is text and unknown fields never become synthetic claims', async () => {
  const title = '<img src=x onerror=alert(1)>';
  ui.state.projects = [{ key: 'project', title }]; ui.state.projectKey = 'project';
  ui.state.works = [{ key: 'W', title }]; ui.state.selected = 'W'; ui.render();
  const card = node('teamOverview').find(el => el.getAttribute('role') === 'button');
  assert.equal(card.getAttribute('aria-pressed'), 'true');
  assert.match(card.textContent, /<img src=x/);
  assert.equal(card.find(el => el.tagName === 'IMG'), null);
  assert.doesNotMatch(node('teamOverview').textContent, /Synthetic demo data/);
  assert.doesNotMatch(node('teamDetail').textContent, /Passed|Merged|GPT-/);
});

for (const locale of ['en', 'zh-CN']) {
  test(`structured reports retain provenance, missing reasons and historical status in ${locale}`, async () => {
    i18n.setLocale(locale);
    const escaped = '<img src=x onerror=alert(1)>';
    live((_url, works) => ({ ok: true, work: { ...detail(works[0]).work,
      last_participant: 'Contributor', claimant: null, agent: 'Example Agent', model: 'example-model',
      progress_report: { phase: 'testing', summary: escaped, completed: ['Atomic writes'], blockers: [],
        tests: [{ name: 'Durability', outcome: 'passed', reference: 'reports/check.txt' }],
        artifacts: [{ label: 'Source', reference: 'javascript:alert(1)' }], stale: true, reported_at_unix_ms: 1000 },
      usage: { input_tokens: 0, output_tokens: null, cached_input_tokens: 0, coverage: 'partial',
        observed_at_unix_ms: 900, reported_at_unix_ms: 1000, source: 'native_host_event', source_ref: 'logs/session#3', counter_id: 'one' },
      guidance: { code: 'inspect_delivery' }, next_step: 'Wrong stale action',
      checkpoint: { next_action: 'Old handoff', created_at_unix_ms: 1000, contract_matches_current: true },
      execution: { state: 'unknown', receipt_missing: 'permission_restricted' },
      missing: { execution_receipt: 'permission_restricted' },
    } }), 1);
    await ui.refresh();
    const view = node('teamDetail'), text = view.textContent;
    assert.match(text, /Example Agent|example-model/);
    assert.match(text, /Atomic writes/); assert.match(text, /reports\/check.txt/);
    assert.match(text, /logs\/session#3/); assert.match(text, /Input tokens0|输入 Token0/);
    assert.match(text, /Historical report|历史报告/);
    assert.match(text, /not include this report|当前权限无法查看/);
    assert.match(text, /Not a task total or a bill|不是本任务总消耗或账单/);
    assert.doesNotMatch(text, /Wrong stale action|\[object Object\]|feedback\.|ui\.network_|\{loaded\}|\{total\}/);
    assert.match(text, /CoveragePartial|采集覆盖部分采集/);
    const history = view.find(el => el.tagName === 'DETAILS' && el.textContent.includes('Old handoff'));
    assert.ok(history); assert.equal(history.getAttribute('open'), null);
    assert.equal(view.find(el => el.tagName === 'IMG'), null);
    assert.equal(view.find(el => el.getAttribute('href') === 'javascript:alert(1)'), null);
    assert.match(node('teamOverview').textContent, /Last participant: Contributor|最近参与：Contributor/);
    assert.match(node('teamOverview').textContent, /Page refreshed|页面刷新于/);
  });
}

test('unsupported collection and missing reports stay distinct', async () => {
  live((_url, works) => ({ ok: true, work: { ...detail(works[0]).work,
    missing: { model: 'client_collection_unsupported', progress: 'not_reported_by_client', usage: 'client_capability_unknown' },
  } }), 1);
  await ui.refresh();
  const text = node('teamDetail').textContent;
  assert.match(text, /does not support collecting/);
  assert.match(text, /Supported by the client, but not reported/);
  assert.match(text, /has not declared whether it can report/);
  assert.doesNotMatch(text, /0 tokens|\$0/);
});
