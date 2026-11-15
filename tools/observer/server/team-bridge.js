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

/** Owned Observer path so browsers attach the session to /api/team/* routes. */
const OWNED_COOKIE_PATH = '/api/team';
const UPSTREAM_COOKIE_PATH = '/v1/web';
const INBOX_PAGE_SIZE = 20;
const INBOX_READ_OPS = new Set(['work.prepare', 'work.observe', 'work.recovery',
  'review.inspect', 'delivery.neutral.inspect', 'delivery.integration.inspect', 'delivery.source.status']);

/**
 * Bounded read-only observation adapter (AWR-OBS-001). Every implemented Team
 * query is reachable except deferred full ops audit and route setup discovery;
 * commands are never exposed. The
 * selector, paging and budget rules mirror the authoritative upstream
 * WorkstreamQuery validation so bad requests fail here with an explainable
 * error; the upstream service reauthorizes the caller's own cookie session on
 * every forwarded read. Selector values are never echoed in errors.
 */
// Full ops audit is deferred (W13). Route setup belongs to the Agent workflow,
// not this observation surface. The drift guard pins this explicit boundary.
const READ_DEFERRED_OPS = new Set(['audit.history', 'audit.export', 'audit.count', 'delivery.routes']);
const READ_PROJECT_WIDE = new Set(['capabilities', 'workstreams.list', 'work.inbox']);
const READ_WORK_LIST = new Set(['work.list', 'work.graph', 'work.search', 'planning.outcome', 'planning.candidates']);
const READ_AUDIT = new Set(['audit.requests', 'audit.development']);
const READ_EXACT_WORK = new Set(['task.assignees', 'delivery.neutral.inspect',
  'delivery.submission.describe', 'delivery.neutral.outcome', 'delivery.source.status',
  'delivery.integration.inspect']);
const READ_WORK_SELECTION = new Set(['work.prepare', 'work.admission', 'work.snapshot',
  'work.observe', 'work.recovery', 'command.inspect', 'claim.inspect', 'execution.inspect',
  'handoff.inspect', 'evidence.inspect', 'review.inspect', 'review.rounds', 'completion.inspect',
  'delivery.inspect', 'artifact.content', 'artifact.segment', 'delivery.exports']);
const READ_PAGED = new Set(['workstreams.list', 'work.list', 'work.graph', 'work.search', 'events.list',
  'work.next', 'work.inbox', 'task.assignees', 'delivery.exports', 'planning.candidates', 'sources.inspect', 'documents.list', 'decisions.list', 'review.rounds', 'handoff.list', 'dependencies.list']);
const READ_CONTEXT_BUDGET = new Set(['work.prepare', 'work.snapshot', 'work.observe', 'work.inbox',
  'source.content', 'artifact.content', 'artifact.segment', 'review.inspect',
  'delivery.neutral.inspect', 'delivery.submission.describe', 'planning.candidates', 'sources.inspect', 'documents.list', 'decisions.list', 'review.rounds', 'handoff.list', 'dependencies.list']);
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
  'change_id', 'member_actor_id', 'category', 'recipient_person_id', 'decision_id']);
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
  'sources.inspect': [{ path: ['work_id'], selector: 'work_id' }],
  'documents.list': [{ path: ['work_id'], selector: 'work_id' }],
  'review.rounds': [{ path: ['work_id'], selector: 'work_id' }],
  'handoff.list': [{ path: ['work_id'], selector: 'work_id' }],
  'dependencies.list': [{ path: ['work_id'], selector: 'work_id' }],
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

function isControlledSourcePath(value) {
  return typeof value === 'string' && value.length > 0 && value.length <= 512 && !value.startsWith('/')
    && !value.includes('..') && !value.includes('\\') && !value.includes(':') && !/[\x00-\x1f\x7f]/.test(value);
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
  } else if (op === 'sources.inspect' || op === 'documents.list' || op === 'dependencies.list') {
    allowIds('work_id', 'workstream_id');
  } else if (op === 'decisions.list') {
    allowIds('work_id', 'workstream_id', 'decision_id');
    allowed.add('decision_state');
  } else if (op === 'handoff.list') {
    allowIds('work_id', 'workstream_id', 'recipient_person_id');
    allowed.add('handoff_state');
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
  if (op === 'work.search' || op === 'task.assignees' || op === 'documents.list') allowed.add('search');
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
  if ('handoff_state' in query && !['pending', 'history', 'all'].includes(query.handoff_state)) {
    return invalidRead('Handoff view is invalid');
  }
  if ('decision_state' in query && !['open', 'replied', 'cancelled', 'all'].includes(query.decision_state)) {
    return invalidRead('Question view is invalid');
  }
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
    if (!isControlledSourcePath(query.source_path)) {
      return invalidRead('Selector source_path must be a controlled project-relative path');
    }
  }
  if ('include_denies' in query && typeof query.include_denies !== 'boolean') {
    return invalidRead('Selector include_denies must be a boolean');
  }
  return { ok: true };
}

function validWorkstreamList(reply) {
  return reply && typeof reply === 'object' && !Array.isArray(reply)
    && Array.isArray(reply.items)
    && reply.items.every(item => item && typeof item === 'object' && !Array.isArray(item) && isBoundedId(item.id))
    && Number.isSafeInteger(reply.total) && reply.total >= reply.items.length
    && (reply.next_cursor === null || typeof reply.next_cursor === 'string'
      && reply.next_cursor.length > 0 && reply.next_cursor.length <= 4096);
}

function validReadResponse(reply, op) {
  // workstreams.list is a flat catalog page without a protocol/data envelope.
  if (op === 'workstreams.list') return validWorkstreamList(reply);
  if (!reply || reply.protocol_version !== 1) return false;
  // capabilities is flat discovery; the remaining reads carry data.
  if (op === 'capabilities') return reply.protocol === 'awr-team-workstream';
  const data = reply.data;
  if (!data || typeof data !== 'object' || Array.isArray(data)) return false;
  if (data.next_cursor != null
    && (typeof data.next_cursor !== 'string' || data.next_cursor.length === 0
      || data.next_cursor.length > 4096)) return false;
  if (op === 'sources.inspect') return validRequiredSources(reply);
  if (op === 'documents.list') return validDocuments(reply);
  if (op === 'decisions.list') return validDecisions(reply);
  if (op === 'review.rounds') return validReviewRounds(reply);
  if (op === 'handoff.list') return validHandoffs(reply);
  if (op === 'dependencies.list') return validDependencies(reply);
  if (op === 'delivery.inspect' && data.delivery?.dependencies !== undefined) {
    return Array.isArray(data.delivery.dependencies) && data.delivery.dependencies.every((item) =>
      validDependency(item, data.delivery.work_id) && !('work_title' in item) && !('dependency_position' in item));
  }
  return true;
}

