/**
 * Shared observation read contract (AWR-OBS-002). Pure and dependency-free:
 * the Inspector bridge requires it and the browser surface may import it as
 * a plain script, so both sides apply one vocabulary and one ordering rule.
 *
 * Plane states: every freshness clock (report received, authoritative
 * measurement, page read) is exactly one of measured | stale | failed |
 * not_measured. An unmeasured, stale or failed plane never substitutes for
 * another state: a missing measurement is reported as not_measured, an old
 * one as stale, and one that could not be obtained as failed.
 *
 * Who decides stale: the service where it declares one (the progress record
 * carries `stale`, computed against the `stale_after_ms` the capabilities
 * declare in session_feedback, and `report_order_basis` names the ordering
 * key of reports); otherwise the bridge applies a bound it always reports
 * next to the measurement (stale_after_ms and age_ms). The bridge's own
 * bound below is a declared constant, never a hidden one.
 *
 * Out-of-order rule: an incoming authoritative measurement may replace the
 * held one only when both carry the same source snapshot and coordinator
 * epoch and the incoming project revision is numerically higher or equal. A
 * lower revision never replaces. A different snapshot or epoch, or a plane
 * that is unmeasured, partial or failed, is incomparable: the verdict says
 * so and neither side is silently dropped or adopted.
 */
(function (root, factory) {
  if (typeof module === 'object' && module.exports) {
    module.exports = factory();
  } else {
    root.AwrObservationContract = factory();
  }
})(typeof self !== 'undefined' ? self : globalThis, function () {
  'use strict';

  const PLANE_STATES = Object.freeze(['measured', 'stale', 'failed', 'not_measured']);

  /** The bridge's declared staleness bound, reported wherever it is applied. */
  const OBSERVATION_STALE_AFTER_MS = 120000;

  const REPLACE = 'replace';
  const KEEP = 'keep';
  const INCOMPARABLE = 'incomparable';

  /**
   * The comparable core of an authoritative plane, or null when the plane
   * carries no complete measurement. Only measured and stale planes are
   * measurements; failed and not_measured planes (and partial field sets)
   * are not comparable. Revisions must be safe integers: "9" < "10".
   */
  function measurementOf(plane) {
    if (!plane || typeof plane !== 'object' || Array.isArray(plane)) return null;
    if (plane.status != null && plane.status !== 'measured' && plane.status !== 'stale') return null;
    const revision = typeof plane.project_revision === 'string' && /^\d+$/.test(plane.project_revision)
      ? Number(plane.project_revision) : null;
    if (revision == null || !Number.isSafeInteger(revision)) return null;
    const source = typeof plane.source_snapshot_id === 'string' && plane.source_snapshot_id
      ? plane.source_snapshot_id : null;
    const epoch = typeof plane.coordinator_epoch === 'string' && plane.coordinator_epoch
      ? plane.coordinator_epoch : null;
    if (!source || !epoch) return null;
    return { revision, source, epoch };
  }

  /**
   * May `incoming` replace `held`? Returns REPLACE (same snapshot and epoch,
   * numerically higher or equal revision), KEEP (same snapshot and epoch,
   * lower revision — an old reply never overwrites a newer one) or
   * INCOMPARABLE (different snapshot or epoch, or either plane unmeasured,
   * partial or failed — the caller must declare the conflict rather than
   * silently dropping or adopting either side).
   */
  function compareAuthoritative(held, incoming) {
    const heldMeasurement = measurementOf(held);
    const incomingMeasurement = measurementOf(incoming);
    if (!heldMeasurement || !incomingMeasurement) return INCOMPARABLE;
    if (heldMeasurement.source !== incomingMeasurement.source
      || heldMeasurement.epoch !== incomingMeasurement.epoch) return INCOMPARABLE;
    return incomingMeasurement.revision >= heldMeasurement.revision ? REPLACE : KEEP;
  }

  /**
   * Resolve measured vs stale for a plane that carries a measurement. The
   * service's own stale declaration wins; otherwise the declared bound
   * (staleAfterMs) decides against the age at readAtMs. The bound and the
   * age are reported next to the measurement whenever a bound is in effect;
   * with no bound there is nothing to report and nothing is invented.
   */
  function resolveCurrency({ serviceStale, atUnixMs, readAtMs, staleAfterMs } = {}) {
    const bound = Number.isFinite(staleAfterMs) ? staleAfterMs : null;
    const age = Number.isFinite(atUnixMs) && Number.isFinite(readAtMs) ? readAtMs - atUnixMs : null;
    const stale = serviceStale === true || (bound != null && age != null && age > bound);
    const result = { status: stale ? 'stale' : 'measured' };
    if (bound != null) result.stale_after_ms = bound;
    if (bound != null && age != null) result.age_ms = age;
    return result;
  }

  return { PLANE_STATES, OBSERVATION_STALE_AFTER_MS, compareAuthoritative, resolveCurrency };
});
