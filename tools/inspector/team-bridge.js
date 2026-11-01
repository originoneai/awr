/**
 * Team Web bridge helpers (WS-044).
 * Demo fixtures when not live; live `--team-url` proxies to awr-server `/v1/web`
 * (cookie session, authorized query/command store, durable receipts).
 */
'use strict';

const { mapObservation, createGithubObserver } = require('./team-progress');
const { OBSERVATION_STALE_AFTER_MS, READ_TIMEOUT_MS, OVERVIEW_DEADLINE_MS, resolveCurrency } = require('./observation-contract');

const fs = require('fs');
const path = require('path');

/** Owned Inspector path so browsers attach the session to /api/team/* routes. */
const OWNED_COOKIE_PATH = '/api/team';
const UPSTREAM_COOKIE_PATH = '/v1/web';
const INBOX_PAGE_SIZE = 20;
const INBOX_READ_OPS = new Set(['work.prepare', 'work.observe', 'work.recovery',
  'review.inspect', 'delivery.neutral.inspect', 'delivery.integration.inspect', 'delivery.source.status']);

/**
 * Bounded read-only observation adapter (AWR-OBS-001). Every implemented Team
 * query is reachable except the deferred full ops audit export
 * (audit.history/audit.export/audit.count); commands are never exposed. The
 * selector, paging and budget rules mirror the authoritative upstream
 * WorkstreamQuery validation so bad requests fail here with an explainable
 * error; the upstream service reauthorizes the caller's own cookie session on
 * every forwarded read. Selector values are never echoed in errors.
 */
// Deferred full ops audit (W13): the only upstream queries this adapter must
// never expose. The drift guard test pins READ_OPS against the service list.
const READ_DEFERRED_OPS = new Set(['audit.history', 'audit.export', 'audit.count']);
const READ_PROJECT_WIDE = new Set(['capabilities', 'workstreams.list', 'work.inbox']);
const READ_WORK_LIST = new Set(['work.list', 'work.search', 'planning.outcome']);
const READ_AUDIT = new Set(['audit.requests', 'audit.development']);
const READ_EXACT_WORK = new Set(['task.assignees', 'delivery.neutral.inspect',
  'delivery.submission.describe', 'delivery.neutral.outcome', 'delivery.source.status',
  'delivery.integration.inspect']);
const READ_WORK_SELECTION = new Set(['work.prepare', 'work.admission', 'work.snapshot',
  'work.observe', 'work.recovery', 'command.inspect', 'claim.inspect', 'execution.inspect',
  'handoff.inspect', 'evidence.inspect', 'review.inspect', 'completion.inspect',
  'delivery.inspect', 'artifact.content', 'artifact.segment', 'delivery.exports']);
const READ_PAGED = new Set(['workstreams.list', 'work.list', 'work.search', 'events.list',
  'work.next', 'work.inbox', 'task.assignees', 'delivery.exports']);
const READ_CONTEXT_BUDGET = new Set(['work.prepare', 'work.snapshot', 'work.observe', 'work.inbox',
  'source.content', 'artifact.content', 'artifact.segment', 'review.inspect',
  'delivery.neutral.inspect', 'delivery.submission.describe']);
const READ_REQUIRED_SELECTOR = {
  'command.inspect': 'request_id',
  'planning.outcome': 'request_id',
  'delivery.neutral.outcome': 'request_id',
  'delivery.integration.inspect': 'request_id',
  'claim.inspect': 'claim_id',
  'execution.inspect': 'execution_id',
  'handoff.inspect': 'handoff_id',
  'evidence.inspect': 'evidence_id',
  'review.inspect': 'review_round_id',
  'source.content': 'source_path',
  'session.inspect': 'session_id',
};
const READ_OPS = new Set([
  ...READ_PROJECT_WIDE, ...READ_WORK_LIST, ...READ_AUDIT, ...READ_EXACT_WORK,
  ...READ_WORK_SELECTION, ...READ_CONTEXT_BUDGET, ...READ_PAGED,
  ...Object.keys(READ_REQUIRED_SELECTOR),
]);
const READ_ID_KEYS = new Set(['work_id', 'session_id', 'request_id', 'claim_id', 'execution_id',
  'handoff_id', 'evidence_id', 'review_round_id', 'artifact_id', 'export_id', 'workstream_id',
  'change_id', 'member_actor_id', 'category']);
const READ_RESPONSE_LIMIT = { default: 1048576, 'artifact.content': 8388608, 'artifact.segment': 65536 };

/**
 * Single-object reads whose query names an object identity must echo it back
 * (AWR-OBS-002): a reply that omits the echo cannot be attributed to the
 * requested object and is refused. Each entry maps a response path inside
 * data to the query selector it must equal. work.admission and work.recovery
 * carry no object echo in their upstream response today, so they stay
 * attributable only by their request binding (server-side follow-up).
 */
const READ_ECHOES = {
  'work.prepare': [{ path: ['work_id'], selector: 'work_id' }],
  'work.snapshot': [{ path: ['work_id'], selector: 'work_id' }],
  'work.observe': [{ path: ['work_id'], selector: 'work_id' }],
  'review.inspect': [
    { path: ['review', 'work_id'], selector: 'work_id' },
    { path: ['review', 'round_id'], selector: 'review_round_id' },
  ],
  'evidence.inspect': [
    { path: ['evidence', 'work_id'], selector: 'work_id' },
    { path: ['evidence', 'evidence_id'], selector: 'evidence_id' },
  ],
  'execution.inspect': [
    { path: ['work_id'], selector: 'work_id' },
    { path: ['execution_id'], selector: 'execution_id' },
    { path: ['claim_id'], selector: 'claim_id' },
  ],
  'claim.inspect': [{ path: ['claim_id'], selector: 'claim_id' }],
  'artifact.content': [
    { path: ['work_id'], selector: 'work_id' },
    { path: ['artifact_id'], selector: 'artifact_id' },
  ],
  // Segments require their artifact, but work attribution stays at the
  // request binding; completion.inspect likewise has no work echo and is
  // bound by the UI through selected_completion_id instead.
  'artifact.segment': [{ path: ['artifact_id'], selector: 'artifact_id' }],
  'delivery.inspect': [{ path: ['delivery', 'work_id'], selector: 'work_id' }],
  'delivery.neutral.inspect': [{ path: ['work_id'], selector: 'work_id' }],
  'delivery.submission.describe': [{ path: ['work_id'], selector: 'work_id' }],
  'delivery.neutral.outcome': [{ path: ['work_id'], selector: 'work_id' }],
  'delivery.source.status': [{ path: ['work_id'], selector: 'work_id' }],
  'delivery.integration.inspect': [{ path: ['integration_id'], selector: 'request_id' }],
};

function invalidRead(message) {
  return { error: { code: 'InvalidInput', message } };
}

function isBoundedId(value) {
  return typeof value === 'string' && value.length > 0 && value.length <= 128
    && !/[\x00-\x1f\x7f]/.test(value);
}