function validDecisions(reply) {
  const data = reply.data;
  const text = (value, bytes) => typeof value === 'string' && value.trim().length > 0
    && !value.includes('\0') && Buffer.byteLength(value, 'utf8') <= bytes;
  const name = (value) => value === null || typeof value === 'string' && Array.from(value).length <= 128;
  const person = (value) => asObject(value) && isBoundedId(value.person_id) && name(value.display_name);
  const actor = (value) => person(value) && isBoundedId(value.actor_id) && isBoundedId(value.client_id);
  if (!Array.isArray(data.items) || typeof data.truncated !== 'boolean'
    || data.truncated !== (data.next_cursor !== null) || data.execution_authorized !== false
    || !(data.work_id === null || isBoundedId(data.work_id))
    || !(data.decision_id === null || isBoundedId(data.decision_id))
    || !['open', 'replied', 'cancelled', 'all'].includes(data.decision_state)
    || !Number.isSafeInteger(data.as_of?.at_unix_ms) || data.as_of.at_unix_ms < 0
    || data.as_of.source_snapshot_id !== reply.source_snapshot_id
    || data.as_of.project_revision !== reply.project_revision) return false;
  return data.items.every((item) => {
    if (!asObject(item) || !isBoundedId(item.decision_id) || !isBoundedId(item.work_id)
      || !isBoundedId(item.workstream_id) || !isBoundedId(item.session_id) || !isBoundedId(item.request_key)
      || !text(item.work_title, 8192) || !text(item.question, 2048) || !text(item.impact, 2048)
      || typeof item.decision_version !== 'string' || !/^[1-9]\d*$/.test(item.decision_version)
      || typeof item.contract_hash !== 'string' || !/^[0-9a-f]{64}$/.test(item.contract_hash)
      || typeof item.baseline_matches_current !== 'boolean' || !actor(item.requester)
      || !isBoundedId(item.source_snapshot_id) || item.approval_granted !== false
      || !Number.isSafeInteger(item.created_at_unix_ms) || item.created_at_unix_ms < 0
      || !Array.isArray(item.allowed_responders) || item.allowed_responders.length < 1
      || item.allowed_responders.length > 20 || !item.allowed_responders.every(person)
      || !Array.isArray(item.allowed_responder_person_ids)
      || item.allowed_responder_person_ids.length !== item.allowed_responders.length
      || new Set(item.allowed_responder_person_ids).size !== item.allowed_responders.length
      || !item.allowed_responders.every((responder, index) => responder.person_id === item.allowed_responder_person_ids[index])) return false;
    if (item.state === 'open') return item.reply === null && item.resolution === null;
    return ['replied', 'cancelled'].includes(item.state) && text(item.reply, 4096)
      && actor(item.resolution) && item.resolution.kind === item.state
      && isBoundedId(item.resolution.session_id) && isBoundedId(item.resolution.request_id)
      && Number.isSafeInteger(item.resolution.at_unix_ms) && item.resolution.at_unix_ms >= item.created_at_unix_ms;
  });
}

function validDocuments(reply) {
  const data = reply.data;
  const fields = new Set(['path', 'name', 'summary', 'summary_kind', 'sha256', 'byte_length',
    'source_kind', 'readable', 'status', 'reason', 'document_position', 'content_work_id']);
  const boundedText = (value, maximum) => typeof value === 'string' && Array.from(value).length <= maximum;
  const fingerprint = (value) => typeof value === 'string' && /^[0-9a-f]{64}$/.test(value);
  if (!Array.isArray(data.items) || !Number.isSafeInteger(data.total) || data.total < data.items.length
    || typeof data.truncated !== 'boolean' || data.truncated !== (data.next_cursor !== null)
    || data.read_only !== true || data.execution_authorized !== false
    || data.content_read_limit_bytes !== 262144
    || !['work', 'authorized_workstreams', 'authorized_works'].includes(data.scope)
    || !Number.isSafeInteger(data.as_of?.at_unix_ms) || data.as_of.at_unix_ms < 0
    || data.as_of.source_snapshot_id !== reply.source_snapshot_id
    || data.as_of.project_revision !== reply.project_revision) return false;
  return data.items.every((item) => {
    if (!asObject(item) || Object.keys(item).some((key) => !fields.has(key))
      || !Number.isSafeInteger(item.document_position) || item.document_position < 1
      || item.document_position > data.total || item.source_kind !== 'registered_source'
      || item.summary_kind !== 'source_excerpt'
      || ('content_work_id' in item && !isBoundedId(item.content_work_id))
      || (data.scope === 'authorized_workstreams' && 'content_work_id' in item)
      || !(item.path === null || isControlledSourcePath(item.path))
      || !(item.name === null || boundedText(item.name, 256))
      || !(item.summary === null || boundedText(item.summary, 512))) return false;
    if (item.status === 'available' || item.status === 'too_large') {
      return isControlledSourcePath(item.path) && boundedText(item.name, 256) && item.name.length > 0
        && fingerprint(item.sha256) && Number.isSafeInteger(item.byte_length) && item.byte_length >= 0
        && (item.status === 'available'
          ? item.readable === true && item.reason === null && item.byte_length <= data.content_read_limit_bytes
          : item.readable === false && item.reason === 'content_exceeds_read_budget'
            && item.byte_length > data.content_read_limit_bytes);
    }
    return ['restricted', 'missing', 'changed', 'unverified'].includes(item.status)
      && item.readable === false && boundedText(item.reason, 128) && item.reason.length > 0
      && item.name === null && item.summary === null && item.sha256 === null && item.byte_length === null
      && (item.status !== 'restricted' || item.path === null && item.reason === 'permission_restricted'
        && !('content_work_id' in item));
  });
}

