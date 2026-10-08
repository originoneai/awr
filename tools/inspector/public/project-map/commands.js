/**
 * The official awr commands behind the project map, as argument lists (without the leading `--project <dir> --json`).
 * The static export runs them directly; the bridge's /api/map routes run the same lists after validating their query
 * parameters, and a test holds the two to each other. Every command is read-only.
 */
(function (root) {
  'use strict';

  const commonJS = typeof module !== 'undefined' && module.exports;

  const integer = (value, name, min, max) => {
    if (!Number.isSafeInteger(value) || value < min || value > max) throw new RangeError(`${name} must be an integer from ${min} to ${max}`);
    return String(value);
  };

  const BUILDERS = {
    workGraph: (p) => ['work', 'graph', '--limit', integer(p.limit === undefined ? 100 : p.limit, 'limit', 1, 1000), ...(p.cached ? ['--cached'] : [])],
    nav: (p) => ['nav', '--cached', ...(p.milestone ? ['--milestone', String(p.milestone)] : [])],
    goals: () => ['search', '--type', 'goal', '--limit', '100', '--cached'],
    events: (p) => ['event', 'history', '--limit', integer(p.limit === undefined ? 1000 : p.limit, 'limit', 1, 1000),
      ...(p.through !== undefined && p.through !== null ? ['--through-revision', integer(p.through, 'through', 0, Number.MAX_SAFE_INTEGER)] : []),
      ...(p.cursor ? ['--cursor', JSON.stringify(p.cursor)] : [])],
    sessions: () => ['session', 'list', '--active', '--limit', '100'],
    doctor: () => ['doctor'],
  };

  /** Argument list of one extractor command; throws RangeError / Error for bad parameters or unknown names. */
  function argvFor(name, params = {}) {
    if (!Object.prototype.hasOwnProperty.call(BUILDERS, name)) throw new Error(`unknown project-map command: ${name}`);
    return BUILDERS[name](params || {});
  }

  const api = { argvFor, NAMES: Object.freeze(Object.keys(BUILDERS)) };
  if (commonJS) module.exports = api;
  else {
    root.AWR_PROJECT_MAP = root.AWR_PROJECT_MAP || {};
    root.AWR_PROJECT_MAP.commands = api;
  }
})(typeof globalThis !== 'undefined' ? globalThis : this);
