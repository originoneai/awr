/**
 * Snapshot contract: canonical JSON, fingerprint and validation. The fixtures empty/single/states were written by the
 * reference implementation of the contract in another language, so validating them also proves both implementations
 * hash the same canonical bytes.
 *
 * Run: node --test test/project-map-snapshot.test.js
 */

'use strict';

const { test } = require('node:test');
const assert = require('node:assert/strict');
const crypto = require('node:crypto');
const h = require('./fixtures/project-map/helpers.js');
const s = require('../public/project-map/snapshot.js');

const clone = (value) => JSON.parse(JSON.stringify(value));
const resealed = (snapshot) => ({ ...snapshot, fingerprint: s.fingerprint(snapshot) });

test('fixtures from the reference implementation validate: same canonical JSON, same hash', () => {
  for (const name of ['empty', 'single', 'states']) {
    const snapshot = h.readJson(`${name}.json`);
    assert.equal(s.validate(snapshot), snapshot, name);
    assert.match(snapshot.fingerprint, /^sha256:[0-9a-f]{64}$/);
  }
});

test('canonical JSON sorts keys, drops whitespace and keeps text unescaped', () => {
  assert.equal(s.canonical({ b: 1, a: [true, null, 'é中文'], c: { z: 1, y: undefined } }), '{"a":[true,null,"é中文"],"b":1,"c":{"z":1}}');
  assert.equal(s.canonical('a"b\n'), '"a\\"b\\n"');
  assert.equal(s.canonical([]), '[]');
  assert.equal(s.canonical(undefined), 'null');
});

test('the fingerprint covers the content and ignores generated_at and itself', () => {
  const snapshot = h.readJson('states.json');
  assert.equal(s.fingerprint({ ...snapshot, generated_at: '2030-01-01T00:00:00Z', fingerprint: 'x' }), snapshot.fingerprint);
  const changed = clone(snapshot);
  changed.nodes[0].title += '!';
  assert.notEqual(s.fingerprint(changed), snapshot.fingerprint);
});

test('validation refuses every structural violation', () => {
  const base = h.readJson('states.json');
  const refuse = (mutate, pattern) => {
    const snapshot = clone(base);
    mutate(snapshot);
    assert.throws(() => s.validate(resealed(snapshot)), pattern);
  };
  refuse((x) => { x.schema = 'other'; }, /schema or version/);
  refuse((x) => { x.version = 2; }, /schema or version/);
  refuse((x) => { x.nodes.reverse(); }, /unique and sorted/);
  refuse((x) => { x.nodes[1].key = x.nodes[0].key; }, /unique and sorted/);
  refuse((x) => { x.nodes[0].status = 'paused'; }, /unknown status/);
  refuse((x) => { x.edges.reverse(); }, /edges must be unique and sorted/);
  refuse((x) => { x.edges.push({ ...x.edges[0] }); }, /edges must be unique and sorted/);
  refuse((x) => { x.edges[0].prerequisite = 'NOWHERE'; x.edges.sort((a, b) => (a.dependent + a.prerequisite < b.dependent + b.prerequisite ? -1 : 1)); }, /not a node|sorted/);
  refuse((x) => { x.basis.work_graph.node_count += 1; }, /basis counts/);
  refuse((x) => { x.sessions[0].work_key = 'NOWHERE'; }, /unknown work key/);
  refuse((x) => { delete x.doctor; }, /missing doctor/);
  // a stale fingerprint is caught even when the content looks fine
  const tampered = clone(base);
  tampered.nodes[0].title = 'edited after sealing';
  assert.throws(() => s.validate(tampered), /fingerprint does not match/);
  assert.throws(() => s.validate(null), /not an object/);
});

test('seal stamps the extraction time and a matching fingerprint', () => {
  const content = clone(h.readJson('empty.json'));
  delete content.generated_at;
  delete content.fingerprint;
  const snapshot = s.seal(content, '2026-02-03T04:05:06Z');
  assert.equal(snapshot.generated_at, '2026-02-03T04:05:06Z');
  assert.equal(snapshot.fingerprint, s.fingerprint(snapshot));
  // the same content sealed at another time keeps its fingerprint: only the clock differs
  assert.equal(s.seal(content, '2030-01-01T00:00:00Z').fingerprint, snapshot.fingerprint);
});

test('the portable SHA-256 used in browsers matches the platform implementation', () => {
  const samples = ['', 'abc', 'a'.repeat(55), 'a'.repeat(56), 'a'.repeat(63), 'a'.repeat(64), 'a'.repeat(65), 'é中文🙂', 'x'.repeat(100000)];
  let seed = 7;
  const random = () => { seed = (seed * 1103515245 + 12345) & 0x7fffffff; return seed; };
  for (let i = 0; i < 40; i++) samples.push(Array.from({ length: random() % 500 }, () => String.fromCodePoint(32 + (random() % 0x2000))).join(''));
  for (const text of samples) assert.equal(s.sha256Portable(text), crypto.createHash('sha256').update(text, 'utf8').digest('hex'));
});

test('the list of unavailable fields is part of every snapshot and names the known gaps', () => {
  assert.deepEqual(s.UNAVAILABLE.map((u) => u.field), ['node.goal', 'node.scope', 'node.priority', 'node.milestone', 'project.scenarios', 'project.release_funnel']);
  for (const name of ['empty', 'single', 'states']) assert.ok(h.readJson(`${name}.json`).unavailable.length >= 6, name);
});