function validDependency(item, work) {
  const fields = new Set(['work_id', 'provider_work_id', 'provider_title', 'version_policy', 'review_assurance',
    'required_version', 'available_version', 'adopted_version', 'consumed_version', 'adoption_valid',
    'validity', 'reason', 'missing', 'work_title', 'dependency_position']);
  const versionFields = ['required_version', 'available_version', 'adopted_version', 'consumed_version'];
  const missingFields = ['provider_title', ...versionFields];
  const version = (value) => value === null || asObject(value)
    && Object.keys(value).length === 3
    && Object.keys(value).every((key) => ['receipt_id', 'contract_hash', 'source_snapshot_id'].includes(key))
    && (value.receipt_id === null || isBoundedId(value.receipt_id))
    && typeof value.contract_hash === 'string' && /^[0-9a-f]{64}$/.test(value.contract_hash)
    && (value.source_snapshot_id === null || isBoundedId(value.source_snapshot_id));
  if (!asObject(item) || !Object.keys(item).every((key) => fields.has(key))
    || !isBoundedId(item.work_id) || work != null && item.work_id !== work || !isBoundedId(item.provider_work_id)
    || !(item.provider_title === null || typeof item.provider_title === 'string')
    || !['current_contract', 'fixed_delivery'].includes(item.version_policy)
    || !['valid', 'upstream_changed', 'unavailable'].includes(item.validity)
    || !(item.reason === null || isBoundedId(item.reason))
    || !(item.adoption_valid === null || typeof item.adoption_valid === 'boolean')
    || !versionFields.every((key) => version(item[key])) || !asObject(item.missing)
    || Object.keys(item.missing).length !== missingFields.length
    || !Object.keys(item.missing).every((key) => missingFields.includes(key))
    || !missingFields.every((key) => item[key] === null ? isBoundedId(item.missing[key]) : item.missing[key] === null)
    || item.validity === 'valid' && (item.available_version === null || item.required_version === null || item.reason !== null)
    || item.validity === 'unavailable' && item.available_version !== null
    || item.required_version !== null && item.available_version !== null
      && item.required_version.contract_hash !== item.available_version.contract_hash
    || item.validity !== 'valid' && item.reason === null) return false;
  const assurance = item.review_assurance;
  if (assurance !== null && !['agent_reviewed_caller_asserted_reconciled', 'simulated_member_independent'].includes(assurance)) {
    const policy = asObject(assurance?.cross_workstream);
    if (!asObject(assurance) || Object.keys(assurance).length !== 1 || !policy
      || Object.keys(policy).length !== 2
      || !['team_independent', 'simulated_member_independent'].includes(policy.review_assurance)
      || policy.version_policy !== item.version_policy) return false;
  }
  for (const field of ['available_version', 'adopted_version', 'consumed_version']) {
    if (item[field] !== null && !isBoundedId(item[field].receipt_id)) return false;
  }
  const sameVersion = (left, right) => {
    if (left === null || right === null) return left === right;
    return ['receipt_id', 'contract_hash', 'source_snapshot_id'].every((field) => left[field] === right[field]);
  };
  const sameReceipt = (left, right) => left !== null && right !== null
    && left.receipt_id === right.receipt_id && left.contract_hash === right.contract_hash;
  if (asObject(assurance)?.cross_workstream) {
    if (item.adoption_valid === true) {
      if (item.adopted_version === null || !sameVersion(item.required_version, item.adopted_version)
        || !sameVersion(item.available_version, item.adopted_version)
        || item.version_policy === 'fixed_delivery' && item.adopted_version.source_snapshot_id === null) return false;
    } else if (item.adopted_version !== null || item.available_version !== null || item.validity === 'valid'
      || (item.adoption_valid === null || item.version_policy === 'fixed_delivery')
        && item.validity !== 'unavailable') return false;
  } else if (item.adoption_valid !== null || item.version_policy !== 'current_contract'
    || !sameVersion(item.adopted_version, item.consumed_version)) return false;
  if (item.validity === 'valid' && item.consumed_version !== null
    && !sameReceipt(item.consumed_version, item.available_version)) return false;
  return true;
}

function validDependencies(reply) {
  const data = reply.data, asOf = asObject(data.as_of);
  const envelopeFields = new Set(['protocol_version', 'scope_id', 'coordinator_epoch', 'project_status',
    'source_snapshot_id', 'project_revision', 'data', 'workstream_id', 'authority_version']);
  const fields = new Set(['work_id', 'items', 'total', 'next_cursor', 'truncated', 'count_scope', 'count_limited',
    'hidden_count', 'execution_authorized', 'as_of']);
  if (!Object.keys(reply).every((key) => envelopeFields.has(key))
    || !Object.keys(data).every((key) => fields.has(key))
    || !isBoundedId(reply.source_snapshot_id) || !isBoundedId(reply.coordinator_epoch)
    || ['project_status', 'workstream_id'].some((key) => key in reply && !isBoundedId(reply[key]))
    || 'authority_version' in reply && (typeof reply.authority_version !== 'string' || !/^[1-9]\d*$/.test(reply.authority_version))
    || !(data.work_id === null || isBoundedId(data.work_id))
    || !Array.isArray(data.items) || data.items.length > 100
    || !Number.isSafeInteger(data.total) || data.total < data.items.length
    || !(data.next_cursor === null || isBoundedCursor(data.next_cursor))
    || data.truncated !== (data.next_cursor !== null) || data.execution_authorized !== false
    || data.count_scope !== 'authorized_matches' || data.count_limited !== true || data.hidden_count !== null
    || !asOf || Object.keys(asOf).length !== 3
    || !Object.keys(asOf).every((key) => ['at_unix_ms', 'source_snapshot_id', 'project_revision'].includes(key))
    || !Number.isSafeInteger(asOf.at_unix_ms) || asOf.at_unix_ms < 0
    || asOf.source_snapshot_id !== reply.source_snapshot_id || asOf.project_revision !== reply.project_revision) return false;
  const edges = new Set();
  return data.items.every((item) => {
    if (!validDependency(item, data.work_id) || typeof item.work_title !== 'string'
      || !Number.isSafeInteger(item.dependency_position) || item.dependency_position < 1
      || item.dependency_position > data.total) return false;
    const key = JSON.stringify([item.work_id, item.provider_work_id]);
    if (edges.has(key)) return false;
    edges.add(key);
    return true;
  });
}

