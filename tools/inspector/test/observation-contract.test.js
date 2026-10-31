'use strict';

/**
 * AWR-OBS-002 shared observation contract (pure module, browser-importable).
 * Pins the plane state vocabulary, the bridge-declared staleness bound and
 * the out-of-order rule: an incoming authoritative measurement may replace
 * the held one only when both carry the same source snapshot and coordinator
 * epoch and the incoming project revision is numerically higher or equal; a
 * lower revision never replaces; a different snapshot or epoch, or a plane
 * that is unmeasured, partial or failed, is incomparable and the verdict
 * says so instead of silently dropping or adopting either side.
 */
const test = require('node:test');
const assert = require('node:assert');
const {
  PLANE_STATES,
  OBSERVATION_STALE_AFTER_MS,
  READ_TIMEOUT_MS,
  OVERVIEW_DEADLINE_MS,
  compareAuthoritative,
  resolveCurrency,
} = require('../observation-contract');

const measured = (revision, extra = {}) => ({
  status: 'measured',
  project_revision: revision,
  source_snapshot_id: 'snap-1',
  coordinator_epoch: 'epoch-1',
  ...extra,
});

test('the freshness vocabulary is measured, stale, failed or not_measured', () => {
  assert.deepEqual([...PLANE_STATES], ['measured', 'stale', 'failed', 'not_measured']);
  assert.ok(Object.isFrozen(PLANE_STATES));
  assert.ok(Number.isInteger(OBSERVATION_STALE_AFTER_MS) && OBSERVATION_STALE_AFTER_MS > 0);
});

test('a newer measurement replaces the held one; an older one never does', () => {
  // Arrival order older-then-newer.
  assert.equal(compareAuthoritative(measured('20'), measured('21')), 'replace');
  // Arrival order newer-then-older: the late old reply must not overwrite.
  assert.equal(compareAuthoritative(measured('21'), measured('20')), 'keep');
  // An equal revision is the same measurement and may be adopted.
  assert.equal(compareAuthoritative(measured('20'), measured('20')), 'replace');
});

test('revisions compare as numbers, never as strings', () => {
  // Lexicographically "10" < "9"; numerically 9 < 10.
  assert.equal(compareAuthoritative(measured('9'), measured('10')), 'replace');
  assert.equal(compareAuthoritative(measured('10'), measured('9')), 'keep');
});

test('a different coordinator epoch or source snapshot is incomparable', () => {
  assert.equal(
    compareAuthoritative(measured('20'), measured('21', { coordinator_epoch: 'epoch-2' })),
    'incomparable'
  );
  assert.equal(
    compareAuthoritative(measured('20'), measured('21', { source_snapshot_id: 'snap-2' })),
    'incomparable'
  );
  assert.equal(
    compareAuthoritative(measured('20', { coordinator_epoch: 'epoch-2' }), measured('19')),
    'incomparable'
  );
});

test('an unmeasured, failed or partial plane is incomparable, never dropped or adopted silently', () => {
  const held = measured('20');
  assert.equal(compareAuthoritative(held, { status: 'not_measured' }), 'incomparable');
  assert.equal(compareAuthoritative({ status: 'not_measured' }, held), 'incomparable');
  assert.equal(compareAuthoritative(held, { status: 'failed', error_code: 'ReadTimeout' }), 'incomparable');
  // A partial plane (some fields missing) is not a measurement.
  assert.equal(compareAuthoritative(held, { status: 'measured', project_revision: '21', source_snapshot_id: 'snap-1' }), 'incomparable');
  assert.equal(compareAuthoritative({ status: 'measured', project_revision: '20', coordinator_epoch: 'epoch-1' }, measured('19')), 'incomparable');
  assert.equal(compareAuthoritative(held, null), 'incomparable');
  assert.equal(compareAuthoritative(undefined, held), 'incomparable');
});

test('a stale plane still carries a comparable measurement', () => {
  const staleOlder = measured('19', { status: 'stale', age_ms: 10, stale_after_ms: 5 });
  const staleNewer = measured('21', { status: 'stale', age_ms: 10, stale_after_ms: 5 });
  assert.equal(compareAuthoritative(measured('20'), staleNewer), 'replace');
  assert.equal(compareAuthoritative(measured('20'), staleOlder), 'keep');
});

