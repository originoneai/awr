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

async function bundledFixture() {
  const cli = createFakeCli(source(), { nodeActivity: true });
  const bundle = { version: 1, project_revision: source().project.revision,
    basis: { coherent: true, source_refresh_performed: false, source_currentness_verified: false, diagnostics_scope: 'recorded_runtime' }, milestones: {} };
  for (const [name, field] of Object.entries({ workGraph: 'work_graph', nav: 'nav', goals: 'goals', sessions: 'sessions', doctor: 'doctor', events: 'events' })) {
    bundle[field] = await cli.call(name, name === 'workGraph' ? { limit: 1000, cached: true } : {});
  }
  for (const milestone of ['M1', 'M2']) bundle.milestones[milestone] = (await cli.call('nav', { milestone })).scope.selection;
  return bundle;
}

test('one coherent bundle retains the legacy extracted content and observation scope', async () => {
  const bundle = await bundledFixture();
  const got = await extractLib.extractBundledSnapshot({ bundle, config: config(), now: NOW });
  const legacy = await extract(createFakeCli(source(), { nodeActivity: true }));
  assert.deepEqual(content(got), content(legacy));
  assert.deepEqual(got.basis.observation, bundle.basis);
  snapshotLib.validate(got);
});

test('incoherent and mixed-revision bundles are refused before rendering', async () => {
  const bundle = await bundledFixture();
  bundle.nav.project_revision += 1;
  await assert.rejects(extractLib.extractBundledSnapshot({ bundle }), (error) => error.code === 'RevisionDrift');
  bundle.basis.coherent = false;
  await assert.rejects(extractLib.extractBundledSnapshot({ bundle }), (error) => error.code === 'InvalidSnapshot');
});

test('archived work omitted by navigation retains its recorded graph fields', async () => {
  const bundle = await bundledFixture();
  const node = bundle.work_graph.nodes[0];
  node.archived = true;
  node.person_responsibility = 'source-owner';
  node.next_action = 'Retain the archived result';
  bundle.nav.mainline_graph.nodes = bundle.nav.mainline_graph.nodes.filter((item) => item.work_key !== node.key);
  const snapshot = await extractLib.extractBundledSnapshot({ bundle, now: NOW });
  const archived = snapshot.nodes.find((item) => item.key === node.key);
  assert.equal(archived.archived, true);
  assert.equal(archived.owner, 'source-owner');
  assert.equal(archived.next_action, 'Retain the archived result');
  snapshotLib.validate(snapshot);
});

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

  // the failure seen in the wild: required 1002 used to be unreadable (GRAPH_CAP and the store both capped at 1000),
  // which made the adaptive retry end at LimitNotConverging. The exact count must succeed on the second try.
  const over = source();
  const overTemplate = over.nodes[0];
  over.nodes = Array.from({ length: 1002 }, (_, i) => ({ ...overTemplate, key: `N-${i}`, id: `id-n-${i}`, claims: [], diagnostics: [], last_event_at: null }));
  over.edges = [];
  over.sessions = [];
  over.basis.work_graph.node_count = 1002;
  over.basis.work_graph.edge_count = 0;
  const overCli = createFakeCli(over);
  const overSnap = await extractSnapshot({ call: overCli.call, config: {}, now: NOW, wait: noWait });
  assert.equal(overSnap.nodes.length, 1002);
  assert.deepEqual(overCli.calls.filter((c) => c[0] === 'work').map((c) => c[c.indexOf('--limit') + 1]), ['100', '1002']);

  // a project of 1500 items is over the old 1000 cap but under the service cap: one retry with the exact count
  const bigger = createFakeCli(big);
  let biggerFirst = true;
  const biggerLimits = [];
  const fifteenHundred = async (name, params) => {
    if (name === 'workGraph') {
      biggerLimits.push(String(params.limit));
      if (biggerFirst) {
        biggerFirst = false;
        throw new AwrCallError('BudgetExceeded', 'context budget exceeded', { required: 1500, budget: params.limit });
      }
    }
    return bigger.call(name, params);
  };
  await extractSnapshot({ call: fifteenHundred, config: {}, now: NOW, wait: noWait });
  assert.deepEqual(biggerLimits, ['100', '1500']);

  const huge = createFakeCli(big);
  const tooBig = async (name, params) => {
    if (name === 'workGraph') throw new AwrCallError('BudgetExceeded', 'context budget exceeded', { required: 20000, budget: params.limit });
    return huge.call(name, params);
  };
  await assert.rejects(extractSnapshot({ call: tooBig, config: {}, now: NOW, wait: noWait }), (e) => e.code === 'ProjectTooLarge' && /20000/.test(e.detail));
});

test('default swimlanes use already-read groups without plan enumeration or per-work reads', async () => {
  const cli = createFakeCli(source(), { nodeActivity: true });
  const got = await extractSnapshot({ call: cli.call, now: NOW, wait: noWait });
  assert.equal(cli.calls.length, 5);
  assert.ok(cli.calls.every((c) => !c.includes('--milestone') && !c.includes('plan')));
  assert.ok(got.nodes.every((node) => node.milestone === null));
  const model = require('../public/project-map/model.js').loadModel(got);
  assert.equal(model.cfg.explore_lanes.length, 3);
  assert.deepEqual(new Set(model.cfg.explore_lanes.flatMap((lane) => lane.work_keys)), new Set(got.nodes.map((node) => node.key)));
});

test('milestone membership already supplied by the task graph is retained without extra reads', async () => {
  const original = source();
  const cli = createFakeCli(original, { nodeActivity: true });
  const milestones = new Map(original.nodes.map((node) => [node.key, node.milestone]));
  const call = async (name, params) => {
    const value = await cli.call(name, params);
    return name === 'workGraph' ? { ...value, nodes: value.nodes.map((node) => ({ ...node, milestone: milestones.get(node.key) })) } : value;
  };
  const got = await extractSnapshot({ call, now: NOW, wait: noWait });
  assert.deepEqual(got.nodes.map((node) => node.milestone), original.nodes.map((node) => node.milestone));
  assert.ok(!got.unavailable.some((entry) => entry.field === 'node.milestone'));
  assert.equal(cli.calls.length, 5);
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
  assert.deepEqual(commands.argvFor('workPrepare', { key: 'W1' }), ['work', 'prepare', 'W1', '--response-view', 'action']);
  assert.throws(() => commands.argvFor('workPrepare', { key: 'W1;rm' }), /key/);
  assert.deepEqual(commands.argvFor('workAssess', { key: 'W1' }), ['work', 'assess', 'W1']);
  assert.throws(() => commands.argvFor('workAssess', { key: 'W1;rm' }), /key/);
  assert.throws(() => commands.argvFor('workGraph', { limit: 16385 }), /limit/);
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

test('explicit milestone configuration controls reads, including opting out with empty lanes', async () => {
  const original = source();
  const cli = createFakeCli(original);
  const got = await extract(cli);
  const asked = cli.calls.filter((c) => c[0] === 'nav' && c.includes('--milestone')).map((c) => c[c.indexOf('--milestone') + 1]).sort();
  assert.deepEqual(asked, ['M1', 'M2']);
  assert.deepEqual(got.nodes.map((n) => n.milestone), original.nodes.map((n) => n.milestone));

  const noConfig = createFakeCli(original);
  const plain = await extractSnapshot({ call: noConfig.call, config: { explore_lanes: [] }, now: NOW, wait: noWait });
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
