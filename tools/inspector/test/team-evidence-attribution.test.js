'use strict';

/**
 * AWR-OBS-006 exact outcome-evidence attribution. Fixtures are verbatim
 * run-09 observer service replies (no credentials), not minimal invented
 * shapes. completion.inspect deliberately has no work echo; artifact.segment
 * requires an artifact echo but does not require a work echo. Those two stay
 * attributable by their request binding and by the UI's cross-object checks.
 */
const test = require('node:test');
const assert = require('node:assert');
const fs = require('fs');
const path = require('path');
const { readAttributionError } = require('../team-bridge');

const DIR = path.resolve(__dirname, 'fixtures', 'obs006');
const WORK = 'issue-filter-api';
const WORKSTREAM = '013FE5E5116E5A8987D377C479';
const EVIDENCE = '01M6ATNTRCBT4VEV44CYT66CWN';
const EXECUTION = '01M6ARNTMQCWJMWM5QQVEK51MC';
const CLAIM = '01M6ARHB295GP0PQ1P111N8MFC';
const ARTIFACT = '01M6ATNTRB664BA81892TRHKQ4';
const REVIEW = '01M6AVQ870N7MBRJ5MPCN79T55';
const INTEGRATION = '01M6B0NNN221D857AA1YTBR66A';
const SHA256 = '37f728ac073d23df94311a460b8c403a6a81be9b8c36b7479a2011a2869f9a87';

function sample(file) {
  return JSON.parse(fs.readFileSync(path.join(DIR, `${file}.json`), 'utf8'));
}

function query(op, extra = {}) {
  return { protocol_version: 1, op, work_id: WORK, workstream_id: WORKSTREAM, ...extra };
}

const reads = [
  ['delivery-inspect', query('delivery.inspect')],
  ['evidence-inspect', query('evidence.inspect', { evidence_id: EVIDENCE })],
  ['execution-inspect', query('execution.inspect', { execution_id: EXECUTION, claim_id: CLAIM })],
  ['claim-inspect', query('claim.inspect', { claim_id: CLAIM })],
  ['artifact-content', query('artifact.content', { artifact_id: ARTIFACT, expected_sha256: SHA256, max_context_bytes: 1048576 })],
  ['artifact-segment', query('artifact.segment', { artifact_id: ARTIFACT, expected_sha256: SHA256, max_context_bytes: 4096 })],
  ['completion-inspect', query('completion.inspect')],
  ['delivery-source-status', query('delivery.source.status')],
  ['delivery-integration-inspect', query('delivery.integration.inspect', { request_id: INTEGRATION })],
  ['review-inspect', query('review.inspect', { review_round_id: REVIEW })],
  ['delivery-neutral-inspect', query('delivery.neutral.inspect')],
  ['delivery-submission-describe', query('delivery.submission.describe')],
];

function changed(file, pathInData, value) {
  const result = sample(file);
  let target = result.data;
  for (const key of pathInData.slice(0, -1)) {
    if (!target || typeof target !== 'object') return result;
    target = target[key];
  }
  const last = pathInData[pathInData.length - 1];
  if (value === undefined) {
    if (target && typeof target === 'object') delete target[last];
  } else if (target && typeof target === 'object') {
    target[last] = value;
  }
  return result;
}

function expectBad(result, message) {
  assert.equal(result && result.code, 'BadGateway', message);
}

test('verbatim evidence reads preserve their exact requested identities', () => {
  for (const [file, readQuery] of reads) {
    assert.equal(readAttributionError(readQuery, sample(file)), null, file);
  }
});

test('delivery.inspect reads the work echo inside data.delivery', () => {
  const readQuery = query('delivery.inspect');
  expectBad(readAttributionError(readQuery, changed('delivery-inspect', ['delivery', 'work_id'], 'OTHER')), 'wrong delivery work');
  expectBad(readAttributionError(readQuery, changed('delivery-inspect', ['delivery', 'work_id'], undefined)), 'missing delivery work');
  const legacyShape = changed('delivery-inspect', ['delivery'], undefined);
  legacyShape.data.work_id = WORK;
  expectBad(readAttributionError(readQuery, legacyShape), 'top-level work_id is not the delivery echo');
});