function validHandoffs(reply) {
  const data = reply.data, asOf = asObject(data.as_of);
  const timestamp = (value) => Number.isSafeInteger(value) && value >= 0;
  const envelopeFields = new Set(['protocol_version', 'scope_id', 'coordinator_epoch', 'project_status',
    'source_snapshot_id', 'project_revision', 'data', 'workstream_id', 'authority_version', 'selection_basis']);
  const dataFields = new Set(['work_id', 'recipient_person_id', 'handoff_state', 'items', 'total', 'next_cursor',
    'truncated', 'count_scope', 'count_limited', 'hidden_count', 'execution_authorized', 'as_of']);
  const asOfFields = new Set(['at_unix_ms', 'source_snapshot_id', 'project_revision', 'handoffs_revision']);
  if (!Object.keys(reply).every((field) => envelopeFields.has(field))
    || !Object.keys(data).every((field) => dataFields.has(field))
    || !isBoundedId(reply.source_snapshot_id) || !isBoundedId(reply.coordinator_epoch)
    || typeof reply.project_revision !== 'string' || !/^(0|[1-9]\d*)$/.test(reply.project_revision)
    || ['project_status', 'selection_basis', 'workstream_id'].some((field) => field in reply && !isBoundedId(reply[field]))
    || 'authority_version' in reply && (typeof reply.authority_version !== 'string' || !/^[1-9]\d*$/.test(reply.authority_version))
    || !(data.work_id === null || isBoundedId(data.work_id))
    || !(data.recipient_person_id === null || isBoundedId(data.recipient_person_id))
    || !['pending', 'history', 'all'].includes(data.handoff_state)
    || !Array.isArray(data.items) || data.items.length > 100
    || data.execution_authorized !== false || !Number.isSafeInteger(data.total) || data.total < data.items.length
    || !(data.next_cursor === null || isBoundedCursor(data.next_cursor))
    || data.truncated !== (data.next_cursor !== null) || data.count_scope !== 'authorized_matches'
    || data.count_limited !== true || data.hidden_count !== null || !asOf
    || !Object.keys(asOf).every((field) => asOfFields.has(field)) || !timestamp(asOf.at_unix_ms)
    || asOf.source_snapshot_id !== reply.source_snapshot_id || asOf.project_revision !== reply.project_revision
    || typeof asOf.handoffs_revision !== 'string' || !/^[0-9a-f]{32}$/.test(asOf.handoffs_revision)) return false;
  const fields = new Set(['handoff_id', 'handoff_position', 'work_id', 'work_title', 'kind', 'status',
    'pending', 'expired', 'group', 'effective_status', 'version', 'from_person_id', 'to_person_id',
    'from_display_name', 'to_display_name', 'created_at_unix_ms', 'updated_at_unix_ms', 'expires_at_unix_ms']);
  const ids = new Set();
  let previous;
  for (const item of data.items) {
    if (!asObject(item) || !Object.keys(item).every((field) => fields.has(field))
      || !['handoff_id', 'work_id', 'from_person_id', 'to_person_id'].every((field) => isBoundedId(item[field]))
      || ids.has(item.handoff_id) || typeof item.work_title !== 'string'
      || !['execution', 'responsibility'].includes(item.kind)
      || !['proposed', 'inspected', 'accepted', 'rejected', 'cancelled', 'timed_out'].includes(item.status)
      || !Number.isSafeInteger(item.handoff_position) || item.handoff_position < 1 || item.handoff_position > data.total
      || typeof item.version !== 'string' || !/^[1-9]\d*$/.test(item.version)
      || !['from_display_name', 'to_display_name'].every((field) => item[field] === null || typeof item[field] === 'string')
      || !timestamp(item.created_at_unix_ms) || !timestamp(item.updated_at_unix_ms)
      || !(item.expires_at_unix_ms === null || timestamp(item.expires_at_unix_ms))) return false;
    const open = ['proposed', 'inspected'].includes(item.status);
    const deadlineElapsed = item.expires_at_unix_ms !== null && item.expires_at_unix_ms <= asOf.at_unix_ms;
    const pending = open && !deadlineElapsed, expired = item.status === 'timed_out' || open && deadlineElapsed;
    if (item.pending !== pending || item.expired !== expired || item.group !== (pending ? 'pending' : 'history')
      || item.effective_status !== (expired ? 'expired' : item.status)
      || data.handoff_state === 'pending' && !pending || data.handoff_state === 'history' && pending
      || previous && item.created_at_unix_ms < previous.created_at_unix_ms) return false;
    ids.add(item.handoff_id);
    previous = item;
  }
  return true;
}

const REVIEW_SUMMARY_FIELDS = ['state', 'bundle_hash', 'contract_hash', 'contract_matches_current',
  'artifact_digest', 'reviewer_role', 'role_basis', 'reviewer_actor_id', 'reviewer_person_id',
  'decision', 'decided_at_unix_ms', 'reason_first_line'];

function validReviewRounds(reply) {
  const data = reply.data, asOf = asObject(data.as_of);
  const digest = (value) => typeof value === 'string' && /^[0-9a-f]{64}$/.test(value);
  if (!isBoundedId(data.work_id) || !Array.isArray(data.items) || data.items.length > 100
    || data.execution_authorized !== false || !Number.isSafeInteger(data.total) || data.total < data.items.length
    || !(data.next_cursor === null || isBoundedCursor(data.next_cursor))
    || data.truncated !== (data.next_cursor !== null) || !asOf
    || !Number.isSafeInteger(asOf.at_unix_ms) || asOf.at_unix_ms < 0
    || asOf.source_snapshot_id !== reply.source_snapshot_id || asOf.project_revision !== reply.project_revision
    || typeof asOf.rounds_revision !== 'string' || !/^[0-9a-f]{32}$/.test(asOf.rounds_revision)) return false;
  const ids = new Set();
  let previousIndex = 0;
  for (const item of data.items) {
    if (!asObject(item) || !isBoundedId(item.round_id) || ids.has(item.round_id)
      || !Number.isSafeInteger(item.round_index) || item.round_index <= previousIndex
      || !Number.isSafeInteger(item.round_position) || item.round_position < 1 || item.round_position > data.total
      || typeof item.readable !== 'boolean' || !asObject(item.missing)) return false;
    ids.add(item.round_id);
    previousIndex = item.round_index;
    if (!item.readable) {
      if (item.reason !== 'not_in_review_work_scope'
        || !REVIEW_SUMMARY_FIELDS.every((field) => item[field] === null && item.missing[field] === item.reason)) return false;
      continue;
    }
    if (item.reason !== null || !['open', 'approved', 'rejected', 'invalidated'].includes(item.state)
      || !digest(item.bundle_hash) || !digest(item.contract_hash) || typeof item.contract_matches_current !== 'boolean'
      || !(item.artifact_digest === null || digest(item.artifact_digest))
      || !(item.reviewer_role === null || ['independent_reviewer', 'author_reviewer', 'agent_reviewer', 'simulated_member_reviewer'].includes(item.reviewer_role))
      || item.role_basis !== (item.reviewer_role === null ? null : 'recorded_approval_basis')
      || !(item.reviewer_actor_id === null || isBoundedId(item.reviewer_actor_id))
      || !(item.reviewer_person_id === null || isBoundedId(item.reviewer_person_id))
      || !(item.decision === null || ['approve', 'reject'].includes(item.decision))
      || !(item.decided_at_unix_ms === null || Number.isSafeInteger(item.decided_at_unix_ms) && item.decided_at_unix_ms >= 0)
      || !(item.reason_first_line === null || typeof item.reason_first_line === 'string' && !/[\r\n\u0085\u2028\u2029]/.test(item.reason_first_line))
      || !REVIEW_SUMMARY_FIELDS.every((field) => item.missing[field] === (item[field] === null ? 'not_recorded' : null))) return false;
    const noDecision = item.decision === null;
    if (noDecision !== (item.decided_at_unix_ms === null) || noDecision !== (item.reviewer_actor_id === null)
      || noDecision !== (item.reason_first_line === null)
      || noDecision && (item.reviewer_role !== null || item.reviewer_person_id !== null)) return false;
  }
  return true;
}

