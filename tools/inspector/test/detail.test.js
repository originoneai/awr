/**
 * Frontend regression tests for the detail panel.
 *
 * Run the actual renderWorkDetail() from app.js with a minimal Node DOM stub,
 * rather than a copied implementation that could drift from production code.
 *
 * Run: node --test test/detail.test.js
 */

'use strict';

const { test, beforeEach } = require('node:test');
const assert = require('node:assert');

const { install } = require('./fixtures/dom-stub.js');
install();

const app = require('../public/app.js');
const i18n = require('../public/i18n.js');

/** Install a fetch stub, record requests, and return responses by work key. */
function specPayload() {
  return {
    ok: true,
    command: 'awr spec check',
    data: {
      status: 'unknown',
      specifications: [],
      references: [],
      requirements: [],
      counts: { covered: 0, uncovered: 0, not_required: 0, needs_review: 0, unknown: 0 },
      diagnostics: ['no_explicit_specifications'],
      association_diagnostics: [],
    },
  };
}

function assessPayload(key) {
  return {
    ok: true,
    command: `awr work assess ${key}`,
    data: {
      work: key,
      decision: {
        mode: 'undetermined',
        reasons: [],
        unknown_observations: [],
        required_actions: [],
        optional_maintenance: [],
        completion_policy: 'unchanged_source_policy',
        execution_admission: 'not_granted_by_management_classification',
      },
      admission_gaps: [],
      next_action: 'record_current_assessment_with_explicit_host_observations',
      observation_basis: 'host_assertion_not_independently_verified',
    },
  };
}

function stubFetch() {
  const calls = [];
  global.fetch = async (url) => {
    const key = decodeURIComponent(String(url).split('key=')[1] || '');
    if (String(url).includes('/api/spec-check')) {
      return { json: async () => specPayload() };
    }
    if (String(url).includes('/api/assess')) {
      return { json: async () => assessPayload(key) };
    }
    calls.push(key);
    return {
      json: async () => ({
        ok: true,
        command: `awr work show ${key}`,
        data: {
          ok: true,
          project_revision: 7,
          // Mark the response so assertions can identify its owning work item.
          marker: `envelope-of-${key}`,
          work: {
            external_key: key,
            title: `标题 ${key}`,
            status: 'ready',
            next_action: `下一步 ${key}`,
            milestone: `goal#${key}`,
            active_claims: [{ agent_id: 'example-agent', expires_at: Date.now() + 10 * 60000 }],
            diagnostics: [],
          },
          acceptance: [`验收 ${key}`],
          required_dependencies: [],
          missing_dependencies: [],
          evidence: [],
          decisions: [],
          dependency_cycles: [],
        },
      }),
    };
  };
  return calls;
}

beforeEach(() => {
  i18n.setLocale('en');
  app.state.mode = 'live';
  app.state.workDetail = {};
  app.state.workAssess = {};
  app.state.specCheck = {};
  app.state.raw = {};
  app.state.selectedWork = null;
  app.detailGuard.invalidate();
});

test('cache hits restore the paired raw response', async () => {
  const calls = stubFetch();
  const rawPanel = document.getElementById('rawWorkBody');
  const detailBox = document.getElementById('workDetail');

  // 1) Select A and await its response.
  await app.renderWorkDetail('EXAMPLE-A');
  assert.equal(app.state.raw.work.data.marker, 'envelope-of-EXAMPLE-A');

  // 2) Select B and await its response.
  await app.renderWorkDetail('EXAMPLE-B');
  assert.equal(app.state.raw.work.data.marker, 'envelope-of-EXAMPLE-B');

  // 3) Select A again, using the cache.
  await app.renderWorkDetail('EXAMPLE-A');

  assert.equal(calls.length, 2, `third selection must use the cache; actual requests: ${JSON.stringify(calls)}`);

  // The title, body, raw response, and raw JSON panel must all refer to A.
  assert.equal(document.getElementById('detailId').textContent, 'EXAMPLE-A');
  assert.ok(detailBox.textContent.includes('标题 EXAMPLE-A'), 'details must belong to A');
  assert.ok(detailBox.textContent.includes('验收 EXAMPLE-A'), 'acceptance criteria must belong to A');
  assert.ok(!detailBox.textContent.includes('EXAMPLE-B'), 'details must not include B');

  assert.equal(
    app.state.raw.work.data.marker,
    'envelope-of-EXAMPLE-A',
    'state.raw.work still contains the response for B'
  );
  assert.ok(
    rawPanel.textContent.includes('envelope-of-EXAMPLE-A'),
    'raw JSON panel still shows B'
  );
  assert.ok(
    !rawPanel.textContent.includes('envelope-of-EXAMPLE-B'),
    'raw JSON panel must not contain B'
  );
});

test('cached detail compile button targets its own item', async () => {
  stubFetch();
  await app.renderWorkDetail('EXAMPLE-A');
  await app.renderWorkDetail('EXAMPLE-B');
  await app.renderWorkDetail('EXAMPLE-A');

  const box = document.getElementById('workDetail');
  // Match by tag too; textContent alone would match the button container first.
  const btn = box.find((el) => el.tagName === 'BUTTON' && el.textContent.includes('Compile context for this item'));
  assert.ok(btn, 'compile button was not found');
  btn.click();
  assert.equal(document.getElementById('fWork').value, 'EXAMPLE-A');
});

