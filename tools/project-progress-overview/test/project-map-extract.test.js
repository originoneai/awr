/**
 * Snapshot extraction from the official commands: data shaping, adaptive limits, same-revision consistency, retries,
 * paging, feature detection and the commands that are (and are not) used. A fake awr answers from a snapshot using the
 * JSON shapes of the real CLI, so extracting from it must reproduce that snapshot.
 *
 * Run: node --test test/project-map-extract.test.js
 */

'use strict';

const { test } = require('node:test');
const assert = require('node:assert/strict');
const h = require('./fixtures/project-map/helpers.js');
const { createFakeCli } = require('./fixtures/project-map/fake-cli.js');
const commands = require('../public/project-map/commands.js');
const extractLib = require('../public/project-map/extract.js');
const snapshotLib = require('../public/project-map/snapshot.js');

const { AwrCallError, extractSnapshot } = extractLib;
const NOW = '2026-02-03T04:05:06Z';
const noWait = async () => {};
const config = () => h.readJson('config.json');
const source = () => h.readJson('states.json');
const extract = (cli, extra = {}) => extractSnapshot({ call: cli.call, config: config(), now: NOW, wait: noWait, ...extra });

/**
 * Everything the snapshot carries except the extraction clock, the basis bookkeeping and the wording of the
 * unavailable-field reasons (the fixtures were written by another implementation; the field names must still agree).
 */
function content(snapshot) {
  const { generated_at: _g, fingerprint: _f, basis: _b, unavailable, ...rest } = snapshot;
  return { ...rest, unavailable: unavailable.map((u) => u.field) };
}

test('extracting from the official JSON reproduces the snapshot it was served from', async () => {
  const original = source();
  const got = await extract(createFakeCli(original));
  assert.deepEqual(content(got), content(original));
  assert.equal(got.generated_at, NOW);
  assert.equal(got.fingerprint, snapshotLib.fingerprint(got));
  assert.deepEqual(got.basis.milestones_resolved, ['M1', 'M2']);
  assert.deepEqual(got.basis.work_graph, original.basis.work_graph);
  assert.deepEqual(got.basis.nav, original.basis.nav);
  assert.deepEqual(got.basis.sessions, original.basis.sessions);
  assert.equal(got.basis.events.source, 'event_history');
});

test('the work graph limit follows BudgetExceeded and stops at the service cap', async () => {
  const cli = createFakeCli(source());
  await extract(cli);
  const limits = cli.calls.filter((c) => c[0] === 'work').map((c) => c[c.indexOf('--limit') + 1]);
  assert.deepEqual(limits, ['100'], 'the demo project has 20 items: the default limit is enough');

  // a project of 250 items needs the second try with the exact count
  const big = source();
  const template = big.nodes[0];
  big.nodes = Array.from({ length: 250 }, (_, i) => ({ ...template, key: `BIG-${String(i).padStart(4, '0')}`, id: `id-${i}`, claims: [], diagnostics: [], last_event_at: null }));
  big.edges = [];
  big.sessions = [];
  big.basis.work_graph.node_count = 250;
  big.basis.work_graph.edge_count = 0;
  const bigCli = createFakeCli(big);
  const snapshot = await extractSnapshot({ call: bigCli.call, config: {}, now: NOW, wait: noWait });
  assert.equal(snapshot.nodes.length, 250);
  assert.deepEqual(bigCli.calls.filter((c) => c[0] === 'work').map((c) => c[c.indexOf('--limit') + 1]), ['100', '250']);

  const huge = createFakeCli(big);
  const tooBig = async (name, params) => {
    if (name === 'workGraph') throw new AwrCallError('BudgetExceeded', 'context budget exceeded', { required: 1500, budget: params.limit });
    return huge.call(name, params);
  };
  await assert.rejects(extractSnapshot({ call: tooBig, config: {}, now: NOW, wait: noWait }), (e) => e.code === 'ProjectTooLarge' && /1500/.test(e.detail));
});