function validateReadQuery(query) {
  if (!query || Array.isArray(query) || typeof query !== 'object') {
    return invalidRead('One bounded read query required');
  }
  if (query.protocol_version !== 1) return invalidRead('Unsupported read protocol version');
  const op = query.op;
  if (typeof op !== 'string' || !READ_OPS.has(op)) {
    return { error: { code: 'Unsupported', message: 'Read operation is not exposed by this adapter' } };
  }
  // Key allow-list: identity and authority fields can never ride a read body.
  const allowed = new Set(['protocol_version', 'op']);
  const allowIds = (...keys) => keys.forEach((key) => allowed.add(key));
  if (READ_PROJECT_WIDE.has(op)) {
    // Navigation reads are selector-free.
  } else if (READ_WORK_LIST.has(op)) {
    allowIds('workstream_id');
  } else if (READ_AUDIT.has(op)) {
    allowIds('work_id', 'change_id', 'member_actor_id', 'category', 'request_id');
    allowed.add('include_denies');
  } else if (READ_EXACT_WORK.has(op)) {
    allowIds('work_id', 'workstream_id');
  } else if (op === 'work.next') {
    // work.next is selector-free navigation.
  } else {
    allowIds('work_id', 'workstream_id', 'session_id');
  }
  if (READ_PAGED.has(op) || READ_AUDIT.has(op) || op === 'artifact.segment') allowed.add('cursor');
  if (READ_PAGED.has(op) || READ_AUDIT.has(op)) allowed.add('limit');
  if (op === 'work.search' || op === 'task.assignees') allowed.add('search');
  if (READ_CONTEXT_BUDGET.has(op)) allowed.add('max_context_bytes');
  const required = READ_REQUIRED_SELECTOR[op];
  if (required) allowed.add(required);
  if (op === 'artifact.content' || op === 'artifact.segment') allowIds('artifact_id', 'export_id');
  if (['source.content', 'artifact.content', 'artifact.segment'].includes(op)) allowed.add('expected_sha256');
  for (const key of Object.keys(query)) {
    if (!allowed.has(key)) return invalidRead(`Selector ${key} is not valid for ${op}`);
  }
  for (const key of allowed) {
    if (!(key in query)) continue;
    if (READ_ID_KEYS.has(key) && !isBoundedId(query[key])) return invalidRead(`Selector ${key} is invalid`);
  }
  if (required && !(required in query)) return invalidRead(`Selector ${required} is required for ${op}`);
  if (READ_WORK_SELECTION.has(op) && !query.work_id && !query.session_id) {
    return invalidRead(`${op} requires a work or session selector`);
  }
  if (READ_EXACT_WORK.has(op) && !query.work_id) return invalidRead(`${op} requires a work selector`);
  if ('search' in query) {
    const search = query.search;
    if (typeof search !== 'string' || search.length === 0 || search.length > 512
      || /[\x00-\x1f\x7f]/.test(search)) return invalidRead('Selector search is invalid');
  }
  if (op === 'work.search' && query.search === undefined) return invalidRead('Selector search is required');
  if ('cursor' in query) {
    if (typeof query.cursor !== 'string' || query.cursor.length === 0 || query.cursor.length > 4096) {
      return invalidRead('Selector cursor is invalid');
    }
  }
  if ('limit' in query) {
    if (!Number.isInteger(query.limit) || query.limit < 1 || query.limit > 100) {
      return invalidRead('Selector limit must be 1 through 100');
    }
  }
  if ('max_context_bytes' in query) {
    const bound = op === 'artifact.content' ? 1048576 : op === 'artifact.segment' ? 4096 : 262144;
    if (!Number.isInteger(query.max_context_bytes)
      || query.max_context_bytes < 1 || query.max_context_bytes > bound) {
      return invalidRead('Selector max_context_bytes is out of bounds');
    }
  }
  if ('expected_sha256' in query) {
    if (typeof query.expected_sha256 !== 'string' || !/^[0-9a-f]{64}$/.test(query.expected_sha256)) {
      return invalidRead('Selector expected_sha256 must be 64 lowercase hexadecimal characters');
    }
  }
  if (op === 'artifact.segment' && query.expected_sha256 === undefined) {
    return invalidRead('Selector expected_sha256 is required for artifact.segment');
  }
  if (op === 'artifact.content' || op === 'artifact.segment') {
    if (('artifact_id' in query) === ('export_id' in query)) {
      return invalidRead('Exactly one of artifact_id or export_id is required');
    }
  }
  if ('source_path' in query) {
    const path = query.source_path;
    if (typeof path !== 'string' || path.length === 0 || path.length > 512 || path.startsWith('/')
      || path.includes('..') || path.includes('\\') || path.includes(':') || /[\x00-\x1f\x7f]/.test(path)) {
      return invalidRead('Selector source_path must be a controlled project-relative path');
    }
  }
  if ('include_denies' in query && typeof query.include_denies !== 'boolean') {
    return invalidRead('Selector include_denies must be a boolean');
  }
  return { ok: true };
}

function validReadResponse(reply, op) {
  if (!reply || reply.protocol_version !== 1) return false;
  // capabilities is the one flat discovery response; every other read carries data.
  if (op === 'capabilities') return reply.protocol === 'awr-team-workstream';
  const data = reply.data;
  if (!data || typeof data !== 'object' || Array.isArray(data)) return false;
  if (data.next_cursor != null
    && (typeof data.next_cursor !== 'string' || data.next_cursor.length === 0
      || data.next_cursor.length > 4096)) return false;
  return true;
}

/**
 * AWR-OBS-002 shared read attribution: one semantics for the generic read
 * route and the coordination detail route. The response must carry the
 * standard envelope, belong to the requested work/workstream/scope, and must
 * never grant execution. strictWorkstream additionally pins the response
 * workstream (coordination detail always selects one).
 */
function readAttributionError(query, result, { strictWorkstream } = {}) {
  // work.next is an identity-scoped navigation read: the real upstream
  // response carries only { data, project_revision, protocol_version } — no
  // scope/snapshot attribution envelope exists for it (verified against a
  // live Team service). The standard envelope check therefore does not
  // apply, and nothing is fabricated for it either; its authoritative
  // freshness plane stays not_measured (readFreshness requires the full
  // triple), never a borrowed clock. The response must still be well formed
  // and must never grant execution.
  if (query.op === 'work.next') {
    if (!result || result.protocol_version !== 1) {
      return { code: 'BadGateway', message: 'Invalid Team read response' };
    }
    const data = result.data;
    if (!data || typeof data !== 'object' || Array.isArray(data)) {
      return { code: 'BadGateway', message: 'Invalid Team read response' };
    }
    if (data.execution_authorized === true) {
      return { code: 'BadGateway', message: 'A Team read response never authorizes execution' };
    }
    return null;
  }
  if (!result || result.protocol_version !== 1 || result.scope_id !== 'main'
    || !['source_snapshot_id', 'project_revision', 'coordinator_epoch']
      .every((k) => typeof result[k] === 'string' && result[k])
    || !/^\d+$/.test(result.project_revision)) {
    return { code: 'BadGateway', message: 'Invalid Team read response' };
  }
  const data = result.data;
  if (!data || typeof data !== 'object' || Array.isArray(data)) {
    return { code: 'BadGateway', message: 'Invalid Team read response' };
  }
  if (data.execution_authorized === true) {
    return { code: 'BadGateway', message: 'A Team read response never authorizes execution' };
  }
  if (strictWorkstream && result.workstream_id !== query.workstream_id) {
    return { code: 'BadGateway', message: 'Invalid coordination detail response' };
  }
  if (!strictWorkstream && result.workstream_id != null && query.workstream_id != null
    && result.workstream_id !== query.workstream_id) {
    return { code: 'BadGateway', message: 'Team read response is not for the requested workstream' };
  }
  // Single-object reads must echo the identity their query named; omission
  // cannot be attributed and is refused, not skipped (AWR-OBS-002).
  for (const echo of READ_ECHOES[query.op] || []) {
    if (query[echo.selector] == null) continue;
    let seen = data;
    for (const key of echo.path) {
      seen = seen && typeof seen === 'object' ? seen[key] : undefined;
    }
    if (seen !== query[echo.selector]) {
      return { code: 'BadGateway', message: 'Team read response is not for the requested object' };
    }
  }
  // For every other op a present but mismatched work identity still refuses.
  if (query.work_id != null
    && [data.work_id, data.review && data.review.work_id].some((id) => id != null && id !== query.work_id)) {
    return { code: 'BadGateway', message: 'Team read response is not for the requested work' };
  }
  return null;
}

