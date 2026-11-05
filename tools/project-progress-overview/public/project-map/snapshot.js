/**
 * Project snapshot contract (schema awr-project-snapshot, version 1).
 *
 * A snapshot is the only input of the project-map renderers: it holds what the official `awr --json` commands reported
 * for one project revision, plus the list of fields those commands do not provide. This module has no I/O. It defines the
 * canonical JSON form, the content fingerprint and the structural validation, and it runs unchanged in the browser and
 * in Node (the static export).
 */
(function (root) {
  'use strict';

  const commonJS = typeof module !== 'undefined' && module.exports;

  const SCHEMA = 'awr-project-snapshot';
  const VERSION = 1;
  const STATUSES = Object.freeze(['draft', 'planned', 'ready', 'claimed', 'in_progress', 'blocked', 'completed', 'cancelled']);
  /** Fields that describe the extraction, not the project state; they are left out of the content fingerprint. */
  const VOLATILE = Object.freeze(['fingerprint', 'generated_at']);

  /** What the official interfaces do not provide. Renderers must show these as unavailable and never fill them in. */
  const UNAVAILABLE = Object.freeze([
    { field: 'node.goal', reason: 'work graph and nav nodes carry no goal; membership exists only per goal (nav --goal), which this map does not query' },
    { field: 'node.scope', reason: 'the contract or scope id is not exposed per item' },
    { field: 'node.priority', reason: 'only work show returns it, one refreshing call per item; not collected in bulk' },
    { field: 'node.milestone', reason: 'there is no bulk field; it is resolved only for the milestones named in the display configuration, through nav --milestone' },
    { field: 'project.scenarios', reason: 'project-specific acceptance scenarios are outside the AWR work model and have no data source' },
    { field: 'project.release_funnel', reason: 'a presentation concept without a data source' },
  ].map((entry) => Object.freeze(entry)));

  class SnapshotError extends Error {
    constructor(message) {
      super(`snapshot invalid: ${message}`);
      this.name = 'SnapshotError';
    }
  }

  const compare = (a, b) => (a < b ? -1 : a > b ? 1 : 0);

  /** Canonical JSON: sorted keys, no whitespace, UTF-8 text unescaped, undefined properties dropped. */
  function canonical(value) {
    if (value === undefined) return 'null';
    if (value === null || typeof value !== 'object') return JSON.stringify(value);
    if (Array.isArray(value)) return `[${value.map(canonical).join(',')}]`;
    const keys = Object.keys(value).filter((k) => value[k] !== undefined).sort(compare);
    return `{${keys.map((k) => `${JSON.stringify(k)}:${canonical(value[k])}`).join(',')}}`;
  }

  // SHA-256 for browsers without a usable crypto module (Node uses its own).

  const K = new Uint32Array([
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
  ]);

  function sha256Portable(text) {
    const bytes = new TextEncoder().encode(text);
    const length = bytes.length;
    const padded = new Uint8Array(((length + 9 + 63) >> 6) << 6);
    padded.set(bytes);
    padded[length] = 0x80;
    const view = new DataView(padded.buffer);
    view.setUint32(padded.length - 8, Math.floor(length / 0x20000000), false);
    view.setUint32(padded.length - 4, (length << 3) >>> 0, false);
    const rotr = (x, n) => ((x >>> n) | (x << (32 - n))) >>> 0;
    const h = new Uint32Array([0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19]);
    const w = new Uint32Array(64);
    for (let offset = 0; offset < padded.length; offset += 64) {
      for (let i = 0; i < 16; i++) w[i] = view.getUint32(offset + i * 4, false);
      for (let i = 16; i < 64; i++) {
        const s0 = rotr(w[i - 15], 7) ^ rotr(w[i - 15], 18) ^ (w[i - 15] >>> 3);
        const s1 = rotr(w[i - 2], 17) ^ rotr(w[i - 2], 19) ^ (w[i - 2] >>> 10);
        w[i] = (w[i - 16] + s0 + w[i - 7] + s1) >>> 0;
      }
      let [a, b, c, d, e, f, g, hh] = h;
      for (let i = 0; i < 64; i++) {
        const t1 = (hh + (rotr(e, 6) ^ rotr(e, 11) ^ rotr(e, 25)) + ((e & f) ^ (~e & g)) + K[i] + w[i]) >>> 0;
        const t2 = ((rotr(a, 2) ^ rotr(a, 13) ^ rotr(a, 22)) + ((a & b) ^ (a & c) ^ (b & c))) >>> 0;
        hh = g; g = f; f = e; e = (d + t1) >>> 0; d = c; c = b; b = a; a = (t1 + t2) >>> 0;
      }
      h[0] += a; h[1] += b; h[2] += c; h[3] += d; h[4] += e; h[5] += f; h[6] += g; h[7] += hh;
    }
    return Array.from(h, (x) => x.toString(16).padStart(8, '0')).join('');
  }

  function sha256Hex(text) {
    if (commonJS) return require('node:crypto').createHash('sha256').update(text, 'utf8').digest('hex');
    return sha256Portable(text);
  }

  /** `sha256:` + hash of the canonical JSON of the snapshot without its volatile fields. */
  function fingerprint(snapshot) {
    const body = {};
    for (const key of Object.keys(snapshot)) if (!VOLATILE.includes(key)) body[key] = snapshot[key];
    return `sha256:${sha256Hex(canonical(body))}`;
  }

  /** Throws SnapshotError on any structural violation; returns the snapshot. */
  function validate(snapshot) {
    const need = (condition, message) => {
      if (!condition) throw new SnapshotError(message);
    };
    need(snapshot && typeof snapshot === 'object', 'not an object');
    need(snapshot.schema === SCHEMA && snapshot.version === VERSION, 'schema or version');
    need(snapshot.fingerprint === fingerprint(snapshot), 'fingerprint does not match the content');
    for (const field of ['project', 'basis', 'nodes', 'edges', 'goals', 'sessions', 'doctor', 'unavailable']) {
      need(snapshot[field] !== undefined && snapshot[field] !== null, `missing ${field}`);
    }
    const nodes = snapshot.nodes;
    const keys = nodes.map((n) => n.key);
    const sorted = [...keys].sort(compare);
    need(keys.every((k, i) => k === sorted[i]) && new Set(keys).size === keys.length, 'nodes must be unique and sorted by key');
    need(nodes.every((n) => STATUSES.includes(n.status)), 'unknown status');
    const known = new Set(keys);
    const pairs = snapshot.edges.map((e) => `${e.dependent}\u0000${e.prerequisite}`);
    const sortedPairs = [...pairs].sort(compare);
    need(pairs.every((p, i) => p === sortedPairs[i]) && new Set(pairs).size === pairs.length, 'edges must be unique and sorted');
    need(snapshot.edges.every((e) => known.has(e.dependent) && known.has(e.prerequisite)), 'edge endpoint is not a node');
    const graph = snapshot.basis.work_graph || {};
    need(graph.node_count === nodes.length && graph.edge_count === snapshot.edges.length, 'basis counts');
    for (const session of snapshot.sessions) {
      need(session.work_key === null || known.has(session.work_key), 'session points to an unknown work key');
    }
    return snapshot;
  }

  /** Finish extracted content: stamp the time it was taken and the fingerprint, then validate. */
  function seal(content, generatedAt) {
    const snapshot = Object.assign({}, content, { generated_at: generatedAt });
    snapshot.fingerprint = fingerprint(snapshot);
    return validate(snapshot);
  }

  const api = { SCHEMA, VERSION, STATUSES, VOLATILE, UNAVAILABLE, SnapshotError, canonical, fingerprint, validate, seal, sha256Hex, sha256Portable, compare };
  if (commonJS) module.exports = api;
  else {
    root.AWR_PROJECT_MAP = root.AWR_PROJECT_MAP || {};
    root.AWR_PROJECT_MAP.snapshot = api;
  }
})(typeof globalThis !== 'undefined' ? globalThis : this);