test('only official read commands are used, each exactly as the shared definitions build them', async () => {
  const cli = createFakeCli(source());
  await extract(cli);
  const used = new Set(cli.calls.map((c) => c.slice(0, 2).join(' ')));
  assert.deepEqual([...used].sort(), ['doctor', 'event history', 'nav --cached', 'search --type', 'session list', 'work graph']);
  // no command writes: none of the verbs that change state ever appears
  for (const c of cli.calls) assert.ok(!c.some((a) => ['reindex', 'complete', 'claim', 'append', 'start', 'end', 'progress', 'block', 'cancel'].includes(a)), c.join(' '));
  assert.deepEqual(commands.argvFor('workGraph', { limit: 250 }), ['work', 'graph', '--limit', '250']);
  assert.deepEqual(commands.argvFor('workGraph', { limit: 10, cached: true }), ['work', 'graph', '--limit', '10', '--cached']);
  assert.deepEqual(commands.argvFor('nav', { milestone: 'M1' }), ['nav', '--cached', '--milestone', 'M1']);
  assert.deepEqual(commands.argvFor('events', { limit: 1000, through: 7, cursor: { event_id: 'E' } }), ['event', 'history', '--limit', '1000', '--through-revision', '7', '--cursor', '{"event_id":"E"}']);
  assert.throws(() => commands.argvFor('workGraph', { limit: 1001 }), /limit/);
  assert.throws(() => commands.argvFor('reindex', {}), /unknown project-map command/);
});

test('every answer must belong to the work graph revision: a drifting project is retried, then refused', async () => {
  // the revision moves once during the first run, then stays put
  const original = source();
  let moved = false;
  const cli = createFakeCli(original, { revision: (n) => { if (n === 3 && !moved) { moved = true; return original.project.revision + 1; } return original.project.revision; } });
  const got = await extract(cli);
  assert.equal(got.project.revision, original.project.revision);
  assert.equal(cli.calls.filter((c) => c[0] === 'work').length, 2, 'the whole run was repeated');

  const never = createFakeCli(original, { revision: (n) => original.project.revision + (n % 2) });
  await assert.rejects(extract(never, { attempts: 3 }), (e) => e.code === 'RevisionDrift' && /3 attempts/.test(e.detail));
});

test('transient command failures are retried a few times, other errors are not', async () => {
  const cli = createFakeCli(source());
  let failures = 0;
  const flaky = (name, params) => (name === 'doctor' && failures++ < 2 ? Promise.reject(new AwrCallError('RevisionConflict', 'the project moved')) : cli.call(name, params));
  assert.equal((await extractSnapshot({ call: flaky, config: config(), now: NOW, wait: noWait })).doctor.findings.length, 3);
  assert.equal(failures, 3);

  let tries = 0;
  const broken = (name) => { tries += 1; return Promise.reject(new AwrCallError('NotFound', 'no such project')); };
  await assert.rejects(extractSnapshot({ call: broken, config: {}, now: NOW, wait: noWait }), (e) => e.code === 'NotFound');
  assert.equal(tries, 1);

  const alwaysBusy = () => Promise.reject(new AwrCallError('BridgeBusy', 'busy'));
  await assert.rejects(extractSnapshot({ call: alwaysBusy, config: {}, now: NOW, wait: noWait }), (e) => e.code === 'BridgeBusy');
});

test('event history is paged under a fixed revision and the newest activity per work item wins', async () => {
  const original = source();
  const cli = createFakeCli(original, { pageSize: 3 });
  const got = await extract(cli);
  const pages = cli.calls.filter((c) => c[0] === 'event');
  assert.ok(pages.length > 1, 'small pages force the cursor path');
  assert.ok(pages.every((c) => c.includes('--through-revision') && c[c.indexOf('--through-revision') + 1] === String(original.project.revision)));
  assert.ok(pages.slice(1).every((c) => c.includes('--cursor')));
  assert.deepEqual(got.nodes.map((n) => n.last_event_at), original.nodes.map((n) => n.last_event_at));
  assert.deepEqual(got.sessions.map((x) => x.last_event_at), original.sessions.map((x) => x.last_event_at));
  assert.equal(got.basis.events.pages, pages.length);
  assert.equal(got.basis.events.count, original.nodes.filter((n) => n.last_event_at).length + original.sessions.filter((x) => x.last_event_at).length);

  // a history that repeats an event is refused instead of looping
  const loop = async (name, params) => {
    const value = await cli.call(name, params);
    if (name === 'events') return { ...value, events: value.events.concat(value.events.slice(0, 1)) };
    return value;
  };
  await assert.rejects(extractSnapshot({ call: loop, config: config(), now: NOW, wait: noWait }), (e) => e.code === 'CursorLoop');
});

test('newer awr versions give the activity on the graph nodes and the event history is skipped', async () => {
  const original = source();
  const cli = createFakeCli(original, { nodeActivity: true });
  const got = await extract(cli);
  assert.equal(cli.calls.filter((c) => c[0] === 'event').length, 0);
  assert.deepEqual(got.nodes.map((n) => n.last_event_at), original.nodes.map((n) => n.last_event_at));
  assert.equal(got.basis.events.source, 'work_graph');
  assert.equal(got.basis.events.pages, 0);
  // the answer is the same as the one built from the event history
  assert.deepEqual(content({ ...got, sessions: [] }).nodes, content({ ...(await extract(createFakeCli(original))), sessions: [] }).nodes);
});