/**
 * AWR-OBS-002 unified freshness envelope. Three clocks are presented
 * separately and never substitute for each other; each plane is exactly one
 * of measured | stale | failed | not_measured (observation-contract.js):
 * - report_received: when the caller's report was received (progress fields).
 *   Stale is the service's own declaration (progress.stale, computed against
 *   the stale_after_ms the response declares in its reporting block); the
 *   bridge applies that declared bound when the flag is absent. The declared
 *   report_order_basis is surfaced unchanged.
 * - authoritative: the service's own measurement (envelope revision, source
 *   snapshot, coordinator epoch, and the query/observation time when known).
 *   The service declares no bound for it (reporting.stale_after_ms is the
 *   report clock's bound, never this one's), so the bridge always applies
 *   its own declared OBSERVATION_STALE_AFTER_MS — reported next to the
 *   measurement.
 * - page_read: when this bridge read the page (always measured here).
 * A plane that was not measured says not_measured instead of borrowing
 * another clock's value; a plane whose measurement failed says failed with
 * the error code (see failedFreshness) instead of vanishing with the error.
 */
function readFreshness(result, readAtMs) {
  const data = result && result.data && typeof result.data === 'object' && !Array.isArray(result.data)
    ? result.data : null;
  const reporting = (data && (data.reporting || (data.observation && data.observation.reporting))) || null;
  const declaredBound = reporting && Number.isFinite(reporting.stale_after_ms) ? reporting.stale_after_ms : null;
  const progress = (data && ((data.observation && data.observation.progress) || data.progress)) || null;
  const report_received = progress
    && (Number.isFinite(progress.reported_at_unix_ms) || typeof progress.recorded_project_revision === 'string')
    ? {
      ...resolveCurrency({
        serviceStale: progress.stale === true,
        atUnixMs: Number.isFinite(progress.reported_at_unix_ms) ? progress.reported_at_unix_ms : null,
        readAtMs,
        staleAfterMs: declaredBound,
      }),
      ...(Number.isFinite(progress.reported_at_unix_ms) ? { at_unix_ms: progress.reported_at_unix_ms } : {}),
      ...(typeof progress.recorded_project_revision === 'string'
        ? { project_revision: progress.recorded_project_revision } : {}),
      ...(typeof progress.report_order_basis === 'string'
        ? { report_order_basis: progress.report_order_basis } : {}),
    }
    : { status: 'not_measured' };
  const revision = result && typeof result.project_revision === 'string' && /^\d+$/.test(result.project_revision)
    ? result.project_revision : null;
  const source = result && typeof result.source_snapshot_id === 'string' && result.source_snapshot_id
    ? result.source_snapshot_id : null;
  const epoch = result && typeof result.coordinator_epoch === 'string' && result.coordinator_epoch
    ? result.coordinator_epoch : null;
  const queried = Number.isFinite(data && data.snapshot && data.snapshot.queried_at_unix_ms)
    ? data.snapshot.queried_at_unix_ms
    : Number.isFinite(data && data.observed_at_unix_ms) ? data.observed_at_unix_ms
      : Number.isFinite(data && data.observation && data.observation.observed_at_unix_ms)
        ? data.observation.observed_at_unix_ms : null;
  const authoritative = revision && source && epoch
    ? {
      ...resolveCurrency({
        serviceStale: false,
        atUnixMs: queried,
        readAtMs,
        staleAfterMs: OBSERVATION_STALE_AFTER_MS,
      }),
      project_revision: revision, source_snapshot_id: source, coordinator_epoch: epoch,
      ...(queried != null ? { at_unix_ms: queried } : {}),
    }
    : { status: 'not_measured' };
  return { report_received, authoritative, page_read: { status: 'measured', at_unix_ms: readAtMs } };
}

/** The clocks of a read that failed after it was attempted. */
function failedFreshness(errorCode, readAtMs) {
  return {
    report_received: { status: 'failed', error_code: errorCode },
    authoritative: { status: 'failed', error_code: errorCode },
    page_read: { status: 'measured', at_unix_ms: readAtMs },
  };
}

/** The clocks of a request that was refused before any read was attempted. */
function unmeasuredFreshness(readAtMs) {
  return {
    report_received: { status: 'not_measured' },
    authoritative: { status: 'not_measured' },
    page_read: { status: 'measured', at_unix_ms: readAtMs },
  };
}

/** A composition never carries a caller report; its failure is authoritative-side. */
function compositionFailedFreshness(errorCode, readAtMs) {
  return {
    report_received: { status: 'not_measured' },
    authoritative: { status: 'failed', error_code: errorCode },
    page_read: { status: 'measured', at_unix_ms: readAtMs },
  };
}

/** Auth/permission failures clear restricted content; they never retain it. */
const READ_AUTH_FAILURES = new Set(['Unauthenticated', 'SessionExpired', 'Forbidden']);

function inboxSelector(query) {
  if (!query || Array.isArray(query) || query.protocol_version !== 1 || !INBOX_READ_OPS.has(query.op)) return false;
  const extra = query.op === 'review.inspect' ? 'review_round_id'
    : query.op === 'delivery.integration.inspect' ? 'request_id' : null;
  const keys = ['protocol_version', 'op', 'work_id', 'workstream_id', ...(extra ? [extra] : [])];
  return Object.keys(query).length === keys.length && keys.every(key => key in query)
    && ['work_id', 'workstream_id', ...(extra ? [extra] : [])]
      .every(key => typeof query[key] === 'string' && query[key].length > 0
        && query[key].length <= 128 && !/[\x00-\x1f\x7f]/.test(query[key]));
}

function validInboxPage(reply, cursor) {
  const data = reply?.data;
  return reply?.protocol_version === 1 && reply.scope_id === 'main'
    && ['source_snapshot_id', 'project_revision', 'coordinator_epoch'].every(k => typeof reply[k] === 'string' && reply[k])
    && /^\d+$/.test(reply.project_revision)
    && typeof data?.identity?.actor_id === 'string' && data.identity.actor_id
    && typeof data.identity.client_id === 'string' && data.identity.client_id
    && data.state_basis === 'current_persistent_facts' && data.deduplicate_by === 'item_key'
    && data.refresh_from_first_page_on_change === true && data.execution_authorized === false
    && Array.isArray(data.items) && data.items.length <= INBOX_PAGE_SIZE
    && (data.next_cursor === null || typeof data.next_cursor === 'string'
      && data.next_cursor.length > 0 && data.next_cursor.length <= 4096 && data.next_cursor !== cursor)
    && data.items.every(item => typeof item?.item_key === 'string' && item.item_key
      && typeof item.contract_hash === 'string' && item.contract_hash
      && typeof item.title === 'string' && [...item.title].length <= 160
      && typeof item.title_truncated === 'boolean' && inboxSelector(item.next_query)
      && item.next_query.work_id === item.work_id && item.next_query.workstream_id === item.workstream_id
      && item.guidance?.action?.op === item.next_query.op
      && inboxSelector(item.guidance.action.query)
      && Object.keys(item.next_query).every(k => item.guidance.action.query[k] === item.next_query[k])
      && typeof item.guidance.code === 'string' && typeof item.guidance.when === 'string'
      && Array.isArray(item.guidance.because) && item.guidance.because.every(v => typeof v === 'string')
      && typeof item.guidance.recheck_on === 'string');
}

function asObject(body) {
  if (body == null) return {};
  if (typeof body === 'object') return body;
  try { return JSON.parse(body || '{}'); } catch { return null; }
}

/**
 * Rewrite upstream Set-Cookie Path=/v1/web → Path=/api/team at the proxy
 * boundary (including Max-Age=0 clears). Upstream Path would not be sent to
 * Inspector /api/team/projects, logout, or revoke.
 */
function rewriteOwnedCookiePath(cookie) {
  const raw = String(cookie);
  if (/;\s*Path=\/v1\/web(?=;|$)/i.test(raw)) {
    return raw.replace(/;\s*Path=\/v1\/web(?=;|$)/gi, `; Path=${OWNED_COOKIE_PATH}`);
  }
  // Upstream omitted Path or used another value — still pin to owned routes.
  if (/;\s*Path=/i.test(raw)) {
    return raw.replace(/;\s*Path=[^;]*/i, `; Path=${OWNED_COOKIE_PATH}`);
  }
  return `${raw}; Path=${OWNED_COOKIE_PATH}`;
}