function validRequiredSources(reply) {
  const data = reply.data, collection = asObject(data.collection), asOf = asObject(data.as_of);
  const timestamp = (value) => Number.isSafeInteger(value) && value >= 0;
  const reason = (value) => typeof value === 'string' && value.length > 0 && value.length <= 1024;
  const digest = (value) => typeof value === 'string' && /^[0-9a-f]{64}$/.test(value);
  const nullableReason = (value) => value === null || reason(value);
  if (!collection || !asOf || !Array.isArray(data.items) || data.items.length > 100
    || !(data.work_id === null || isBoundedId(data.work_id)) || data.execution_authorized !== false
    || !Number.isSafeInteger(data.total) || data.total < data.items.length
    || !(data.next_cursor === null || isBoundedCursor(data.next_cursor))
    || data.truncated !== (data.next_cursor !== null)
    || collection.required_total !== data.total || typeof collection.complete !== 'boolean'
    || collection.completeness !== (collection.complete ? 'complete' : 'incomplete')
    || !['work', 'authorized_workstreams'].includes(collection.scope)
    || collection.verification_basis !== 'active_source_snapshot' || collection.recovery_scope !== 'page'
    || !timestamp(collection.checked_at_unix_ms) || asOf.at_unix_ms !== collection.checked_at_unix_ms
    || asOf.source_snapshot_id !== reply.source_snapshot_id || asOf.project_revision !== reply.project_revision
    || !Array.isArray(collection.reasons) || collection.reasons.length > 100 || !collection.reasons.every(reason)
    || new Set(collection.reasons).size !== collection.reasons.length
    || collection.complete !== (collection.reasons.length === 0)
    || !Array.isArray(collection.recovery) || collection.recovery.length > data.items.length) return false;
  const positions = new Set();
  for (const item of data.items) {
    if (!asObject(item) || !Number.isSafeInteger(item.source_position) || item.source_position < 1
      || item.source_position > data.total || positions.has(item.source_position)
      || !(item.path === null || isControlledSourcePath(item.path))
      || !(item.expected_sha256 === null || digest(item.expected_sha256))
      || !(item.current_sha256 === null || digest(item.current_sha256))
      || !['consistent', 'changed', 'missing', 'forbidden', 'unverified'].includes(item.status)
      || item.checked_at_unix_ms !== collection.checked_at_unix_ms
      || !asObject(item.missing) || !['path', 'expected_sha256', 'current_sha256'].every((key) => nullableReason(item.missing[key]))
      || !nullableReason(item.reason)) return false;
    positions.add(item.source_position);
    if (item.status === 'consistent') {
      if (item.reason !== null || item.path === null
        || !digest(item.expected_sha256) || item.expected_sha256 !== item.current_sha256) return false;
    } else if (!reason(item.reason) || collection.complete || !collection.reasons.includes(item.reason)) return false;
    if (item.status === 'forbidden' && (item.path !== null || item.expected_sha256 !== null || item.current_sha256 !== null
      || item.reason !== 'required_spec_forbidden' || !Object.values(item.missing).every((value) => value === 'permission_restricted'))) return false;
    if (item.reason === 'required_spec_forbidden' && item.status !== 'forbidden') return false;
    if (item.status === 'changed' && (item.path === null || !digest(item.expected_sha256) || !digest(item.current_sha256)
      || item.expected_sha256 === item.current_sha256 || item.reason !== 'spec_digest_mismatch')) return false;
    if (item.status === 'missing' && (item.path === null || item.current_sha256 !== null || item.reason !== 'required_spec_missing')) return false;
  }
  const recoveryPositions = new Set();
  for (const entry of collection.recovery) {
    const item = data.items.find((item) => item.source_position === entry?.source_position);
    if (!asObject(entry) || !item || item.reason === null || entry.reason !== item.reason
      || recoveryPositions.has(entry.source_position) || typeof entry.recognized !== 'boolean'
      || !(entry.next_action === null || typeof entry.next_action === 'string' && entry.next_action.length <= 2048)
      || !entry.recognized && entry.next_action !== null) return false;
    recoveryPositions.add(entry.source_position);
  }
  return recoveryPositions.size === data.items.filter((item) => item.reason !== null).length;
}

function isBoundedCursor(value) {
  return typeof value === 'string' && value.length > 0 && value.length <= 4096;
}

function validPageCoverage(data, query, positionField) {
  const items = data.items, limit = query.limit ?? 20;
  if (!Array.isArray(items) || items.length > limit) return false;
  if (data.total === 0) return items.length === 0 && data.next_cursor === null;
  if (items.length === 0) return false;
  const first = items[0]?.[positionField];
  if (!Number.isSafeInteger(first) || !query.cursor && first !== 1
    || !items.every((item, index) => item?.[positionField] === first + index)) return false;
  const last = first + items.length - 1;
  return data.next_cursor === null ? last === data.total : last < data.total && items.length === limit;
}

/**
 * AWR-OBS-002 shared read attribution: one semantics for the generic read
 * route and the coordination detail route. Enveloped reads must belong to
 * the requested work/workstream/scope; navigation reads use their own
 * response contract. No read may grant execution. strictWorkstream
 * additionally pins the response workstream (coordination detail selects one).
 */