test('recorded-state reads need work graph --cached and say so when awr cannot do it', async () => {
  const original = source();
  const modern = createFakeCli(original);
  const got = await extract(modern, { cached: true });
  assert.ok(modern.calls.some((c) => c[0] === 'work' && c.includes('--cached')));
  assert.equal(got.nodes.length, original.nodes.length);
  await assert.rejects(extract(createFakeCli(original, { cachedSupported: false }), { cached: true }), (e) => e.code === 'CachedUnsupported' && /--cached/.test(e.detail));
  // without the flag the refreshing read is used and works on every version
  const old = createFakeCli(original, { cachedSupported: false });
  await extract(old);
  assert.ok(old.calls.every((c) => !(c[0] === 'work' && c.includes('--cached'))));
});

test('milestones are resolved only for the configured ones; the rest stay unavailable', async () => {
  const original = source();
  const cli = createFakeCli(original);
  const got = await extract(cli);
  const asked = cli.calls.filter((c) => c[0] === 'nav' && c.includes('--milestone')).map((c) => c[c.indexOf('--milestone') + 1]).sort();
  assert.deepEqual(asked, ['M1', 'M2']);
  assert.deepEqual(got.nodes.map((n) => n.milestone), original.nodes.map((n) => n.milestone));

  const noConfig = createFakeCli(original);
  const plain = await extractSnapshot({ call: noConfig.call, config: undefined, now: NOW, wait: noWait });
  assert.equal(noConfig.calls.filter((c) => c.includes('--milestone')).length, 0);
  assert.ok(plain.nodes.every((n) => n.milestone === null));
  assert.deepEqual(plain.unavailable.map((u) => u.field), snapshotLib.UNAVAILABLE.map((u) => u.field));

  // an item that two configured milestones both claim is a contradiction, not something to pick from
  const clash = async (name, params) => {
    const value = await cli.call(name, params);
    return name === 'nav' && params.milestone === 'M2' ? { ...value, scope: { selection: ['DEMO-A-001'] } } : value;
  };
  await assert.rejects(extractSnapshot({ call: clash, config: config(), now: NOW, wait: noWait }), (e) => e.code === 'MilestoneConflict');
});

test('disagreeing answers are refused rather than merged', async () => {
  const cli = createFakeCli(source());
  const withNav = (patch) => async (name, params) => {
    const value = await cli.call(name, params);
    return name === 'nav' && !params.milestone ? patch(value) : value;
  };
  await assert.rejects(extractSnapshot({ call: withNav((v) => ({ ...v, mainline_graph: { ...v.mainline_graph, nodes: v.mainline_graph.nodes.slice(1) } })), config: {}, now: NOW, wait: noWait }), (e) => e.code === 'NodeSetMismatch');
  await assert.rejects(extractSnapshot({ call: withNav((v) => ({ ...v, mainline_graph: { ...v.mainline_graph, nodes: v.mainline_graph.nodes.map((n, i) => (i ? n : { ...n, status: 'blocked' })) } })), config: {}, now: NOW, wait: noWait }), (e) => e.code === 'StatusMismatch');
  const incomplete = async (name, params) => (name === 'workGraph' ? { ...(await cli.call(name, params)), graph_valid: false } : cli.call(name, params));
  await assert.rejects(extractSnapshot({ call: incomplete, config: {}, now: NOW, wait: noWait }), (e) => e.code === 'GraphIncomplete');
});

test('doctor reports findings through a nonzero exit and still counts as an answer', async () => {
  const cli = createFakeCli(source());
  const result = cli.run(['--project', '/p', '--json', 'doctor']);
  assert.equal(result.code, 1);
  const got = await extract(cli);
  assert.equal(got.doctor.ok, false);
  assert.deepEqual(got.doctor.findings.map((f) => f.code), ['active_session', 'expired_claim', 'orphan_session']);
});

test('the snapshot is deterministic and sorted whatever order the answers come in', async () => {
  const original = source();
  const shuffled = JSON.parse(JSON.stringify(original));
  shuffled.nodes.reverse();
  shuffled.edges.reverse();
  shuffled.goals.reverse();
  shuffled.sessions.reverse();
  shuffled.doctor.findings.reverse();
  const a = await extract(createFakeCli(original));
  const b = await extract(createFakeCli(shuffled));
  assert.equal(a.fingerprint, b.fingerprint);
  assert.equal(JSON.stringify(a), JSON.stringify(b));
});
