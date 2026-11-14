'use strict';

const { READ_OPS } = require('./team-bridge');

const ERROR_CODES = new Set([
  'Unauthorized', 'Forbidden', 'Unavailable', 'Unsupported', 'Timeout', 'NotFound',
  'InvalidInput', 'ResponseTooLarge', 'SourceDivergence', 'RevisionConflict',
  'BridgeError', 'UpstreamError', 'BadGateway', 'ReadTimeout', 'BodyTooLarge', 'NoRoute',
  'BadRequestTarget', 'ForbiddenHost', 'ForbiddenOrigin', 'ForbiddenSite', 'MissingGuardHeader',
]);

/** Bounded operational metadata only. Request bodies and upstream prose never enter logs. */
function diagnosticRecord({ requestId, route, method, operation, status, code, durationMs }) {
  const safeCode = ERROR_CODES.has(code)
    ? code : 'ObservationError';
  return {
    timestamp: new Date().toISOString(),
    component: 'awr-observer',
    event: 'observation_failed',
    request_id: requestId,
    route,
    method,
    operation: READ_OPS.has(operation) ? operation : null,
    status,
    code: safeCode,
    duration_ms: Math.max(0, Math.round(durationMs)),
  };
}

module.exports = { diagnosticRecord };