function readAttributionError(query, result, { strictWorkstream } = {}) {
  // The catalog page has no scope/snapshot/object attribution envelope.
  // Its project and identity are bound by the forwarded request path and
  // cookie session; the service filters visible streams and binds cursors to
  // that authority and snapshot. Do not invent missing freshness metadata.
  if (query.op === 'workstreams.list') {
    if (!validWorkstreamList(result) || result.items.length > (query.limit ?? 50)) {
      return { code: 'BadGateway', message: 'Invalid Team read response' };
    }
    if (result.execution_authorized === true) {
      return { code: 'BadGateway', message: 'A Team read response never authorizes execution' };
    }
    return null;
  }
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
  if (query.op === 'sources.inspect' && (
    !validPageCoverage(data, query, 'source_position')
    || ((query.work_id || query.workstream_id) && (!isBoundedId(result.workstream_id)
      || typeof result.authority_version !== 'string' || !/^\d+$/.test(result.authority_version)))
    || query.workstream_id && result.workstream_id !== query.workstream_id
    || !query.work_id && data.work_id !== null
    || data.collection?.scope !== (query.work_id ? 'work' : 'authorized_workstreams'))) {
    return { code: 'BadGateway', message: 'Source observations are not attributable to the requested scope' };
  }
  if (query.op === 'documents.list' && (
    !validPageCoverage(data, query, 'document_position')
    || ((query.work_id || query.workstream_id) && (!isBoundedId(result.workstream_id)
      || typeof result.authority_version !== 'string' || !/^[1-9]\d*$/.test(result.authority_version)))
    || query.workstream_id && result.workstream_id !== query.workstream_id
    || data.work_id !== (query.work_id ?? null)
    || (query.work_id ? data.scope !== 'work' : !['authorized_workstreams', 'authorized_works'].includes(data.scope))
    || query.work_id && data.items.some((item) => item.content_work_id && item.content_work_id !== query.work_id))) {
    return { code: 'BadGateway', message: 'Documents are not attributable to the requested scope' };
  }
  if (query.op === 'decisions.list' && (
    data.items.length > (query.limit ?? 20)
    || data.next_cursor !== null && data.items.length !== (query.limit ?? 20)
    || data.work_id !== (query.work_id ?? null) || data.decision_id !== (query.decision_id ?? null)
    || data.decision_state !== (query.decision_state ?? 'open')
    || result.selection_basis !== (query.work_id ? 'work' : query.workstream_id ? 'explicit' : 'project')
    || (query.work_id || query.workstream_id) && (!isBoundedId(result.workstream_id)
      || typeof result.authority_version !== 'string' || !/^[1-9]\d*$/.test(result.authority_version))
    || new Set(data.items.map((item) => item.decision_id)).size !== data.items.length
    || data.items.some((item, index) => query.work_id && item.work_id !== query.work_id
      || (query.work_id || query.workstream_id) && item.workstream_id !== result.workstream_id
      || query.decision_id && item.decision_id !== query.decision_id
      || data.decision_state !== 'all' && item.state !== data.decision_state
      || index > 0 && data.items[index - 1].decision_id <= item.decision_id))) {
    return { code: 'BadGateway', message: 'Questions are not attributable to the requested scope' };
  }
  if (query.op === 'review.rounds' && (!validPageCoverage(data, query, 'round_position')
    || !isBoundedId(result.workstream_id) || typeof result.authority_version !== 'string'
    || !/^\d+$/.test(result.authority_version))) {
    return { code: 'BadGateway', message: 'Review history is not attributable to the requested scope' };
  }
  if (query.op === 'handoff.list' && (!validPageCoverage(data, query, 'handoff_position')
    || data.work_id !== (query.work_id ?? null) || data.recipient_person_id !== (query.recipient_person_id ?? null)
    || data.handoff_state !== (query.handoff_state ?? 'pending')
    || (query.work_id || query.workstream_id) && (!isBoundedId(result.workstream_id)
      || typeof result.authority_version !== 'string' || !/^\d+$/.test(result.authority_version))
    || query.work_id && data.items.some((item) => item.work_id !== query.work_id)
    || query.recipient_person_id && data.items.some((item) => item.to_person_id !== query.recipient_person_id))) {
    return { code: 'BadGateway', message: 'Handoff observations are not attributable to the requested scope' };
  }
  if (query.op === 'dependencies.list' && (!validPageCoverage(data, query, 'dependency_position')
    || !query.work_id && data.work_id !== null
    || (query.work_id || query.workstream_id) && (!isBoundedId(result.workstream_id)
      || typeof result.authority_version !== 'string' || !/^[1-9]\d*$/.test(result.authority_version))
    || query.work_id && data.items.some((item) => item.work_id !== query.work_id))) {
    return { code: 'BadGateway', message: 'Dependency observations are not attributable to the requested scope' };
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
 * Observer /api/team/projects, logout, or revoke.
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
      path.resolve(__dirname, '../../../tests/fixtures/workstreams/team-web-loop'),
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

  const OVERVIEW_CACHE_TTL_MS = 60000;
  const overviewCache = new Map();
  const overviewSessionExpiries = new WeakMap();

  function overviewEntryExpired(entry) {
    return now() - entry.readAt >= OVERVIEW_CACHE_TTL_MS || now() < entry.readAt
      || (entry.expiresAtMs != null && entry.expiresAtMs <= now());
  }

  function cachedOverviewValue(req, kind, project) {
    const sessionId = parseTeamCookie(req);
    if (!sessionId) return null;
    const key = `${sessionId}:${kind}:${project}`;
    const entry = overviewCache.get(key);
    if (!entry) return null;
    if (overviewEntryExpired(entry)) {
      overviewCache.delete(key);
      return null;
    }
    return { value: structuredClone(entry.value), readAt: entry.readAt };
  }

  function storeOverviewValue(req, kind, project, value) {
    const sessionId = parseTeamCookie(req);
    if (!sessionId) return;
    const expiresAtMs = overviewSessionExpiries.get(req) ?? null;
    // Sweep on every write so sessions that never return cannot accumulate.
    for (const [key, entry] of overviewCache.entries()) {
      if (overviewEntryExpired(entry)) overviewCache.delete(key);
    }
    overviewCache.set(`${sessionId}:${kind}:${project}`, {
      value: structuredClone(value), readAt: now(), expiresAtMs,
    });
  }

  function clearOverviewCache(req) {
    const sessionId = parseTeamCookie(req);
    if (!sessionId) return;
    for (const key of overviewCache.keys()) {
      if (key.startsWith(`${sessionId}:`)) overviewCache.delete(key);
    }
  }

  async function readOverviewSession(req) {
    // Session checks are small but revocation-sensitive: always revalidate.
    const session = await proxyTeam('/v1/web/session', req, null, 'GET');
    if (!session || session.status >= 400) {
      clearOverviewCache(req);
      return session;
    }
    const expiresAtMs = session.json && session.json.expires_at_ms;
    // Keep only this request's expiry metadata, never the session response.
    // Every value stored later in the composition inherits that hard limit.
    overviewSessionExpiries.set(req, Number.isFinite(expiresAtMs) ? expiresAtMs : null);
    return { ...session, readAt: now() };
  }

  async function readOverviewCapabilities(project, req, signal) {
    const cached = cachedOverviewValue(req, 'capabilities', project);
    if (cached) return { status: 200, json: cached.value, setCookie: [], readAt: cached.readAt };
    const capabilities = await proxyTeam(`/v1/web/projects/${encodeURIComponent(project)}/query`, req,
      { protocol_version: 1, op: 'capabilities' }, 'POST', signal);
    if (!capabilities || capabilities.status >= 400) {
      clearOverviewCache(req);
      return capabilities;
    }
    storeOverviewValue(req, 'capabilities', project, capabilities.json);
    return { ...capabilities, readAt: now() };
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
      const summary = item?.summary?.version === 1 ? item.summary : null;
      const reported = summary?.report?.value;
      const outcome = summary?.outcome?.value;
      const blocker = summary?.blocker?.value;
      return {
        key: String(key),
        title: (item && (item.title || item.name)) || String(key),
        owner_person: summary ? summary.owner?.value?.display_name ?? null : (item && item.owner_person) || null,
        agent: summary ? reported?.origin?.actor_name ?? null : (item && item.agent) || null,
        outcome: summary ? outcome?.summary ?? null : (item && item.outcome) || null,
        status: summary ? summary.runtime?.value ?? 'unknown' : (item && (item.status || item.state)) || 'unknown',
        blocker: summary ? blocker?.text ?? null : (item && item.blocker) || null,
        next_step: summary ? summary.next_action?.value ?? null : (item && item.next_step) || null,
        ...(summary ? { observation_summary: summary } : {}),
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
  async function queryAllPagesUncached(project, query, req, res, { pageTimeoutMs = 4000, deadlineAt = null } = {}) {
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

  async function queryAllPages(project, query, req, res, options = {}) {
    if (query.op === 'workstreams.list') {
      const cached = cachedOverviewValue(req, 'workstreams', project);
      if (cached) return { ...cached.value, readAt: cached.readAt };
    }
    const result = await queryAllPagesUncached(project, query, req, res, options);
    if (query.op === 'workstreams.list' && !result.error) {
      storeOverviewValue(req, 'workstreams', project, result);
    }
    return { ...result, readAt: now() };
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
        const session = await readOverviewSession(req);
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
        const session = await readOverviewSession(req);
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
        const scopeReadAt = streams.readAt ?? now();
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
              read_at_unix_ms: page.readAt ?? now(),
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
            read_at_unix_ms: page.readAt ?? now(),
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
          capabilities = await readOverviewCapabilities(project, req,
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
        const pageReadAt = Math.min(
          session.readAt ?? now(),
          scopeReadAt,
          capabilities.readAt ?? now(),
          ...parts.map((part) => part.read_at_unix_ms ?? now()),
        );
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
            page_read: { status: 'measured', at_unix_ms: pageReadAt },
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

    'GET /api/team/graph': async (url, _body, req, res) => {
      const project = url.searchParams.get('project');
      if (!project || project.length > 128 || [...url.searchParams.keys()].some((key) => key !== 'project')) {
        return { ok: false, error: { code: 'InvalidInput', message: 'One bounded project required' } };
      }
      if (!TEAM.live) {
        if (!TEAM.demoMode) return { ok: false, error: { code: 'DemoDisabled', message: 'fixtures require demo mode' } };
        const data = readFixture('team-view.json');
        if (!data.projects.some((item) => item.key === project)) {
          return { ok: false, error: { code: 'NotFound', message: 'Project unavailable' } };
        }
        return {
          ok: true, project, demo: true, workstreams: data.workstreams || [],
          as_of: null, read_at_unix_ms: now(),
          items: data.works.map((work) => ({
            work_id: work.key, external_key: work.key, title: work.title,
            workstream_id: work.workstream_id, state: work.status || 'unknown',
            required_dependencies: (work.depends_on || []).filter((dep) => dep.visible !== false).map((dep) => dep.key),
            dependencies_complete: !(work.hidden_deps || []).length,
            owner: work.owner_person || work.agent || null, last_event_at: null, claims: [],
          })),
        };
      }
      const session = await readOverviewSession(req);
      applyProxiedCookies(res, session && session.setCookie);
      if (!session || session.status >= 400) return liveError(session);
      const deadlineAt = now() + overviewDeadlineMs;
      const streams = await queryAllPagesUncached(project, { op: 'workstreams.list' }, req, res,
        { pageTimeoutMs: readTimeoutMs, deadlineAt });
      if (streams.error) return { ok: false, error: streams.error };
      if (streams.truncated || streams.items.length !== streams.total) {
        return { ok: false, error: { code: 'ResponseTooLarge', message: 'Graph scope is incomplete' } };
      }
      const items = [];
      let asOf = null;
      for (const stream of streams.items) {
        const page = await queryAllPages(project, { op: 'work.graph', workstream_id: stream.id }, req, res,
          { pageTimeoutMs: readTimeoutMs, deadlineAt });
        if (page.error) return { ok: false, error: page.error };
        if (page.truncated || page.items.length !== page.total) {
          return { ok: false, error: { code: 'ResponseTooLarge', message: 'Graph work enumeration is incomplete' } };
        }
        if (!page.as_of || asOf && JSON.stringify(asOf) !== JSON.stringify(page.as_of)) {
          return { ok: false, error: { code: 'SourceChanged', message: 'Project changed while reading the graph; refresh' } };
        }
        asOf = page.as_of;
        items.push(...page.items.map((item) => ({ ...item, workstream_id: stream.id })));
      }
      // Scope discovery has a bare list contract. Recheck its complete identity
      // after composing revision-bound work pages to catch grants or scope changes.
      const finalScope = await queryAllPagesUncached(project, { op: 'workstreams.list' }, req, res,
        { pageTimeoutMs: readTimeoutMs, deadlineAt });
      if (finalScope.error) return { ok: false, error: finalScope.error };
      const scopeIdentity = (list) => JSON.stringify(list.map(({ id, authority_version }) => [id, authority_version])
        .sort(([a], [b]) => a < b ? -1 : a > b ? 1 : 0));
      if (finalScope.truncated || finalScope.items.length !== finalScope.total
        || scopeIdentity(streams.items) !== scopeIdentity(finalScope.items)) {
        return { ok: false, error: { code: 'SourceChanged', message: 'Project scope changed while reading the graph; refresh' } };
      }
      return {
        ok: true, project, items, as_of: asOf, read_at_unix_ms: now(),
        workstreams: streams.items.map(({ id, external_key, title }) => ({ id, external_key, title })),
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

    'GET /api/team/plans': async (url, _body, req, res) => {
      const allowed = new Set(['project', 'workstream_id', 'cursor', 'limit', 'max_context_bytes']);
      if ([...url.searchParams.keys()].some((key) => !allowed.has(key) || url.searchParams.getAll(key).length !== 1)) {
        return { ok: false, error: { code: 'InvalidInput', message: 'One bounded planning read required' },
          freshness: unmeasuredFreshness(Date.now()) };
      }
      const query = { protocol_version: 1, op: 'planning.candidates' };
      for (const key of ['workstream_id', 'cursor']) {
        if (url.searchParams.has(key)) query[key] = url.searchParams.get(key);
      }
      for (const key of ['limit', 'max_context_bytes']) {
        if (url.searchParams.has(key)) {
          const value = url.searchParams.get(key);
          query[key] = /^[1-9]\d*$/.test(value) ? Number(value) : null;
        }
      }
      return routes['POST /api/team/read'](url, { project: url.searchParams.get('project'), query }, req, res);
    },

    'GET /api/team/sources': async (url, _body, req, res) => {
      const allowed = new Set(['project', 'work_id', 'workstream_id', 'cursor', 'limit', 'max_context_bytes']);
      if ([...url.searchParams.keys()].some((key) => !allowed.has(key) || url.searchParams.getAll(key).length !== 1)) {
        return { ok: false, error: { code: 'InvalidInput', message: 'One bounded source read required' },
          freshness: unmeasuredFreshness(Date.now()) };
      }
      const query = { protocol_version: 1, op: 'sources.inspect' };
      for (const key of ['work_id', 'workstream_id', 'cursor']) {
        if (url.searchParams.has(key)) query[key] = url.searchParams.get(key);
      }
      for (const key of ['limit', 'max_context_bytes']) {
        if (url.searchParams.has(key)) {
          const value = url.searchParams.get(key);
          query[key] = /^[1-9]\d*$/.test(value) ? Number(value) : null;
        }
      }
      return routes['POST /api/team/read'](url, { project: url.searchParams.get('project'), query }, req, res);
    },

    'GET /api/team/reviews': async (url, _body, req, res) => {
      const allowed = new Set(['project', 'work_id', 'session_id', 'workstream_id', 'cursor', 'limit', 'max_context_bytes']);
      if ([...url.searchParams.keys()].some((key) => !allowed.has(key) || url.searchParams.getAll(key).length !== 1)) {
        return { ok: false, error: { code: 'InvalidInput', message: 'One bounded review history read required' },
          freshness: unmeasuredFreshness(Date.now()) };
      }
      const query = { protocol_version: 1, op: 'review.rounds' };
      for (const key of ['work_id', 'session_id', 'workstream_id', 'cursor']) {
        if (url.searchParams.has(key)) query[key] = url.searchParams.get(key);
      }
      for (const key of ['limit', 'max_context_bytes']) {
        if (url.searchParams.has(key)) {
          const value = url.searchParams.get(key);
          query[key] = /^[1-9]\d*$/.test(value) ? Number(value) : null;
        }
      }
      return routes['POST /api/team/read'](url, { project: url.searchParams.get('project'), query }, req, res);
    },

    'GET /api/team/dependencies': async (url, _body, req, res) => {
      const allowed = new Set(['project', 'work_id', 'workstream_id', 'cursor', 'limit', 'max_context_bytes']);
      if ([...url.searchParams.keys()].some((key) => !allowed.has(key) || url.searchParams.getAll(key).length !== 1)) {
        return { ok: false, error: { code: 'InvalidInput', message: 'One bounded dependency read required' },
          freshness: unmeasuredFreshness(Date.now()) };
      }
      const query = { protocol_version: 1, op: 'dependencies.list' };
      for (const key of ['work_id', 'workstream_id', 'cursor']) {
        if (url.searchParams.has(key)) query[key] = url.searchParams.get(key);
      }
      for (const key of ['limit', 'max_context_bytes']) {
        if (url.searchParams.has(key)) {
          const value = url.searchParams.get(key);
          query[key] = /^[1-9]\d*$/.test(value) ? Number(value) : null;
        }
      }
      return routes['POST /api/team/read'](url, { project: url.searchParams.get('project'), query }, req, res);
    },

    'GET /api/team/handoffs': async (url, _body, req, res) => {
      const allowed = new Set(['project', 'work_id', 'workstream_id', 'recipient_person_id', 'handoff_state', 'cursor', 'limit', 'max_context_bytes']);
      if ([...url.searchParams.keys()].some((key) => !allowed.has(key) || url.searchParams.getAll(key).length !== 1)) {
        return { ok: false, error: { code: 'InvalidInput', message: 'One bounded handoff read required' },
          freshness: unmeasuredFreshness(Date.now()) };
      }
      const query = { protocol_version: 1, op: 'handoff.list' };
      for (const key of ['work_id', 'workstream_id', 'recipient_person_id', 'handoff_state', 'cursor']) {
        if (url.searchParams.has(key)) query[key] = url.searchParams.get(key);
      }
      for (const key of ['limit', 'max_context_bytes']) {
        if (url.searchParams.has(key)) {
          const value = url.searchParams.get(key);
          query[key] = /^[1-9]\d*$/.test(value) ? Number(value) : null;
        }
      }
      return routes['POST /api/team/read'](url, { project: url.searchParams.get('project'), query }, req, res);
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
      // a flat discovery response and carries no attributable object.
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
      clearOverviewCache(req);
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
      clearOverviewCache(req);
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
        const session = await readOverviewSession(req);
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
    cacheSize: () => overviewCache.size,
    readFixture,
    teamAuth,
    rewriteOwnedCookiePath,
    OWNED_COOKIE_PATH,
    UPSTREAM_COOKIE_PATH,
  };
}

module.exports = { createTeamBridge, rewriteOwnedCookiePath, READ_OPS, READ_DEFERRED_OPS, READ_ECHOES, readFreshness, readAttributionError };