function applyProxiedCookies(res, setCookie) {
  if (!res || !setCookie || !setCookie.length) return;
  const rewritten = setCookie.map(rewriteOwnedCookiePath);
  res.setHeader('set-cookie', rewritten.length === 1 ? rewritten[0] : rewritten);
}

function createTeamBridge(opts) {
  const live = Boolean(opts.teamUrl);
  const observeGithub = createGithubObserver(opts.githubFetch);
  // One page may wait readTimeoutMs; the whole overview composition answers
  // within overviewDeadlineMs, declaring whatever is left unread as failed.
  const readTimeoutMs = Number.isInteger(opts.readTimeoutMs) && opts.readTimeoutMs > 0
    ? opts.readTimeoutMs : READ_TIMEOUT_MS;
  const overviewDeadlineMs = Number.isInteger(opts.overviewDeadlineMs) && opts.overviewDeadlineMs > 0
    ? opts.overviewDeadlineMs : OVERVIEW_DEADLINE_MS;
  // Internal clock for deadline arithmetic only (tests inject a fake one so a
  // timer that fires a millisecond early cannot race the deadline check).
  const now = typeof opts.now === 'function' ? opts.now : Date.now;
  let publicUrl = null;
  if (live) {
    const url = new URL(opts.teamPublicUrl || opts.teamUrl);
    if (!['http:', 'https:'].includes(url.protocol) || url.username || url.password ||
        url.search || url.hash || !/^[/a-zA-Z0-9._-]*$/.test(url.pathname)) {
      throw new Error('Team public URL must be an HTTP(S) base URL without credentials, query or fragment');
    }
    publicUrl = url.href.replace(/\/$/, '');
  }

  // Fixtures are demo-only: never when --team-url is set.
  const demoMode = !live && opts.demo !== false;
  const TEAM = {
    url: opts.teamUrl || null,
    live,
    demoMode,
    port: opts.port,
    fixtureDir:
      opts.teamFixtureDir ||
      path.resolve(__dirname, '../../tests/fixtures/workstreams/team-web-loop'),
    sessions: new Map(),
    receipts: new Map(),
  };

  function readFixture(name) {
    const file = path.join(TEAM.fixtureDir, name);
    return JSON.parse(fs.readFileSync(file, 'utf8'));
  }

  function demoSessionCookie(id) {
    return `awr_web_session=${id}; HttpOnly; Path=${OWNED_COOKIE_PATH}; SameSite=Strict`;
  }

  function parseTeamCookie(req) {
    const raw = req.headers.cookie || '';
    for (const part of raw.split(';')) {
      const p = part.trim();
      if (p.startsWith('awr_web_session=')) {
        const id = p.slice('awr_web_session='.length).trim();
        if (/^[A-Za-z0-9_-]{1,200}$/.test(id)) return id;
      }
    }
    return null;
  }

  function teamAuth(req) {
    const id = parseTeamCookie(req);
    if (!id) return null;
    const session = TEAM.sessions.get(id);
    if (!session || session.revoked || session.expires_at_ms <= Date.now()) return null;
    return session;
  }

  async function proxyTeam(reqPath, req, body, methodOverride, signal) {
    if (!TEAM.url) return null;
    const method = methodOverride || req.method || 'GET';
    const headers = {
      'content-type': 'application/json',
      'x-awr-web': '1',
      origin: `http://127.0.0.1:${TEAM.port}`,
    };
    if (req.headers && req.headers.cookie) headers.cookie = req.headers.cookie;
    const payload =
      body == null || method === 'GET' || method === 'HEAD'
        ? undefined
        : typeof body === 'string'
          ? body
          : JSON.stringify(body);
    const res = await fetch(String(TEAM.url).replace(/\/$/, '') + reqPath, {
      method,
      headers,
      body: payload,
      signal,
    });
    const text = await res.text();
    let json;
    try {
      json = JSON.parse(text);
    } catch {
      json = { ok: false, error: { code: 'BadGateway', message: text.slice(0, 200) } };
    }
    const setCookie =
      typeof res.headers.getSetCookie === 'function' ? res.headers.getSetCookie() : [];
    return { status: res.status, json, setCookie };
  }

  function liveError(proxied) {
    const body = proxied && proxied.json;
    if (body && body.code) {
      return { ok: false, error: { code: body.code, message: body.message || body.code } };
    }
    if (body && body.error) {
      return { ok: false, error: body.error };
    }
    return {
      ok: false,
      error: {
        code: proxied && proxied.status === 401 ? 'Unauthenticated' : proxied && proxied.status === 403 ? 'Forbidden' : 'UpstreamError',
        message: (body && body.message) || 'upstream request failed',
      },
    };
  }

  const ACTION_OP = {
    accept_responsibility: 'handoff.accept',
    select_agent: 'claim.acquire',
    respond_blocker: 'session.checkpoint',
    handoff_receive: 'handoff.accept',
    submit_review: 'delivery.submit_and_request_review',
    rework: 'work.rework',
    accept: 'review.accept',
  };

  function mapWorksFromQuery(data) {
    const items = (data && data.items) || (data && data.works) || [];
    if (!Array.isArray(items)) return [];
    return items.map((item) => {
      if (item && item.key) return item;
      const key = (item && (item.external_key || item.work_id || item.id)) || 'unknown';
      return {
        key: String(key),
        title: (item && (item.title || item.name)) || String(key),
        owner_person: (item && item.owner_person) || null,
        agent: (item && item.agent) || null,
        outcome: (item && item.outcome) || null,
        status: (item && (item.status || item.state)) || 'unknown',
        blocker: (item && item.blocker) || null,
        next_step: (item && item.next_step) || null,
        depends_on: (item && item.depends_on) || [],
        hidden_deps: (item && item.hidden_deps) || [],
        capabilities: (item && item.capabilities) || {},
        contract_hash: item && item.contract_hash,
        detail_loaded: false,
      };
    });
  }

  /**
   * Follow one scoped list query to the end of its pages, bounded per page
   * and by the composition deadline. The result declares its own coverage:
   * the server-reported total when one exists, a truncation marker with the
   * resume cursor when the page cap is hit (never silently counted as all),
   * and the pages' shared as-of triple — null when the pages carry no
   * envelope or mix revisions, so a mixed or unmeasured composition can
   * never claim one authoritative version. A failure keeps the confirmed
   * pages: the caller declares the part failed with the error code, what was
   * read, its as-of and the cursor that resumes the failed page.
   */
  async function queryAllPages(project, query, req, res, { pageTimeoutMs = 4000, deadlineAt = null } = {}) {
    const items = [];
    const seen = new Set();
    let cursor;
    let total = null;
    let asOf;
    let mixed = false;
    const failed = (code, message) => ({
      error: { code, message },
      items,
      total,
      truncated: false,
      next_cursor: cursor || null,
      as_of: mixed ? null : asOf || null,
    });
    for (let page = 0; page < 100; page += 1) {
      const remaining = deadlineAt == null ? pageTimeoutMs : Math.min(pageTimeoutMs, deadlineAt - now());
      if (remaining <= 0) {
        return failed('ReadTimeout', 'Scoped list read exceeded the composition deadline; retry this read');
      }
      const signal = AbortSignal.timeout(remaining);
      let upstream;
      try {
        upstream = await proxyTeam(
          `/v1/web/projects/${encodeURIComponent(project)}/query`, req,
          { protocol_version: 1, ...query, limit: 100, ...(cursor ? { cursor } : {}) },
          'POST', signal
        );
      } catch (error) {
        return signal.aborted
          ? failed('ReadTimeout', 'Scoped list read timed out; retry this read')
          : failed('BadGateway', 'Scoped list service is unreachable');
      }
      if (!upstream) return failed('BadGateway', 'live proxy unavailable');
      applyProxiedCookies(res, upstream.setCookie);
      if (upstream.status >= 400) {
        const failure = liveError(upstream);
        return failed(failure.error.code, failure.error.message);
      }
      const data = upstream.json && (upstream.json.data || upstream.json);
      if (!data || !Array.isArray(data.items)) {
        return failed('BadGateway', 'invalid Team query page');
      }
      const triple = ['project_revision', 'source_snapshot_id', 'coordinator_epoch']
        .every((k) => upstream.json && typeof upstream.json[k] === 'string' && upstream.json[k])
        ? {
          project_revision: upstream.json.project_revision,
          source_snapshot_id: upstream.json.source_snapshot_id,
          coordinator_epoch: upstream.json.coordinator_epoch,
        }
        : null;
      if (page === 0) asOf = triple;
      else if (JSON.stringify(triple) !== JSON.stringify(asOf)) mixed = true;
      if (Number.isInteger(data.total) && data.total >= 0) total = data.total;
      items.push(...data.items);
      const next = data.next_cursor;
      if (next == null) {
        return { items, total, truncated: false, next_cursor: null, as_of: mixed ? null : asOf };
      }
      if (typeof next !== 'string' || !next || seen.has(next)) {
        return failed('BadGateway', 'invalid Team query cursor');
      }
      seen.add(next);
      cursor = next;
    }
    return { items, total, truncated: true, next_cursor: cursor, as_of: mixed ? null : asOf };
  }

  async function inboxRead(project, query, req, res, maxBytes) {
    const signal = AbortSignal.timeout(readTimeoutMs);
    try {
      const upstream = await proxyTeam(`/v1/web/projects/${encodeURIComponent(project)}/query`, req, query, 'POST', signal);
      applyProxiedCookies(res, upstream?.setCookie);
      if (!upstream || upstream.status >= 400 || upstream.json?.ok === false || upstream.json?.code || upstream.json?.error)
        return liveError(upstream);
      if (Buffer.byteLength(JSON.stringify(upstream.json), 'utf8') > maxBytes)
        return { ok: false, error: { code: 'ResponseTooLarge', message: 'Coordination read exceeded its response limit' } };
      return { ok: true, result: upstream.json };
    } catch (error) {
      return { ok: false, error: { code: signal.aborted ? 'ReadTimeout' : 'BadGateway',
        message: signal.aborted ? 'Coordination read timed out; retry this read' : 'Coordination service is unreachable' } };
    }
  }

  const routes = {
    'GET /api/team/projects': async (url, _body, req, res) => {
      if (TEAM.live) {
        const session = await proxyTeam('/v1/web/session', req, null, 'GET');
        if (!session || session.status >= 400) return liveError(session);
        applyProxiedCookies(res, session.setCookie);
        const proxied = await proxyTeam('/v1/web/projects', req, null, 'GET');
        if (!proxied) {
          return { ok: false, error: { code: 'BadGateway', message: 'live proxy unavailable' } };
        }
        applyProxiedCookies(res, proxied.setCookie);
        if (proxied.status >= 400) return liveError(proxied);
        return { ...proxied.json, session: session.json };
      }
      if (!TEAM.demoMode) {
        return { ok: false, error: { code: 'DemoDisabled', message: 'fixtures require demo mode' } };
      }
      const view = url.searchParams.get('view') === 'personal' ? 'personal-view.json' : 'team-view.json';
      const data = readFixture(view);
      const session =
        teamAuth(req) || { session_id: 'demo-anonymous', expires_at_ms: Date.now() + 3600000 };
      return { ok: true, projects: data.projects, session, view: data.view };
    },

    'GET /api/team/overview': async (url, _body, req, res) => {
      if (TEAM.live) {
        const project = url.searchParams.get('project');
        if (!project) {
          return { ok: false, error: { code: 'InvalidInput', message: 'project required' },
            freshness: unmeasuredFreshness(Date.now()) };
        }
        const session = await proxyTeam('/v1/web/session', req, null, 'GET');
        if (!session) {
          return { ok: false, error: { code: 'BadGateway', message: 'live proxy unavailable' },
            freshness: compositionFailedFreshness('BadGateway', Date.now()) };
        }
        applyProxiedCookies(res, session.setCookie);
        if (session.status >= 400) {
          const failure = liveError(session);
          return { ...failure, freshness: compositionFailedFreshness(failure.error.code, Date.now()) };
        }

        // A member with multiple grants must select a workstream for work.list.
        // Discover only authorized streams and retain that scope on every page.
        // One deadline bounds the whole composition: whatever is left unread
        // when it expires is declared failed, never silently dropped.
        const deadlineAt = now() + overviewDeadlineMs;
        const streams = await queryAllPages(project, { op: 'workstreams.list' }, req, res,
          { pageTimeoutMs: readTimeoutMs, deadlineAt });
        const scopeReadAt = Date.now();
        if (streams.error && (READ_AUTH_FAILURES.has(streams.error.code) || streams.items.length === 0)) {
          // Identity or permission revocation clears restricted content at
          // once; with nothing confirmed there is nothing to retain either.
          return { ok: false, error: streams.error,
            freshness: compositionFailedFreshness(streams.error.code, Date.now()) };
        }
        // A failed scope read keeps its confirmed streams, declared failed
        // with the resume cursor below; it never counts as the whole scope.
        const scopeCoverage = {
          status: streams.error ? 'failed' : 'succeeded',
          ...(streams.error ? { error_code: streams.error.code } : {}),
          enumerated: streams.items.length,
          truncated: streams.truncated || Boolean(streams.error),
          next_cursor: streams.truncated || streams.error ? streams.next_cursor : null,
          read_at_unix_ms: scopeReadAt,
        };
        const works = [];
        const parts = [];
        for (const stream of streams.items) {
          if (!stream || typeof stream.id !== 'string' || !stream.id) {
            return { ok: false, error: { code: 'BadGateway', message: 'invalid Team workstream' },
              freshness: compositionFailedFreshness('BadGateway', Date.now()) };
          }
          const page = await queryAllPages(
            project, { op: 'work.list', workstream_id: stream.id }, req, res,
            { pageTimeoutMs: readTimeoutMs, deadlineAt }
          );
          if (page.error) {
            // Identity or permission revocation clears restricted content at
            // once; nothing already read is retained or returned.
            if (READ_AUTH_FAILURES.has(page.error.code)) {
              return { ok: false, error: page.error,
                freshness: compositionFailedFreshness(page.error.code, Date.now()) };
            }
            // A non-auth partial failure keeps the confirmed pages: the part
            // says failed with the error code, what was read, its as-of and
            // the cursor that resumes the failed page.
            parts.push({
              workstream_id: stream.id, status: 'failed', error_code: page.error.code,
              works_enumerated: page.items.length, works_total: page.total,
              truncated: false, next_cursor: page.next_cursor, as_of: page.as_of,
              read_at_unix_ms: Date.now(),
            });
            works.push(...mapWorksFromQuery(page).map((work) => ({
              ...work, workstream_id: stream.id,
            })));
            continue;
          }
          parts.push({
            workstream_id: stream.id, status: 'succeeded',
            works_enumerated: page.items.length, works_total: page.total,
            truncated: page.truncated, next_cursor: page.next_cursor, as_of: page.as_of,
            read_at_unix_ms: Date.now(),
          });
          works.push(...mapWorksFromQuery(page).map((work) => ({
            ...work, workstream_id: stream.id,
          })));
        }
        if (parts.length > 0 && parts.every((part) => part.status === 'failed')) {
          // Nothing confirmed to retain: fail closed with the part's reason.
          return { ok: false, error: { code: parts[0].error_code, message: 'No authorized workstream could be read' },
            freshness: compositionFailedFreshness(parts[0].error_code, Date.now()) };
        }
        let capabilities;
        try {
          capabilities = await proxyTeam(`/v1/web/projects/${encodeURIComponent(project)}/query`, req,
            { protocol_version: 1, op: 'capabilities' }, 'POST',
            AbortSignal.timeout(Math.max(1, Math.min(readTimeoutMs, deadlineAt - now()))));
        } catch (error) {
          return { ok: false, error: { code: 'ReadTimeout', message: 'Capabilities read exceeded the composition deadline' },
            freshness: compositionFailedFreshness('ReadTimeout', Date.now()) };
        }
        if (!capabilities || capabilities.status >= 400) {
          const failure = liveError(capabilities);
          return { ...failure, freshness: compositionFailedFreshness(failure.error.code, Date.now()) };
        }
        const identity = capabilities.json && (capabilities.json.data || capabilities.json).identity;
        const succeeded = parts.filter((part) => part.status === 'succeeded');
        const failed = parts.filter((part) => part.status === 'failed');
        // A total exists only when every enumerated part reported one and the
        // authorized scope itself was fully enumerated.
        const worksTotal = failed.length === 0 && !scopeCoverage.truncated
          && succeeded.every((part) => Number.isInteger(part.works_total))
          ? succeeded.reduce((sum, part) => sum + part.works_total, 0)
          : null;
        // One common revision only when every measured part agrees on it.
        const triples = [streams.as_of, ...succeeded.map((part) => part.as_of)];
        const common = triples.length > 0 && triples.every((triple) => triple)
          && new Set(triples.map((triple) => JSON.stringify(triple))).size === 1
          ? triples[0]
          : null;
        return {
          ok: true,
          identity: identity || null,
          project,
          works,
          workstreams: streams.items.map(({ id, external_key, title }) => ({ id, external_key, title })),
          members: [],
          handoffs: [],
          reviews: [],
          coverage: {
            scope: scopeCoverage,
            streams: parts,
            works_enumerated: works.length,
            works_total: worksTotal,
            succeeded: succeeded.length,
            failed: failed.length,
            truncated: streams.truncated || succeeded.some((part) => part.truncated),
          },
          freshness: {
            report_received: { status: 'not_measured' },
            authoritative: common
              ? { status: 'measured', ...common }
              : { status: 'not_measured' },
            page_read: { status: 'measured', at_unix_ms: Date.now() },
          },
          schema: 'awr-team-web-loop-live/v1',
          interaction_mode: 'mcp',
          mcp_url: publicUrl + '/v1/projects/' + encodeURIComponent(project) + '/mcp',
          view_modes: ['team'],
          session: {
            session_id: session.json && session.json.session_id,
            expires_at_ms: session.json && session.json.expires_at_ms,
          },
        };
      }
      if (!TEAM.demoMode) {
        return { ok: false, error: { code: 'DemoDisabled', message: 'fixtures require demo mode' } };
      }
      const view = url.searchParams.get('view') === 'personal' ? 'personal-view.json' : 'team-view.json';
      const data = readFixture(view);
      const fixtureStreams = data.workstreams || [];
      return {
        ok: true,
        project: url.searchParams.get('project') || (data.projects[0] && data.projects[0].key),
        works: data.works,
        workstreams: fixtureStreams,
        members: data.members,
        handoffs: data.handoffs || [],
        reviews: data.reviews || [],
        coverage: {
          scope: { status: 'succeeded', enumerated: fixtureStreams.length, truncated: false,
            next_cursor: null, read_at_unix_ms: Date.now() },
          streams: fixtureStreams.map((stream) => ({
            workstream_id: stream.id, status: 'succeeded',
            works_enumerated: data.works.filter((work) => work.workstream_id === stream.id).length,
            works_total: data.works.filter((work) => work.workstream_id === stream.id).length,
            truncated: false, next_cursor: null, as_of: null, read_at_unix_ms: Date.now(),
          })),
          works_enumerated: data.works.length,
          works_total: data.works.length,
          succeeded: fixtureStreams.length,
          failed: 0,
          truncated: false,
        },
        freshness: {
          report_received: { status: 'not_measured' },
          authoritative: { status: 'not_measured' },
          page_read: { status: 'measured', at_unix_ms: Date.now() },
        },
        schema: data.schema,
      };
    },

    'GET /api/team/inbox': async (url, _body, req, res) => {
      if (!TEAM.live) return { ok: false, error: { code: 'Unsupported', message: 'Live Team coordination required' } };
      const project = url.searchParams.get('project'), cursor = url.searchParams.get('cursor');
      if (!project || project.length > 128 || (cursor != null && (!cursor || cursor.length > 4096))
        || [...url.searchParams.keys()].some(k => !['project', 'cursor'].includes(k)))
        return { ok: false, error: { code: 'InvalidInput', message: 'Project and optional bounded inbox cursor required' },
          freshness: unmeasuredFreshness(Date.now()) };
      const reply = await inboxRead(project, { protocol_version: 1, op: 'work.inbox', limit: INBOX_PAGE_SIZE,
        ...(cursor == null ? {} : { cursor }) }, req, res, 65536);
      if (reply.ok && !validInboxPage(reply.result, cursor))
        return { ok: false, error: { code: 'BadGateway', message: 'Invalid current coordination page' },
          freshness: failedFreshness('BadGateway', Date.now()) };
      if (reply.ok) return { ...reply, freshness: readFreshness(reply.result, Date.now()) };
      return { ...reply, freshness: failedFreshness(reply.error.code, Date.now()) };
    },

    'POST /api/team/inbox-detail': async (_url, body, req, res) => {
      if (!TEAM.live) return { ok: false, error: { code: 'Unsupported', message: 'Live Team coordination required' } };
      const input = asObject(body);
      if (!input || typeof input.project !== 'string' || !input.project || input.project.length > 128
        || !inboxSelector(input.query) || Object.keys(input).some(k => !['project', 'query'].includes(k)))
        return { ok: false, error: { code: 'InvalidInput', message: 'Project and an exact coordination read selector required' },
          freshness: unmeasuredFreshness(Date.now()) };
      // The upstream reauthorizes this read. No command, acknowledgement or provider lookup is issued.
      const reply = await inboxRead(input.project, input.query, req, res, 262144);
      if (reply.ok) {
        // One shared attribution semantics with the generic read route
        // (AWR-OBS-002); the coordination detail always pins its workstream.
        const attribution = readAttributionError(input.query, reply.result, { strictWorkstream: true });
        if (attribution) return { ok: false, error: attribution, freshness: failedFreshness(attribution.code, Date.now()) };
        return { ...reply, query: input.query, freshness: readFreshness(reply.result, Date.now()) };
      }
      return { ...reply, freshness: failedFreshness(reply.error.code, Date.now()) };
    },

    'POST /api/team/read': async (_url, body, req, res) => {
      if (!TEAM.live) return { ok: false, error: { code: 'Unsupported', message: 'Live Team coordination required' } };
      const input = asObject(body);
      if (!input || typeof input.project !== 'string' || !input.project || input.project.length > 128
        || Object.keys(input).some(k => !['project', 'query'].includes(k)))
        return { ok: false, error: { code: 'InvalidInput', message: 'Project and one bounded read query required' },
          freshness: unmeasuredFreshness(Date.now()) };
      const checked = validateReadQuery(input.query);
      if (checked.error) return { ok: false, error: checked.error, freshness: unmeasuredFreshness(Date.now()) };
      // The upstream reauthorizes this read against the caller's own cookie
      // session; the adapter adds no identity and grants no authority itself.
      const maxBytes = READ_RESPONSE_LIMIT[input.query.op] || READ_RESPONSE_LIMIT.default;
      const reply = await inboxRead(input.project, input.query, req, res, maxBytes);
      if (reply.ok && !validReadResponse(reply.result, input.query.op))
        return { ok: false, error: { code: 'BadGateway', message: 'Invalid Team read response' },
          freshness: failedFreshness('BadGateway', Date.now()) };
      // One shared attribution semantics with the coordination detail route
      // (AWR-OBS-002): the response must belong to the requested
      // work/workstream/scope and must never grant execution. capabilities is
      // the one flat discovery response and carries no attributable object.
      if (reply.ok && input.query.op !== 'capabilities') {
        const attribution = readAttributionError(input.query, reply.result, { strictWorkstream: false });
        if (attribution) return { ok: false, error: attribution, freshness: failedFreshness(attribution.code, Date.now()) };
      }
      if (reply.ok) return { ...reply, query: input.query, freshness: readFreshness(reply.result, Date.now()) };
      return { ...reply, freshness: failedFreshness(reply.error.code, Date.now()) };
    },

    'GET /api/team/work': async (url, _body, req, res) => {
      const result = await workDetail(url, req, res);
      // A failed detail read keeps its clocks: the planes that could not be
      // measured say failed with the error code instead of vanishing.
      if (result && result.ok === false && result.error && !result.freshness) {
        return { ...result, freshness: failedFreshness(result.error.code, Date.now()) };
      }
      return result;
    },
  };

  async function workDetail(url, req, res) {
    if (!TEAM.live) return { ok: false, error: { code: 'Unsupported', message: 'Live Team detail required' } };
    const project = url.searchParams.get('project');
    const work = url.searchParams.get('work');
    const stream = url.searchParams.get('workstream');
    if (!project || !work || !stream) return { ok: false, error: { code: 'InvalidInput', message: 'project, work and workstream required' },
      freshness: unmeasuredFreshness(Date.now()) };
    // Bound the entire detail read, including legacy fallback. This signal
    // is never applied to mutating commands with possibly unknown outcomes.
    const readSignal = AbortSignal.timeout(readTimeoutMs);
    const query = async op => {
      try { return await proxyTeam(`/v1/web/projects/${encodeURIComponent(project)}/query`, req, {
      protocol_version: 1, op, work_id: work, workstream_id: stream,
      ...(op === 'work.snapshot' ? { max_context_bytes: 262144 } : {}),
      }, 'POST', readSignal); } catch (error) {
        if (!readSignal.aborted) throw error;
        return { status: 504, json: { code: 'ReadTimeout', message: 'Scoped work detail timed out; retry this read' } };
      }
    };
    const unsupported = reply => [400, 501].includes(reply?.status)
      && (reply.json?.code === 'Unsupported' || reply.json?.error?.code === 'Unsupported');
    let upstream = await query('work.snapshot');
    applyProxiedCookies(res, upstream?.setCookie);
    const legacy = unsupported(upstream);
    if (legacy) upstream = await query('work.prepare');
    if (!upstream || upstream.status >= 400) return liveError(upstream);
    applyProxiedCookies(res, upstream.setCookie);
    const data = upstream.json && upstream.json.data;
    if (!data || data.work_id !== work || !data.visible_contract || upstream.json.workstream_id !== stream) {
      return { ok: false, error: { code: 'BadGateway', message: 'Invalid scoped Team work detail' } };
    }
    const expected = url.searchParams.get('contract');
    if (expected && expected !== data.contract_hash) {
      return { ok: false, error: { code: 'SourceChanged', message: 'Work contract changed; refresh the overview' } };
    }
    const observed = legacy ? await query('work.observe') : upstream;
    if (!observed) return liveError(observed);
    if (legacy) applyProxiedCookies(res, observed.setCookie);
    let progress = { observation_available: false, observation_error: 'unsupported' };
    let snapshot = { consistency: 'unconfirmed_legacy' };
    if (observed.status >= 400) {
      if (!legacy || !unsupported(observed)) return liveError(observed);
    } else {
      const observation = legacy ? observed.json?.data : data.observation;
      if (!observation || observation.work_id !== work || observed.json.workstream_id !== stream
        || observation.contract_hash !== data.contract_hash || !Number.isFinite(observation.observed_at_unix_ms)) {
        return { ok: false, error: { code: legacy ? 'SourceChanged' : 'BadGateway', message: 'Invalid scoped observation; refresh the project' } };
      }
      if (!legacy) {
        const meta = data.snapshot, envelope = upstream.json;
        const fields = ['state', 'work_version', 'last_fence', 'recovery_blocked', 'selected_completion_id'];
        const validRuntime = value => value && !Array.isArray(value)
          && typeof value.state === 'string' && value.state
          && typeof value.work_version === 'string' && /^\d+$/.test(value.work_version)
          && typeof value.last_fence === 'string' && /^\d+$/.test(value.last_fence)
          && typeof value.recovery_blocked === 'boolean'
          && (value.selected_completion_id === null || typeof value.selected_completion_id === 'string');
        const runtimeMatches = data.runtime === null && observation.runtime === null
          || validRuntime(data.runtime) && validRuntime(observation.runtime)
            && fields.every(k => data.runtime[k] === observation.runtime[k]);
        if (meta?.version !== 1 || meta.consistency !== 'repeatable_read'
          || typeof meta.project_revision !== 'string' || !/^\d+$/.test(meta.project_revision)
          || meta.project_revision !== envelope.project_revision
          || typeof meta.source_snapshot_id !== 'string' || !meta.source_snapshot_id
          || meta.source_snapshot_id !== envelope.source_snapshot_id
          || typeof meta.coordinator_epoch !== 'string' || !meta.coordinator_epoch
          || meta.coordinator_epoch !== envelope.coordinator_epoch
          || meta.queried_at_unix_ms !== observation.observed_at_unix_ms
          || typeof data.context_complete !== 'boolean' || !runtimeMatches) {
          return { ok: false, error: { code: 'BadGateway', message: 'Invalid atomic Team snapshot' } };
        }
        snapshot = meta;
      }
      progress = mapObservation(observation);
      progress.github = observeGithub.background(progress.pr_reference);
    }
    // prepare may omit its optional hint to respect the caller's byte budget.
    // Its required completeness fact still takes priority over observe advice.
    if (data.context_complete === false) {
      progress.guidance = data.guidance?.code === 'restore_context' ? data.guidance
        : { code: 'restore_context', action: { op: 'work.prepare', note: data.next_step || null } };
      progress.next_step = progress.guidance.action?.note || null;
    }
    // Three clocks side by side (AWR-OBS-002): the caller's report receipt,
    // the service's own measurement and this page read — never substituted.
    const freshness = readFreshness(
      legacy ? (observed.json && observed.json.data ? observed.json : upstream.json) : upstream.json,
      Date.now());
    return { ok: true, freshness, work: {
      key: work, workstream_id: stream, contract_hash: data.contract_hash,
      detail_loaded: true, status: data.runtime ? data.runtime.state : null,
      runtime_available: Boolean(data.runtime),
      recovery_blocked: Boolean(data.runtime && data.runtime.recovery_blocked),
      acceptance: data.visible_contract.acceptance || [],
      goals: data.visible_contract.goals || [],
      depends_on: (data.visible_contract.required_dependencies || []).map((key) => ({ key, visible: true })),
      dependency_export_unavailable: data.dependency_export_unavailable === true,
      context_complete: data.context_complete === true,
      completeness_reasons: data.completeness_reasons || [],
      next_step: data.next_step || null,
      execution_admission: data.execution_admission || 'not_evaluated',
      ...progress,
      snapshot,
    } };
  }

  Object.assign(routes, {
    'POST /api/team/access': async (_url, body, req, res) => {
      if (!TEAM.live) return { ok: false, error: { code: 'Unsupported', message: 'Live Team access required' } };
      const input = asObject(body);
      if (!input || typeof input.project !== 'string' || !input.project ||
          !['inspect', 'preview', 'apply', 'outcome'].includes(input.operation) ||
          !input.payload || typeof input.payload !== 'object' || Array.isArray(input.payload)) {
        return { ok: false, error: { code: 'InvalidInput', message: 'Invalid access operation' } };
      }
      const upstream = await proxyTeam(`/v1/web/projects/${encodeURIComponent(input.project)}/access/${input.operation}`,
        req, input.payload, 'POST');
      if (!upstream || upstream.status >= 400) return liveError(upstream);
      applyProxiedCookies(res, upstream.setCookie);
      return { ok: true, data: upstream.json };
    },

    'GET /api/team/activity': async (url, _body, req, res) => {
      if (!TEAM.live) return { ok: false, error: { code: 'Unsupported', message: 'Live Team audit required' } };
      const project = url.searchParams.get('project');
      const kind = url.searchParams.get('kind');
      if (!project || !['requests', 'development'].includes(kind)) {
        return { ok: false, error: { code: 'InvalidInput', message: 'Project and activity kind required' } };
      }
      const query = { protocol_version: 1, op: 'audit.' + kind, limit: 50 };
      for (const key of ['cursor', 'member_actor_id', 'work_id']) {
        const value = url.searchParams.get(key);
        if (value) query[key] = value;
      }
      const upstream = await proxyTeam(`/v1/web/projects/${encodeURIComponent(project)}/query`, req, query, 'POST');
      if (!upstream || upstream.status >= 400) return liveError(upstream);
      applyProxiedCookies(res, upstream.setCookie);
      return { ok: true, data: upstream.json.data || upstream.json };
    },

    'POST /api/team/login': async (_url, body, req, res) => {
      if (TEAM.live) {
        const proxied = await proxyTeam('/v1/web/login', req, body, 'POST');
        if (!proxied) {
          return { ok: false, error: { code: 'BadGateway', message: 'live proxy unavailable' } };
        }
        applyProxiedCookies(res, proxied.setCookie);
        if (proxied.status >= 400) return liveError(proxied);
        return proxied.json;
      }
      if (!TEAM.demoMode) {
        return { ok: false, error: { code: 'DemoDisabled', message: 'fixtures require demo mode' } };
      }
      const parsed = asObject(body);
      if (!parsed) {
        return { ok: false, error: { code: 'InvalidInput', message: 'invalid login' } };
      }
      if (!parsed.bearer || typeof parsed.bearer !== 'string' || parsed.bearer.length < 8) {
        return { ok: false, error: { code: 'Forbidden', message: 'access denied' } };
      }
      const id = 'ws_demo_' + Date.now().toString(36);
      const session = {
        session_id: id,
        bearer_present: true,
        expires_at_ms: Date.now() + 8 * 3600 * 1000,
        revoked: false,
      };
      TEAM.sessions.set(id, session);
      res.setHeader('set-cookie', demoSessionCookie(id));
      return {
        ok: true,
        protocol: 'awr-team-web-entry',
        protocol_version: 1,
        session_id: id,
        expires_at_ms: session.expires_at_ms,
        auth: { kind: 'http_only_cookie', bearer_in_page: false },
      };
    },

    'POST /api/team/logout': async (_url, _body, req, res) => {
      if (TEAM.live) {
        const proxied = await proxyTeam('/v1/web/logout', req, '{}', 'POST');
        if (!proxied) {
          return { ok: false, error: { code: 'BadGateway', message: 'live proxy unavailable' } };
        }
        applyProxiedCookies(res, proxied.setCookie);
        if (proxied.status >= 400) return liveError(proxied);
        return proxied.json;
      }
      const id = parseTeamCookie(req);
      if (id && TEAM.sessions.has(id)) TEAM.sessions.get(id).revoked = true;
      res.setHeader(
        'set-cookie',
        `awr_web_session=; HttpOnly; Path=${OWNED_COOKIE_PATH}; SameSite=Strict; Max-Age=0`
      );
      return { ok: true, logged_out: true };
    },

    'POST /api/team/session/revoke': async (_url, body, req, res) => {
      if (TEAM.live) {
        const proxied = await proxyTeam('/v1/web/session/revoke', req, body, 'POST');
        if (!proxied) {
          return { ok: false, error: { code: 'BadGateway', message: 'live proxy unavailable' } };
        }
        applyProxiedCookies(res, proxied.setCookie);
        if (proxied.status >= 400) return liveError(proxied);
        return proxied.json;
      }
      const id = parseTeamCookie(req);
      if (id && TEAM.sessions.has(id)) TEAM.sessions.get(id).revoked = true;
      for (const s of TEAM.sessions.values()) s.revoked = true;
      res.setHeader(
        'set-cookie',
        `awr_web_session=; HttpOnly; Path=${OWNED_COOKIE_PATH}; SameSite=Strict; Max-Age=0`
      );
      return { ok: true, revoked: [{ scope: 'all_mine' }] };
    },

    'POST /api/team/action': async (_url, body, req, res) => {
      const parsed = asObject(body);
      if (!parsed) {
        return { ok: false, error: { code: 'InvalidInput', message: 'invalid action' } };
      }

      if (TEAM.live) {
        if (!parsed.project || typeof parsed.project !== 'string') {
          return { ok: false, error: { code: 'InvalidInput', message: 'project required' } };
        }
        // Live writes must hit the authorized command store — never invent receipts.
        const session = await proxyTeam('/v1/web/session', req, null, 'GET');
        if (!session) {
          return { ok: false, error: { code: 'BadGateway', message: 'live proxy unavailable' } };
        }
        applyProxiedCookies(res, session.setCookie);
        if (session.status >= 400) return liveError(session);

        let commandPayload = parsed.command;
        if (!commandPayload || typeof commandPayload !== 'object') {
          // Allow callers to supply a full command under top-level fields.
          if (parsed.protocol_version && parsed.op && parsed.request_id) {
            commandPayload = { ...parsed };
            delete commandPayload.project;
            delete commandPayload.action;
            delete commandPayload.command;
            delete commandPayload.expired;
          } else {
            return {
              ok: false,
              error: {
                code: 'InvalidInput',
                message:
                  'live action requires a command payload for the authorized command store',
              },
            };
          }
        }
        if (parsed.expired === true) {
          return { ok: false, error: { code: 'ExpiredOperation', message: 'operation expired' } };
        }

        const proxied = await proxyTeam(
          `/v1/web/projects/${encodeURIComponent(parsed.project)}/command`,
          req,
          commandPayload,
          'POST'
        );
        if (!proxied) {
          return { ok: false, error: { code: 'BadGateway', message: 'live proxy unavailable' } };
        }
        applyProxiedCookies(res, proxied.setCookie);
        if (proxied.status >= 400) return liveError(proxied);

        const upstream = proxied.json || {};
        // Uncertain-outcome / exact replay: forward store receipt unchanged.
        return {
          ok: true,
          replayed: Boolean(upstream.replayed),
          receipt: upstream.receipt || null,
          server_op: (upstream.receipt && upstream.receipt.op) || commandPayload.op || null,
          execution_authorized: upstream.execution_authorized,
          upstream,
        };
      }

      if (!TEAM.demoMode) {
        return { ok: false, error: { code: 'DemoDisabled', message: 'fixtures require demo mode' } };
      }

      const allowed = new Set(Object.keys(ACTION_OP));
      if (!allowed.has(parsed.action) || !parsed.request_id || !parsed.work_key) {
        return { ok: false, error: { code: 'InvalidInput', message: 'invalid action fields' } };
      }
      if (parsed.expired === true) {
        return { ok: false, error: { code: 'ExpiredOperation', message: 'operation expired' } };
      }
      const receiptKey = parsed.request_id;
      if (TEAM.receipts.has(receiptKey)) {
        return { ok: true, replayed: true, receipt: TEAM.receipts.get(receiptKey) };
      }
      const receipt = {
        id: 'rcpt_' + receiptKey,
        request_id: receiptKey,
        op: ACTION_OP[parsed.action],
        work_key: parsed.work_key,
        project: parsed.project,
        agent_id: parsed.agent_id || null,
        at_ms: Date.now(),
      };
      TEAM.receipts.set(receiptKey, receipt);
      return { ok: true, replayed: false, receipt, server_op: receipt.op };
    },
  });

  return {
    TEAM,
    routes,
    readFixture,
    teamAuth,
    rewriteOwnedCookiePath,
    OWNED_COOKIE_PATH,
    UPSTREAM_COOKIE_PATH,
  };
}

module.exports = { createTeamBridge, rewriteOwnedCookiePath, READ_OPS, READ_DEFERRED_OPS, READ_ECHOES, readFreshness, readAttributionError };