test('a revision that is not a safe numeric string is not comparable', () => {
  assert.equal(compareAuthoritative(measured('20'), measured('rev-21')), 'incomparable');
  assert.equal(compareAuthoritative(measured('9007199254740993'), measured('20')), 'incomparable');
});

test('resolveCurrency: the service declaration wins, then the declared bound, both reported', () => {
  // The service-declared stale flag decides even when the age looks small.
  assert.deepEqual(
    resolveCurrency({ serviceStale: true, atUnixMs: 1000, readAtMs: 1001, staleAfterMs: 900000 }),
    { status: 'stale', stale_after_ms: 900000, age_ms: 1 }
  );
  // Over the declared bound: stale, with the bound and the age reported.
  assert.deepEqual(
    resolveCurrency({ serviceStale: false, atUnixMs: 1000, readAtMs: 2000, staleAfterMs: 500 }),
    { status: 'stale', stale_after_ms: 500, age_ms: 1000 }
  );
  // Within the declared bound: measured, still reporting bound and age.
  assert.deepEqual(
    resolveCurrency({ serviceStale: false, atUnixMs: 1000, readAtMs: 1200, staleAfterMs: 500 }),
    { status: 'measured', stale_after_ms: 500, age_ms: 200 }
  );
  // No bound in effect: measured without inventing a bound.
  assert.deepEqual(
    resolveCurrency({ serviceStale: false, atUnixMs: 1000, readAtMs: 99999999 }),
    { status: 'measured' }
  );
  // No time of its own: measured by its revision, no age to report.
  assert.deepEqual(
    resolveCurrency({ serviceStale: false, readAtMs: 1000, staleAfterMs: 500 }),
    { status: 'measured', stale_after_ms: 500 }
  );
  // A future timestamp (clock skew) is not stale.
  assert.deepEqual(
    resolveCurrency({ serviceStale: false, atUnixMs: 5000, readAtMs: 1000, staleAfterMs: 500 }),
    { status: 'measured', stale_after_ms: 500, age_ms: -4000 }
  );
});

test('the declared bounds are pinned constants the PR text can name', () => {
  assert.equal(OBSERVATION_STALE_AFTER_MS, 120000);
  assert.equal(READ_TIMEOUT_MS, 4000);
  assert.equal(OVERVIEW_DEADLINE_MS, 12000);
});

test('a failed or unmeasured plane is incomparable even when it carries all three fields', () => {
  const fields = { project_revision: '21', source_snapshot_id: 'snap-1', coordinator_epoch: 'epoch-1' };
  assert.equal(
    compareAuthoritative(measured('20'), { status: 'failed', error_code: 'ReadTimeout', ...fields }),
    'incomparable'
  );
  assert.equal(
    compareAuthoritative(measured('20'), { status: 'not_measured', ...fields }),
    'incomparable'
  );
  assert.equal(
    compareAuthoritative({ status: 'failed', error_code: 'ReadTimeout', ...fields }, measured('20')),
    'incomparable'
  );
});

test('only a plain decimal revision is comparable', () => {
  for (const bogus of ['1e3', '0x10', ' 12', '1.0']) {
    assert.equal(compareAuthoritative(measured('20'), measured(bogus)), 'incomparable', bogus);
    assert.equal(compareAuthoritative(measured(bogus), measured('20')), 'incomparable', bogus);
  }
});

test('two planes that both lack a source or an epoch are incomparable, not equal', () => {
  const noSource = { status: 'measured', project_revision: '20', coordinator_epoch: 'epoch-1' };
  assert.equal(compareAuthoritative(noSource, { ...noSource }), 'incomparable');
  const noEpoch = { status: 'measured', project_revision: '20', source_snapshot_id: 'snap-1' };
  assert.equal(compareAuthoritative(noEpoch, { ...noEpoch }), 'incomparable');
});

test('an age exactly at the bound is not stale; one millisecond more is', () => {
  assert.deepEqual(
    resolveCurrency({ serviceStale: false, atUnixMs: 1000, readAtMs: 1500, staleAfterMs: 500 }),
    { status: 'measured', stale_after_ms: 500, age_ms: 500 }
  );
  assert.deepEqual(
    resolveCurrency({ serviceStale: false, atUnixMs: 1000, readAtMs: 1501, staleAfterMs: 500 }),
    { status: 'stale', stale_after_ms: 500, age_ms: 501 }
  );
});
