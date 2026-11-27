'use strict';

// Public GitHub observations are separate from AWR registration and acceptance.
// Only an exact GitHub PR reference from an authorized work response is followed.
function pullRequestReference(observation) {
  const registered = (observation.pr_deliveries || []).find(p => p.state === 'active' && p.contract_matches_current);
  const checkpoint = observation.checkpoint;
  const text = registered ? registered.url : checkpoint && checkpoint.contract_matches_current
    ? [checkpoint.next_action, ...(checkpoint.open_loops || [])].join('\n') : '';
  const url = String(text || '').match(/https:\/\/github\.com\/([\w.-]+)\/([\w.-]+)\/pull\/([1-9]\d*)(?=$|[\s/?#).,])/);
  const shorthand = !registered && String(text || '').match(/(?:^|[\s(])([\w.-]+)\/([\w.-]+)#([1-9]\d*)(?=$|[\s).,])/);
  const match = url || shorthand;
  if (!match) return null;
  const [, owner, repo, number] = match;
  if ([owner, repo].some(s => s === '.' || s === '..') || !Number.isSafeInteger(Number(number))) return null;
  return { owner, repo, number: Number(number), url: `https://github.com/${owner}/${repo}/pull/${number}`,
    registration: registered ? 'registered' : 'checkpoint_mention', expected_head: registered?.head_sha || null };
}

function mapObservation(data) {
  const session = data.session || {}, owner = data.responsibility || {};
  const participant = 'last_participation' in data ? data.last_participation || {} : session;
  const execution = data.execution, checkpoint = data.checkpoint, claim = data.claim;
  const runtime = data.runtime || {};
  const terminal = ['completed', 'cancelled', 'archived'].includes(runtime.state);
  const attention = terminal ? null : runtime.recovery_blocked || execution?.recovery_blocked
    ? 'recovery_required' : claim && ['active', 'expired'].includes(claim.state) && !claim.lease_live
      ? 'lease_expired' : execution?.state === 'unknown' ? 'result_unknown' : null;
  return {
    observation_available: true, observed_at_ms: data.observed_at_unix_ms,
    last_activity_at_ms: data.last_activity_at_unix_ms,
    owner_person: owner.owner_name || owner.owner_person_id || owner.historical || null,
    claimant: claim?.state === 'active' && claim.lease_live ? session.actor_name || session.actor_id || null : null,
    last_participant: participant.actor_name || participant.actor_id || null,
    last_participation: data.last_participation || null,
    agent: data.client?.product || null, delegated_agent: owner.executor_agent_id || null,
    client_info: data.client || null, client_id: session.client_id || null,
    model: typeof data.model === 'string' ? data.model : data.model?.id || null,
    model_info: data.model || null, usage: data.usage || null, session_id: session.id || null,
    progress_report: data.progress || null, reporting: data.reporting || null,
    structured_waits: mapStructuredWaits(data.structured_waits),
    checkpoint, claim, execution: execution ? {
      state: execution.state, id: execution.execution_id,
      origin: execution.origin || null,
      contract_matches_current: execution.contract_matches_current,
      lease_live: execution.lease_live, receipt_details_available: execution.receipt_details_available,
      terminal_reported: booleanOrUnknown(execution.terminal_reported),
      artifact_verified: booleanOrUnknown(execution.artifact_verified),
      artifact_verification_basis: execution.artifact_verification_basis || null,
      effects_settled: booleanOrUnknown(execution.effects_settled),
      settlement_basis: execution.settlement_basis || null,
      settlement_scope: execution.settlement_scope || null,
      recovery_blocked: booleanOrUnknown(execution.recovery_blocked),
      recovery_cause: execution.recovery_cause || null,
      reconciliation_authority: booleanOrUnknown(execution.reconciliation_authority),
      controlled_confirmation_available: booleanOrUnknown(execution.controlled_confirmation_available),
      previous_epoch_review_required: booleanOrUnknown(execution.previous_epoch_review_required),
      reconciliation_coverage: mapReconciliationCoverage(execution.reconciliation_coverage),
      receipt_missing: data.missing?.execution_receipt || null,
      report: execution.latest_receipt ? {
        kind: execution.latest_receipt.receipt_kind,
        outcome: execution.latest_receipt.payload?.outcome,
        note: execution.latest_receipt.payload?.note,
      } : null,
    } : null,
    attention, status: runtime.state ?? null,
    reconciliation_coverage: mapReconciliationCoverage(data.reconciliation_coverage),
    // These are authorized AWR facts, independent of optional public PR reads.
    collaboration: data.collaboration || null,
    guidance: data.guidance || null,
    // Checkpoint prose is historical handoff context, not current advice.
    next_step: data.guidance?.action?.note || null,
    pr_reference: pullRequestReference(data), missing: data.missing || {},
  };
}

function booleanOrUnknown(value) {
  return typeof value === 'boolean' ? value : null;
}

const WAIT_PARTY_KINDS = new Set(['person', 'group', 'external_party']);
const MAX_STRUCTURED_WAITS = 8;
const MAX_WAIT_PARTY_NAME_CHARS = 80;
const MAX_WAIT_TEXT_CHARS = 200;
const MAX_WAIT_SINCE_UNIX_MS = 4_102_444_800_000;

function isBoundedWaitText(value, maxChars) {
  if (typeof value !== 'string') return false;
  if (/[\x00-\x1f\x7f-\x9f\u2028\u2029]/.test(value)) return false;
  if (value.replace(/[\s\uFEFF\u200B]+/g, '').length === 0) return false;
  return Array.from(value).length <= maxChars;
}

function isBoundedCheckpointId(value) {
  return value === null || (typeof value === 'string' && value.length > 0
    && value.length <= 128 && !/[\x00-\x1f\x7f]/.test(value));
}

function isOptionalTimestampMs(value) {
  return value === null || (Number.isSafeInteger(value) && value >= 0);
}

function validStructuredWaitItem(item) {
  if (!item || typeof item !== 'object' || Array.isArray(item)) return false;
  const keys = Object.keys(item);
  if (keys.length !== 4 || !keys.every((k) => ['waiting_on', 'what', 'since_unix_ms', 'release_condition'].includes(k))) {
    return false;
  }
  const party = item.waiting_on;
  if (!party || typeof party !== 'object' || Array.isArray(party)) return false;
  const partyKeys = Object.keys(party);
  if (partyKeys.length !== 2 || !partyKeys.every((k) => ['kind', 'name'].includes(k))) return false;
  return WAIT_PARTY_KINDS.has(party.kind)
    && isBoundedWaitText(party.name, MAX_WAIT_PARTY_NAME_CHARS)
    && isBoundedWaitText(item.what, MAX_WAIT_TEXT_CHARS)
    && Number.isSafeInteger(item.since_unix_ms)
    && item.since_unix_ms >= 1
    && item.since_unix_ms <= MAX_WAIT_SINCE_UNIX_MS
    && isBoundedWaitText(item.release_condition, MAX_WAIT_TEXT_CHARS);
}

function validStructuredWaits(value) {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return false;
  const allowed = new Set([
    'status', 'items', 'provenance', 'grants_authority',
    'checkpoint_id', 'reported_at_unix_ms', 'contract_matches_current',
  ]);
  const keys = Object.keys(value);
  if (keys.length !== allowed.size || !keys.every((k) => allowed.has(k))) return false;
  if (value.provenance !== 'caller_declared' || value.grants_authority !== false) return false;
  if (!isBoundedCheckpointId(value.checkpoint_id) || !isOptionalTimestampMs(value.reported_at_unix_ms)) return false;
  if (value.contract_matches_current !== null && typeof value.contract_matches_current !== 'boolean') return false;
  if (value.status === 'not_provided') return value.items === null;
  if (value.status === 'provided') {
    return Array.isArray(value.items) && value.items.length <= MAX_STRUCTURED_WAITS
      && value.items.every(validStructuredWaitItem);
  }
  return false;
}

function mapStructuredWaits(value) {
  if (value === undefined || !validStructuredWaits(value)) {
    return { status: 'unknown', items: null };
  }
  return {
    status: value.status,
    items: value.status === 'provided'
      ? value.items.map((item) => ({
        waiting_on: { kind: item.waiting_on.kind, name: item.waiting_on.name },
        what: item.what,
        since_unix_ms: item.since_unix_ms,
        release_condition: item.release_condition,
      }))
      : null,
    provenance: 'caller_declared',
    grants_authority: false,
    checkpoint_id: value.checkpoint_id,
    reported_at_unix_ms: value.reported_at_unix_ms,
    contract_matches_current: value.contract_matches_current,
  };
}

function mapReconciliationCoverage(value) {
  if (value === undefined) return { status: 'unsupported' };
  const supported = value && typeof value === 'object' && !Array.isArray(value)
    && ['covered', 'insufficient', 'none'].includes(value.status)
    && value.basis === 'configured_authority' && value.live_access_not_guaranteed === true;
  return supported ? { status: value.status, basis: 'configured_authority', live_access_not_guaranteed: true }
    : { status: 'unknown' };
}

function createGithubObserver(fetchImpl = fetch, now = Date.now) {
  const cache = new Map();
  let blockedUntil = 0;
  let running = 0;
  const queue = [];
  function pump() {
    while (running < 2 && queue.length) {
      const { task, resolve, reject } = queue.shift();
      running++;
      Promise.resolve().then(task).then(resolve, reject).finally(() => { running--; pump(); });
    }
  }
  function schedule(task) {
    return new Promise((resolve, reject) => { queue.push({ task, resolve, reject }); pump(); });
  }
  async function read(path) {
    if (now() < blockedUntil) throw new Error('GitHub observation rate limited');
    const response = await fetchImpl('https://api.github.com' + path, {
      headers: { accept: 'application/vnd.github+json', 'user-agent': 'AWR-Observer' },
      redirect: 'error', signal: AbortSignal.timeout(5000),
    });
    const header = name => response.headers?.get(name);
    if (response.status === 403 || response.status === 429 || header('x-ratelimit-remaining') === '0') {
      const reset = Number(header('x-ratelimit-reset')) * 1000;
      const retry = header('retry-after');
      const retryAt = /^\d+$/.test(retry || '') ? now() + Number(retry) * 1000 : Date.parse(retry || '');
      const until = Math.max(reset || 0, retryAt || 0);
      blockedUntil = Math.max(blockedUntil, until > now() ? until + 1000 : now() + 900000);
    }
    if (!response.ok) throw new Error('GitHub observation unavailable');
    return response.json();
  }
  async function observe(reference) {
    if (!reference) return null;
    const key = reference.url, hit = cache.get(key);
    // Three anonymous requests per PR. Scale retention with the observed set
    // to stay below 60 requests/hour (45/hour with four-minute PR headroom).
    const successTtl = Math.max(300000, cache.size * 240000);
    if (hit && (hit.pending || now() < (hit.failed ? hit.expiresAt : hit.at + successTtl))) return hit.promise;
    // The cache contains public response metadata only, and stays bounded.
    if (cache.size >= 200) {
      const finished = [...cache].find(([, value]) => !value.pending);
      if (!finished) return { source: 'github_public_api', observed_at_ms: null, unavailable: true,
        reason: 'observation_capacity' };
      cache.delete(finished[0]);
    }
    const entry = { at: now(), failed: false, pending: true, value: hit?.value || null, expiresAt: now() + 300000 };
    entry.promise = schedule(async () => {
      try {
        const base = `/repos/${encodeURIComponent(reference.owner)}/${encodeURIComponent(reference.repo)}`;
        const pr = await read(`${base}/pulls/${reference.number}`);
        if (!/^[a-f0-9]{40}$/.test(pr.head?.sha || '') || pr.html_url !== reference.url) throw new Error('Invalid PR');
        let ci = 'unavailable';
        try {
          const [checks, statuses] = await Promise.all([
            read(`${base}/commits/${pr.head.sha}/check-runs?per_page=100`),
            read(`${base}/commits/${pr.head.sha}/status?per_page=100`),
          ]);
          const runs = checks.check_runs || [], contexts = statuses.statuses || [];
          const incomplete = checks.total_count > runs.length || statuses.total_count > contexts.length;
          if (incomplete) ci = 'incomplete';
          else if (runs.some(r => ['failure', 'timed_out', 'cancelled', 'action_required', 'startup_failure', 'stale'].includes(r.conclusion))
            || contexts.some(s => ['failure', 'error'].includes(s.state))) ci = 'failed';
          else if (runs.some(r => r.status !== 'completed') || contexts.some(s => s.state === 'pending')) ci = 'pending';
          else if (runs.length + contexts.length === 0) ci = 'not_reported';
          else if (runs.every(r => ['success', 'neutral', 'skipped'].includes(r.conclusion)) && contexts.every(s => s.state === 'success')) ci = 'passed';
        } catch (_) { /* Keep PR facts when the independent checks request fails. */ }
        return { source: 'github_public_api', observed_at_ms: now(), url: reference.url,
          head_sha: pr.head.sha, state: pr.merged_at ? 'merged' : pr.state, ci };
      } catch (_) {
        entry.failed = true;
        entry.expiresAt = Math.max(now() + 60000, blockedUntil);
        return { source: 'github_public_api', observed_at_ms: null, attempted_at_ms: now(), unavailable: true };
      }
    }).then(value => { entry.value = value; entry.pending = false; return value; });
    cache.set(key, entry);
    return entry.promise;
  };
  // AWR reads never await optional repository APIs. Public cached facts retain
  // their own time while a refresh runs independently; a first lookup has none.
  observe.background = reference => {
    if (!reference) return null;
    void observe(reference);
    const entry = cache.get(reference.url);
    if (!entry) return { source: 'github_public_api', observed_at_ms: null, unavailable: true,
      reason: 'observation_capacity', pending: false, cached: false };
    return { ...(entry.value || { source: 'github_public_api', url: reference.url, observed_at_ms: null }),
      pending: entry.pending, cached: Boolean(entry.value) };
  };
  return observe;
}

module.exports = { mapObservation, mapStructuredWaits, validStructuredWaits, pullRequestReference, createGithubObserver };