test('late responses cannot replace the selection (end to end)', async () => {
  const calls = [];
  const resolvers = {};
  global.fetch = (url) => {
    const key = decodeURIComponent(String(url).split('key=')[1] || '');
    if (String(url).includes('/api/spec-check')) {
      return Promise.resolve({ json: async () => specPayload() });
    }
    if (String(url).includes('/api/assess')) {
      return Promise.resolve({ json: async () => assessPayload(key) });
    }
    calls.push(key);
    return new Promise((resolve) => {
      resolvers[key] = () =>
        resolve({
          json: async () => ({
            ok: true,
            data: {
              marker: `envelope-of-${key}`,
              work: { external_key: key, title: `标题 ${key}`, status: 'ready', active_claims: [], diagnostics: [] },
              acceptance: [`验收 ${key}`],
              required_dependencies: [],
              missing_dependencies: [],
              evidence: [],
              decisions: [],
              dependency_cycles: [],
            },
          }),
        });
    });
  };

  const a = app.renderWorkDetail('EXAMPLE-A');
  const b = app.renderWorkDetail('EXAMPLE-B');

  // B responds before A.
  resolvers['EXAMPLE-B']();
  await b;
  resolvers['EXAMPLE-A']();
  await a;

  const box = document.getElementById('workDetail');
  assert.ok(box.textContent.includes('标题 EXAMPLE-B'), 'details must remain on B');
  assert.ok(!box.textContent.includes('标题 EXAMPLE-A'), 'late response for A must be discarded');
  assert.equal(app.state.raw.work.data.marker, 'envelope-of-EXAMPLE-B');
});

test('late failure responses cannot replace the selection', async () => {
  const resolvers = {};
  global.fetch = (url) => {
    const key = decodeURIComponent(String(url).split('key=')[1] || '');
    if (String(url).includes('/api/spec-check')) {
      return Promise.resolve({ json: async () => specPayload() });
    }
    if (String(url).includes('/api/assess')) {
      return Promise.resolve({ json: async () => assessPayload(key) });
    }
    return new Promise((resolve) => {
      resolvers[key] = () =>
        resolve({
          json: async () =>
            key === 'EXAMPLE-A'
              ? { ok: false, command: 'awr work show A', error: { code: 'SourceStale', message: '过期了' } }
              : {
                  ok: true,
                  data: {
                    marker: `envelope-of-${key}`,
                    work: { external_key: key, title: `标题 ${key}`, status: 'ready', active_claims: [], diagnostics: [] },
                    acceptance: [],
                    required_dependencies: [],
                    missing_dependencies: [],
                    evidence: [],
                    decisions: [],
                    dependency_cycles: [],
                  },
                },
        });
    });
  };

  const a = app.renderWorkDetail('EXAMPLE-A');
  const b = app.renderWorkDetail('EXAMPLE-B');
  resolvers['EXAMPLE-B']();
  await b;
  resolvers['EXAMPLE-A']();
  await a;

  const box = document.getElementById('workDetail');
  assert.ok(box.textContent.includes('标题 EXAMPLE-B'), 'details must remain on B');
  assert.ok(!box.textContent.includes('SourceStale'), 'late failures must not be displayed');
  assert.equal(app.state.raw.work.data.marker, 'envelope-of-EXAMPLE-B');
});

test('refresh invalidates older requests for the same key', async () => {
  const resolvers = {};
  global.fetch = (url) => {
    const key = decodeURIComponent(String(url).split('key=')[1] || '');
    if (String(url).includes('/api/spec-check')) {
      return Promise.resolve({ json: async () => specPayload() });
    }
    if (String(url).includes('/api/assess')) {
      return Promise.resolve({ json: async () => assessPayload(key) });
    }
    return new Promise((resolve) => {
      resolvers[key] = (marker) =>
        resolve({
          json: async () => ({
            ok: true,
            data: {
              marker,
              work: { external_key: key, title: marker, status: 'ready', active_claims: [], diagnostics: [] },
              acceptance: [],
              required_dependencies: [],
              missing_dependencies: [],
              evidence: [],
              decisions: [],
              dependency_cycles: [],
            },
          }),
        });
    });
  };

  const stale = app.renderWorkDetail('EXAMPLE-A');
  // Refresh: invalidate in-flight requests and clear the cache, as the UI does.
  app.detailGuard.invalidate();
  app.state.workDetail = {};
  app.state.workAssess = {};
  app.state.specCheck = {};

  resolvers['EXAMPLE-A']('旧的-A');
  await stale;

  assert.equal(app.state.raw.work, undefined, 'pre-refresh responses must be discarded');
  assert.equal(app.state.workDetail['EXAMPLE-A'], undefined, 'pre-refresh responses must not enter the cache');
});

for (const locale of ['en', 'zh-CN']) {
  test(`detail labels use ${locale} while multilingual project content stays unchanged`, async () => {
    stubFetch();
    i18n.setLocale(locale);
    await app.renderWorkDetail('EXAMPLE-A');
    const text = document.getElementById('workDetail').textContent;
    assert.ok(text.includes('标题 EXAMPLE-A'));
    assert.ok(text.includes('验收 EXAMPLE-A'));
    assert.ok(text.includes(i18n.t('ui.compile_context_for_this_item')));
    assert.ok(text.includes(i18n.t('ui.expires_in_p0_min', { p0: 10 })));
    i18n.setLocale('en');
  });
}