test('evidence.inspect requires nested work and evidence echoes', () => {
  const readQuery = query('evidence.inspect', { evidence_id: EVIDENCE });
  for (const [pathInData, value, message] of [
    [['evidence', 'work_id'], 'OTHER', 'wrong evidence work'],
    [['evidence', 'work_id'], undefined, 'missing evidence work'],
    [['evidence', 'evidence_id'], 'OTHER', 'wrong evidence id'],
    [['evidence', 'evidence_id'], undefined, 'missing evidence id'],
  ]) expectBad(readAttributionError(readQuery, changed('evidence-inspect', pathInData, value)), message);
});

test('execution.inspect requires work, execution and supplied claim echoes', () => {
  const readQuery = query('execution.inspect', { execution_id: EXECUTION, claim_id: CLAIM });
  for (const [pathInData, value, message] of [
    [['work_id'], 'OTHER', 'wrong execution work'],
    [['execution_id'], 'OTHER', 'wrong execution id'],
    [['execution_id'], undefined, 'missing execution id'],
    [['claim_id'], 'OTHER', 'wrong claim id'],
    [['claim_id'], undefined, 'missing claim id'],
  ]) expectBad(readAttributionError(readQuery, changed('execution-inspect', pathInData, value)), message);
  // A query that does not select claim_id still uses work + execution echoes.
  assert.equal(readAttributionError(query('execution.inspect', { execution_id: EXECUTION }), sample('execution-inspect')), null);
});

test('claim.inspect requires claim_id but does not invent a work echo', () => {
  const readQuery = query('claim.inspect', { claim_id: CLAIM });
  expectBad(readAttributionError(readQuery, changed('claim-inspect', ['claim_id'], 'OTHER')), 'wrong claim id');
  expectBad(readAttributionError(readQuery, changed('claim-inspect', ['claim_id'], undefined)), 'missing claim id');
  assert.equal(sample('claim-inspect').data.work_id, undefined, 'the real claim reply has no work echo');
});

test('artifact.content requires work and artifact echoes', () => {
  const readQuery = query('artifact.content', { artifact_id: ARTIFACT, expected_sha256: SHA256 });
  for (const [pathInData, value, message] of [
    [['work_id'], 'OTHER', 'wrong artifact work'],
    [['work_id'], undefined, 'missing artifact work'],
    [['artifact_id'], 'OTHER', 'wrong artifact id'],
    [['artifact_id'], undefined, 'missing artifact id'],
  ]) expectBad(readAttributionError(readQuery, changed('artifact-content', pathInData, value)), message);
});

test('artifact.segment requires artifact_id but not a work echo', () => {
  const readQuery = query('artifact.segment', { artifact_id: ARTIFACT, expected_sha256: SHA256 });
  expectBad(readAttributionError(readQuery, changed('artifact-segment', ['artifact_id'], 'OTHER')), 'wrong segment artifact');
  expectBad(readAttributionError(readQuery, changed('artifact-segment', ['artifact_id'], undefined)), 'missing segment artifact');
  assert.equal(readAttributionError(readQuery, changed('artifact-segment', ['work_id'], undefined)), null,
    'work attribution stays at the request binding and UI, not a mandatory segment echo');
});

test('completion stays request-bound and defers selected-id binding to the UI', () => {
  const readQuery = query('completion.inspect');
  const completion = sample('completion-inspect');
  assert.equal(completion.data.work_id, undefined, 'the real completion reply has no work echo');
  assert.equal(readAttributionError(readQuery, completion), null);
  const wrongSelection = changed('completion-inspect', ['selected_completion_id'], 'OTHER');
  assert.equal(readAttributionError(readQuery, wrongSelection), null,
    'the bridge does not invent a work echo; the UI must reject this selected-id mismatch');
});

test('existing review, source, integration, neutral and submission echoes match the real shapes', () => {
  expectBad(readAttributionError(query('review.inspect', { review_round_id: REVIEW }),
    changed('review-inspect', ['review', 'round_id'], 'OTHER')), 'wrong review round');
  expectBad(readAttributionError(query('delivery.source.status'),
    changed('delivery-source-status', ['work_id'], 'OTHER')), 'wrong source-status work');
  expectBad(readAttributionError(query('delivery.integration.inspect', { request_id: INTEGRATION }),
    changed('delivery-integration-inspect', ['integration_id'], 'OTHER')), 'wrong integration id');
  expectBad(readAttributionError(query('delivery.neutral.inspect'),
    changed('delivery-neutral-inspect', ['work_id'], 'OTHER')), 'wrong neutral work');
  expectBad(readAttributionError(query('delivery.submission.describe'),
    changed('delivery-submission-describe', ['work_id'], 'OTHER')), 'wrong submission work');
});
