'use strict';
// The next step the page shows for a work is the guidance the service gave the agent, in the page's words: every situation of the task_intake
// code has its own phrase (word for word the service's note in English), and a code the page has no phrase for shows the service's own
// note, never a generic request to refresh that says something else.
const { test, beforeEach } = require('node:test');
const assert = require('node:assert/strict');
const { install } = require('./fixtures/dom-stub');
const { createTeamWeb } = require('../public/team-web');
const i18n = require('../public/i18n');
const en = require('../public/locales/en.js');
const zh = require('../public/locales/zh-CN.js');
const node = id => document.getElementById(id);
let ui;
beforeEach(() => {
  install(); i18n.setLocale('en'); ui = createTeamWeb({ i18n, $: node });
});

const NOTE_OTHER = 'This task belongs to another member. Select your assignment or an available task.';
const SITUATIONS = [
  ['other', 'owned_by_other', 'work.next', NOTE_OTHER],
  ['other', 'assigned_to_other', 'work.next', NOTE_OTHER],
  ['pending', 'responsibility_pending', 'work.recovery', 'Inspect the pending responsibility and request controlled handoff before claiming.'],
  ['pending', 'handoff_required', 'work.recovery', 'Inspect the pending responsibility and request controlled handoff before claiming.'],
  ['observe', 'pool', 'work.next', 'Observe this task within your current scope; taking it requires a current development grant.'],
  ['observe', 'assigned_to_me', 'work.next', 'Observe this task within your current scope; taking it requires a current development grant.'],
  ['observe', 'owned_by_me', 'work.next', 'Observe this task within your current scope; taking it requires a current development grant.'],
  ['session_start', 'pool', 'session.start', 'Consume work.prepare and start your own session before taking responsibility and a coordination lease.'],
  ['accept_assignment', 'assigned_to_me', 'task.accept_assignment', 'Accept the current assignment with its request key and responsibility version after dependencies are ready.'],
  ['claim_available', 'pool', 'task.claim_available', 'Claim the available task and coordination lease together using current responsibility and work versions.'],
  ['claim_acquire', 'owned_by_me', 'claim.acquire', 'Continue your owned task with current work versions after checking dependencies and settled effects.'],
];

function live(guidance, nextStep) {
  const works = [{ key: 'W-0', workstream_id: 'stream', contract_hash: 'current' }];
  global.fetch = async (url) => ({ ok: true, json: async () => {
    if (url.includes('/projects?')) return { ok: true, projects: [{ key: 'project' }], session: { session_id: 'test' } };
    if (url.includes('/overview?')) return { ok: true, works, workstreams: [{ id: 'stream', title: 'Stream' }], interaction_mode: 'mcp' };
    return { ok: true, work: { ...works[0], detail_loaded: true, status: null, acceptance: ['Verify the contract'], depends_on: [], context_complete: true,
      next_step: nextStep, ...(guidance ? { guidance } : {}) } };
  } });
}

async function detailText(guidance, nextStep) {
  live(guidance, nextStep);
  await ui.refresh();
  await ui._selectWork('W-0');
  return node('teamDetail').textContent;
}

const intake = (when, op, note) => ({ code: 'task_intake', when, because: ['current responsibility version 2'], action: { op, note }, recheck_on: 'responsibility, dependency, lease or permission changes' });
const GENERIC_EN = en['feedback.guidance_unknown'];
const GENERIC_ZH = zh['feedback.guidance_unknown'];

test('every situation of the task_intake code has its own phrase, word for word the service note in English', async () => {
  for (const [variant, when, op, note] of SITUATIONS) {
    const key = 'feedback.guidance_task_intake_' + variant;
    assert.equal(en[key], note, key + ' is the service note');
    const text = await detailText(intake(when, op, note), 'a hint');
    assert.ok(text.includes(note), when + ' ' + op + ' shows its phrase');
    assert.ok(!text.includes(GENERIC_EN), when + ' ' + op + ' is not the generic request');
  }
});

test('the Chinese page words each situation too, and never falls back to the generic sentence', async () => {
  i18n.setLocale('zh-CN'); ui = createTeamWeb({ i18n, $: node });
  for (const [variant, when, op, note] of SITUATIONS) {
    const phrase = zh['feedback.guidance_task_intake_' + variant];
    assert.ok(phrase && phrase !== en['feedback.guidance_task_intake_' + variant], variant + ' has a Chinese phrase');
    const text = await detailText(intake(when, op, note), 'a hint');
    assert.ok(text.includes(phrase), when + ' ' + op + ' shows its Chinese phrase');
    assert.ok(!text.includes(GENERIC_ZH), when + ' ' + op + ' is not the generic request');
  }
});

test('an owned-by-another task and an observe-only task are different sentences although both name work.next', async () => {
  const other = await detailText(intake('owned_by_other', 'work.next', NOTE_OTHER), null);
  const observe = await detailText(intake('pool', 'work.next', en['feedback.guidance_task_intake_observe']), null);
  assert.ok(other.includes(NOTE_OTHER) && !other.includes(en['feedback.guidance_task_intake_observe']));
  assert.ok(observe.includes(en['feedback.guidance_task_intake_observe']) && !observe.includes(NOTE_OTHER));
});

test('a code the page has no phrase for shows the service note; without a note the generic sentence stays', async () => {
  const known = await detailText({ code: 'a_code_from_a_later_service', when: 'x', action: { op: 'work.next', note: 'Do the thing the service says.' } }, 'a hint');
  assert.ok(known.includes('Do the thing the service says.'));
  assert.ok(!known.includes(GENERIC_EN));
  const bare = await detailText({ code: 'a_code_from_a_later_service', when: 'x' }, 'a hint');
  assert.ok(bare.includes(GENERIC_EN));
});

test('an unrecognised task_intake situation shows the service note, not another situation\'s phrase', async () => {
  const text = await detailText(intake('pool', 'a.new_op', 'A new situation of the service.'), 'a hint');
  assert.ok(text.includes('A new situation of the service.'));
  for (const variant of new Set(SITUATIONS.map(row => row[0]))) assert.ok(!text.includes(en['feedback.guidance_task_intake_' + variant]), variant);
});

test('a code with its own phrase and a work without guidance are unchanged', async () => {
  const withPhrase = await detailText({ code: 'inspect_completion', when: 'a completion is recorded', action: { op: 'work.next', note: 'service note' } }, 'a hint');
  assert.ok(withPhrase.includes(en['feedback.guidance_inspect_completion']));
  const plain = await detailText(null, 'The plain next step');
  assert.ok(plain.includes('The plain next step'));
});
