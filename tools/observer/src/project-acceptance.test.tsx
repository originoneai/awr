import { createHash, webcrypto } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { act, within, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';
import i18n from './i18n';
import { formatUnixMs } from './lib/format';
import { revokeObservationReads } from './lib/queries';
import { DEMO_OVERVIEW, DEMO_PROJECTS, renderApp } from './test-utils';

// Verbatim service replies used by the review-regression cases:
// service-completion-inspect-api-run09.json,
// service-delivery-integration-inspect-api-run09.json, and
// service-delivery-integration-inspect-page-rejected-run09.json.
const LIVE_COMPLETION = JSON.parse(readFileSync(resolve('./test/fixtures/obs006/completion-inspect.json'), 'utf8'));
const LIVE_INTEGRATION = JSON.parse(readFileSync(resolve('./test/fixtures/obs006/delivery-integration-inspect.json'), 'utf8'));
const REJECTED_INTEGRATION = JSON.parse(readFileSync(resolve('./test/fixtures/obs006/delivery-integration-inspect-page-rejected.json'), 'utf8'));

const WORK = 'issue-filter-api';
const PAGE = 'issue-filter-page';
const ROUND = '01M6AVQ870N7MBRJ5MPCN79T55';
const PAGE_ROUND = '01M6B33RXNY0ZTRN3DQX45KDYR';
const EVIDENCE = '01M6ATNTRCBT4VEV44CYT66CWN';
const EXECUTION = '01M6ARNTMQCWJMWM5QQVEK51MC';
const CLAIM = '01M6ARHB295GP0PQ1P111N8MFC';
const ARTIFACT = '01M6ATNTRB664BA81892TRHKQ4';
const INTEGRATION = '01M6B0NNN221D857AA1YTBR66A';
const COMPLETION = '01M6B189DXC1A21ZD5YQ6V7EG5';
const CONTRACT = '295b63612f58425c516a25c6e4d5d777d1da6661cc11cce9fb93ffc3c9597c4d';
const CANDIDATE = '26361fecff514828e5de69770120ba7bca170ba5425197f38988fe60853a33e3';
const ARTIFACT_TEXT = 'PRIMARY REPORT\nExact bytes are verified.';
const DIGEST = createHash('sha256').update(ARTIFACT_TEXT, 'utf8').digest('hex');

// Fixtures are synthetic but follow the exact shapes of the run-09 samples:
// service-work-snapshot-api-delivered.json, service-review-inspect-api-open.json,
// service-evidence/execution/claim/completion-inspect-api-run09.json,
// service-delivery-source-status/integration-inspect-api-run09.json and
// service-artifact-content/segment-api-run09.json. No sample text is copied.
const WORKS = [
  { key: WORK, title: WORK, status: 'unknown', owner_person: null, agent: null, next_step: null, detail_loaded: false, workstream_id: 'WS1', contract_hash: CONTRACT },
  { key: PAGE, title: PAGE, status: 'unknown', owner_person: null, agent: null, next_step: null, detail_loaded: false, workstream_id: 'WS1', contract_hash: 'page-contract' },
];

function snapshotFor(workId: string, delivered: boolean) {
  const round = delivered ? ROUND : PAGE_ROUND;
  return {
    work_id: workId,
    contract_hash: delivered ? CONTRACT : 'page-contract',
    visible_contract: { goals: [`GOAL OF ${workId}`], acceptance: [`ACCEPT ${workId}`] },
    observation: {
      collaboration: {
        delivery_required: true,
        candidate: { current: true, digest: delivered ? CANDIDATE : 'page-candidate', selection_version: '1', required_checks: ['local_git.manifest'] },
        review: { id: round, state: delivered ? 'approved' : 'open', current_candidate: true, bundle_hash: 'bundle-hash', author_actor_id: 'lin' },
        integration: delivered ? { id: INTEGRATION, state: 'confirmed', current_binding: true, target_changed: false, reviewed_execution_id: EXECUTION } : null,
        publication: { id: 'publication-1', phase: 'confirmed', metadata_revision: '4', basis: 'recorded_publication_phase' },
        source_state: { observation: 'observed', source_synchronized: true, query_started_at_unix_ms: 1793519815935 },
        verification: [{ check: 'local_git.manifest', outcome: 'passed', run_id: 'local-git-run' }],
        facts_truncated: false,
      },
      runtime: { state: delivered ? 'completed' : 'claimed', selected_completion_id: delivered ? COMPLETION : null },
      claim: { id: CLAIM, lease_live: false, state: 'active' },
      reporting: { client_reported_at_unix_ms: 1793513588923 },
    },
    snapshot: { queried_at_unix_ms: 1793519815935 },
  };
}

function reviewFor(workId: string, delivered: boolean) {
  return {
    work_id: workId,
    round_id: delivered ? ROUND : PAGE_ROUND,
    round_index: delivered ? 2 : 1,
    state: delivered ? 'approved' : 'open',
    author_actor_id: 'lin-agent',
    contract_hash: delivered ? CONTRACT : 'page-contract',
    candidate_digest: delivered ? CANDIDATE : 'page-candidate',
    bundle_hash: 'bundle-hash',
    artifact_digest: DIGEST,
    execution_result_digest: DIGEST,
    evidence_id: EVIDENCE,
    execution_id: EXECUTION,
    evidence_provenance: {
      author_self_report: true,
      trust_basis: 'caller_asserted',
      binding_status: 'exact',
      currentness: 'current_contract',
      review_contract_matches_current: true,
      execution_consistency: { status: 'consistent', independent_verification: false, recorded_state: 'succeeded' },
      boundary: 'caller_asserted is not independent verification',
    },
    submission: {
      candidate_current: true,
      candidate_digest: delivered ? CANDIDATE : 'page-candidate',
      inspectable: true,
      reason: 'inspectable_current_submission',
      delivery_required: true,
      source_revision: { format: 'git_sha1', resource: 'local-repository:synthetic', value: '5f7eb2fe9c563e4d355aa64a527e0aef754116c1' },
      artifacts: [{
        manifest_artifact_id: null,
        primary_evidence: true,
        byte_length: '2753',
        query: { op: 'artifact.content', protocol_version: 1, artifact_id: ARTIFACT, expected_sha256: DIGEST, work_id: workId },
        segment_query: { op: 'artifact.segment', protocol_version: 1, artifact_id: ARTIFACT, expected_sha256: DIGEST, work_id: workId },
      }],
    },
    decisions: delivered ? [{
      decision: 'approve', decision_id: 'decision-1', reviewer_actor_id: 'chen-agent', reviewer_person_id: 'chen',
      human_approval: false, independence_kind: 'simulated_member_independent', approval_basis: 'simulated_member_independent_review',
      team_independent_acceptance: false, reason: 'Approved the exact candidate; this is not human approval.',
    }] : [],
    findings: {
      findings: [{ finding_key: 'finding-1', round: 1, criterion: 'Combine filters', rule: 'Retain tests', severity: 'blocking', state: 'resolved', disposition: 'returns_work', summary: 'Original work finding' }],
      deferred: [], open_blocking: [],
    },
  };
}

function envelope(data: unknown, workId = WORK) {
  return {
    protocol_version: 1,
    scope_id: 'main',
    source_snapshot_id: 'snap-1',
    coordinator_epoch: 'epoch-1',
    project_revision: '104',
    workstream_id: 'WS1',
    data,
    work_id: workId,
  };
}

function setupAcceptance({ failOps = [], failWith = {} }: { failOps?: string[]; failWith?: Record<string, string> } = {}) {
  const calls: Array<{ url: string; query?: Record<string, unknown> }> = [];
  const snapshots = { [WORK]: snapshotFor(WORK, true), [PAGE]: snapshotFor(PAGE, false) };
  const reviews = { [WORK]: reviewFor(WORK, true), [PAGE]: reviewFor(PAGE, false) };
  const completion = {
    completion: {
      receipt_id: COMPLETION, contract_hash: CONTRACT, evidence_id: EVIDENCE, execution_id: EXECUTION,
      review_round_id: ROUND, review_decision_id: 'decision-1', independence_kind: 'simulated_member_independent',
      approved_by: { human_approval: false, team_independent_acceptance: false, caller_execution_binding: { artifact_verified: true, effects_settled: true } },
    },
    runtime_state: 'completed', selected_completion_id: COMPLETION,
  };
  const deliveryByWork = {
    [WORK]: { delivery: { work_id: WORK, awr_acceptance: { complete: true, runtime_state: 'completed', selected_completion_id: COMPLETION }, cannot_skip_acceptance_via: ['green_ci', 'already_merged'], github: { approved: false, merged: false, submitted: false }, pr: null, webhook_auto_sync: false } },
    [PAGE]: { delivery: { work_id: PAGE, awr_acceptance: { complete: false, runtime_state: 'claimed', selected_completion_id: null }, cannot_skip_acceptance_via: ['green_ci', 'already_merged'], github: { approved: false, merged: false, submitted: false }, pr: null, webhook_auto_sync: false } },
  };
  const integration: Record<string, unknown> = {
    integration_id: INTEGRATION,
    integration_request: { binding: { target: { reference: 'refs/heads/main', resource: 'local-repository:synthetic', precondition: { kind: 'exact', revision: { value: 'a516342436e434d0072ef9c138f419f2f128caac' } } }, source_revision: { value: '5f7eb2fe9c563e4d355aa64a527e0aef754116c1' } } },
    dispatch_receipt: { data: { state: 'dispatched' }, lease_id: 'lease-1', recorded_at_unix_ms: 1793518656318 },
    confirmation: { envelope: { record: { data: { outcome: 'applied', result_revision: { value: '5f7eb2fe9c563e4d355aa64a527e0aef754116c1' }, provenance: { observed_at_unix_ms: 1793518656287, recorded_at_unix_ms: 1793518656318, source: 'adapter_observation' } } } } },
    current_binding: true, confirmation_current: true, target_changed: false, request_binding: 'original_candidate_and_read_set', dispatched_at: '2026-11-01T07:37:35Z', state: 'confirmed',
  };
  const rounds = Object.fromEntries(Object.entries(reviews).map(([work, review]) => [work, [{
    round_id: review.round_id, round_index: review.round_index, round_position: 1, readable: true,
    state: review.state, contract_matches_current: true, reviewer_role: 'simulated_member_reviewer',
    reviewer_actor_id: 'chen-agent', reviewer_person_id: 'chen', decided_at_unix_ms: 1793518656287,
    reason_first_line: review.decisions[0]?.reason ?? null,
  }]]));
  const activity = { items: [], scope: 'self', next_cursor: null, chat_text_collected: false, tool_io_collected: false };
  const payloads = { snapshots, reviews, completion, deliveryByWork, integration, rounds, activity };
  const fetchMock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
    const url = String(input);
    let query: Record<string, unknown> | undefined;
    if (url.includes('/api/team/read')) query = JSON.parse(String(init?.body)).query;
    calls.push({ url, query });
    if (url.includes('/api/team/projects')) return json(DEMO_PROJECTS);
    if (url.includes('/api/team/activity')) return json({ ok: true, data: activity });
    if (url.includes('/api/team/overview')) {
      return json({ ...DEMO_OVERVIEW, works: WORKS, workstreams: [{ id: 'WS1', external_key: 'support', title: 'Support' }] });
    }
    if (query) {
      if (failOps.includes(String(query.op))) return json({ ok: false, error: { code: failWith[String(query.op)] ?? 'ReadTimeout', message: 'slow' } });
      let data: unknown;
      switch (query.op) {
        case 'work.snapshot': data = snapshots[query.work_id as keyof typeof snapshots]; break;
        case 'review.inspect': data = { review: reviews[query.work_id as keyof typeof reviews] }; break;
        case 'review.rounds': data = { work_id: query.work_id, items: rounds[String(query.work_id)], total: rounds[String(query.work_id)].length, next_cursor: null, as_of: { at_unix_ms: 1793518656287 } }; break;
        case 'evidence.inspect':
          data = { evidence: { work_id: WORK, evidence_id: EVIDENCE, execution_id: EXECUTION, artifact_id: ARTIFACT, artifact_digest: DIGEST, digest: 'evidence-digest', contract_hash: CONTRACT, trust_basis: 'caller_asserted', execution_consistency: { independent_verification: false, status: 'consistent' } } };
          break;
        case 'execution.inspect':
          data = { work_id: WORK, execution_id: EXECUTION, claim_id: CLAIM, state: 'succeeded', artifact_verified: true, artifact_verification_basis: 'current_completion_readable_artifact_digest', effects_settled: true, contract_matches_current: true, epoch_matches_current: true, execution_authorized: false, attestation_authority: false, settlement_basis: 'caller_asserted', terminal_reported: true };
          break;
        case 'claim.inspect':
          data = { claim_id: CLAIM, current_fence: true, epoch_matches_current: true, lease_live: false, lease_version: '1', expires_at: '2026-11-01T06:15:24Z', state: 'active', execution_authorized: false, execution_eligibility_advisory: false, owned_by_client: false };
          break;
        case 'completion.inspect': data = completion; break;
        case 'delivery.inspect': data = deliveryByWork[query.work_id as keyof typeof deliveryByWork]; break;
        case 'delivery.neutral.inspect':
          data = { work_id: WORK, connectors: [{ connector_id: 'repository-api', connector_version: '1', current_epoch: true, enabled: true, fact_source: 'adapter_observation', provider: 'local_git', resource: 'local-repository:synthetic' }], connectors_truncated: false, selected_current: true, selection_version: '1' };
          break;
        case 'delivery.submission.describe':
          data = { work_id: WORK, read_only: true, stored_selection: { binding_digest: CANDIDATE, current: true, selection_version: '1' }, connectors: [{ provider: 'local_git', enabled: true, current_epoch: true }], local_git: { basis: 'local_git.manifest verifies committed bytes' } };
          break;
        case 'delivery.integration.inspect': data = integration; break;
        case 'delivery.source.status':
          data = { work_id: WORK, source_observation: 'observed', confirmed_fingerprint: 'sha256:confirmed', source_fingerprint: 'sha256:confirmed', source_synchronized: true, cursor_current: true, projection_current: true, pending_publication_id: null, metadata_revision: '5', history: [{ phase: 'confirmed', source_synchronized_at_commit: true, source_written_observed: true, confirmation: { operation_effects: { source_metadata_written: true, work_finalized: false }, effect_basis: 'this_publication_operation; not current_work_state' } }], history_truncated: false };
          break;
        case 'delivery.exports': data = { adoption_available: true, adoption_version_policies: ['current_contract', 'fixed_delivery'], items: [], next_cursor: null, upstream_source_access: false }; break;
        case 'artifact.content':
          data = { work_id: query.work_id, artifact_id: query.artifact_id, byte_length: Buffer.byteLength(ARTIFACT_TEXT, 'utf8'), sha256: DIGEST, text: ARTIFACT_TEXT, content_base64: null };
          break;
        case 'artifact.segment':
          data = { artifact_id: query.artifact_id, work_id: query.work_id, byte_length: Buffer.byteLength(ARTIFACT_TEXT, 'utf8'), content_base64: Buffer.from(ARTIFACT_TEXT, 'utf8').toString('base64'), range: { start: 0, end_exclusive: Buffer.byteLength(ARTIFACT_TEXT, 'utf8') }, segment_sha256: DIGEST, sha256: DIGEST, disclosure_sha256: null, end_of_artifact: true, next_cursor: null, next_query: null };
          break;
        default: return json({ ok: false, error: { code: 'NoRoute', message: String(query.op) } });
      }
      return json({ ok: true, result: envelope(data, String(query.work_id ?? WORK)), query, freshness: freshness() });
    }
    return json({ ok: false, error: { code: 'NoRoute', message: url } });
  });
  vi.stubGlobal('fetch', fetchMock);
  return { calls, payloads };
}

function freshness() {
  return {
    report_received: { status: 'not_measured' }, authoritative: { status: 'not_measured' },
    page_read: { status: 'measured', at_unix_ms: 1793520000000 },
  };
}

function json(payload: unknown) {
  return new Response(JSON.stringify(payload), { status: 200, headers: { 'content-type': 'application/json' } });
}

async function waitForAcceptanceReads(queryClient: ReturnType<typeof renderApp>['queryClient']) {
  await screen.findByTestId('acceptance-page');
  for (let pass = 0; pass < 6; pass += 1) {
    await act(async () => { await new Promise((resolve) => setTimeout(resolve, 0)); });
    await waitFor(() => expect(queryClient.isFetching()).toBe(0), { timeout: 10000 });
    await act(async () => { await new Promise((resolve) => setTimeout(resolve, 0)); });
    return;
  }
  throw new Error('Acceptance reads did not settle');
}

function renderAcceptance(path: string, { expand = true }: { expand?: boolean } = {}) {
  const result = renderApp(path);
  if (expand) return waitForAcceptanceReads(result.queryClient).then(() => result);
  return result;
}

type AcceptancePayloads = ReturnType<typeof setupAcceptance>['payloads'];

function installLiveCompletion(payloads: AcceptancePayloads) {
  Object.assign(payloads.completion, structuredClone(LIVE_COMPLETION.data));
}

async function expectLiveCompletionConflict(mutate: (payloads: AcceptancePayloads) => void) {
  const { payloads } = setupAcceptance();
  installLiveCompletion(payloads);
  mutate(payloads);
  await renderAcceptance('/projects/demo?tab=acceptance&work=issue-filter-api');
  expect(await screen.findByTestId('acceptance-completion-conflict')).toBeTruthy();
  expect(screen.queryByTestId('acceptance-completion')).toBeNull();
  expect(screen.getByTestId('delivery-step-acceptance').textContent).not.toContain('业务验收已完成');
  expect(screen.queryByTestId('completion-independence')).toBeNull();
}

afterEach(async () => {
  vi.unstubAllGlobals();
  await act(async () => { await i18n.changeLanguage('zh-CN'); });
});

describe('project acceptance evidence (AWR-OBS-006)', () => {
  it.each(['contract', 'evidence', 'execution', 'review', 'delivery', 'currentContract'])(
    'keeps completion unconfirmed when the %s binding is missing', async (field) => {
      const { payloads } = setupAcceptance();
      const receipt = payloads.completion.completion as unknown as Record<string, unknown>;
      if (field === 'contract') delete receipt.contract_hash;
      if (field === 'evidence') delete receipt.evidence_id;
      if (field === 'execution') delete receipt.execution_id;
      if (field === 'review') delete receipt.review_round_id;
      if (field === 'delivery') delete (payloads.deliveryByWork[WORK].delivery.awr_acceptance as Record<string, unknown>).selected_completion_id;
      if (field === 'currentContract') delete (payloads.reviews[WORK] as unknown as Record<string, unknown>).contract_hash;
      await renderAcceptance('/projects/demo?tab=acceptance&work=issue-filter-api');
      expect(screen.getByTestId('delivery-step-acceptance').textContent).not.toContain('已完成');
      expect(screen.getByTestId('delivery-step-acceptance').textContent).toMatch(/结果未知|待核对|记录不一致/);
      expect(screen.queryByTestId('completion-independence')).toBeNull();
    },
  );

  it('keeps a denied history read local to that panel', async () => {
    setupAcceptance({ failOps: ['review.rounds'], failWith: { 'review.rounds': 'Forbidden' } });
    await renderAcceptance('/projects/demo?tab=acceptance&work=issue-filter-api');
    expect(await screen.findByTestId('state-forbidden')).toBeTruthy();
    expect(screen.getByTestId('acceptance-evidence-chain').textContent).toContain('业务验收已完成');
  });

  it('accepts an explicitly recorded human-reviewed report without an execution', async () => {
    const { payloads } = setupAcceptance();
    const collaboration = payloads.snapshots[WORK].observation.collaboration;
    collaboration.delivery_required = false;
    collaboration.candidate = null as never;
    collaboration.integration = null;
    payloads.reviews[WORK].submission.delivery_required = false;
    payloads.reviews[WORK].submission.candidate_current = false;
    payloads.reviews[WORK].execution_id = null as never;
    payloads.completion.completion.execution_id = null as never;
    payloads.reviews[WORK].evidence_provenance.author_self_report = false;
    payloads.reviews[WORK].evidence_provenance.execution_consistency.independent_verification = true;
    payloads.reviews[WORK].decisions[0].human_approval = true;
    payloads.reviews[WORK].decisions[0].independence_kind = 'team_independent';
    payloads.reviews[WORK].decisions[0].approval_basis = 'human_independent_review';
    payloads.completion.completion.approved_by.human_approval = true;
    Object.assign(payloads.completion.completion.approved_by, { independence_kind: 'team_independent' });
    await renderAcceptance('/projects/demo?tab=acceptance&work=issue-filter-api');
    expect(screen.getByTestId('delivery-step-acceptance').textContent).toBe('已完成');
  });

  it('discards a cached completion after work is returned for another review', async () => {
    const { payloads } = setupAcceptance();
    const { queryClient } = await renderAcceptance('/projects/demo?tab=acceptance&work=issue-filter-api');
    expect(screen.getByTestId('delivery-step-acceptance').textContent).toBe('已完成');
    Object.assign(payloads.snapshots[WORK].observation.runtime, { state: 'claimed', selected_completion_id: null });
    Object.assign(payloads.snapshots[WORK].observation.collaboration.review, { id: PAGE_ROUND, state: 'open' });
    Object.assign(payloads.reviews[WORK], { round_id: PAGE_ROUND, state: 'open' });
    Object.assign(payloads.deliveryByWork[WORK].delivery.awr_acceptance, { complete: false, runtime_state: 'claimed', selected_completion_id: null });
    await act(async () => { await queryClient.invalidateQueries({ queryKey: ['team'] }); });
    await screen.findByTestId(`acceptance-review-${PAGE_ROUND}`);
    await waitFor(() => expect(screen.getByTestId('delivery-step-acceptance').textContent).toBe('还没有完成记录'));
    expect(screen.queryByTestId('acceptance-completion-conflict')).toBeNull();
  });

  it('keeps denied activity local while current acceptance remains readable', async () => {
    setupAcceptance();
    const original = globalThis.fetch;
    vi.stubGlobal('fetch', (input: RequestInfo | URL, init?: RequestInit) => String(input).includes('/api/team/activity')
      ? Promise.resolve(json({ ok: false, error: { code: 'Forbidden', message: 'private-activity-denial' } }))
      : original(input, init));
    await renderAcceptance('/projects/demo?tab=acceptance&work=issue-filter-api');
    expect(await screen.findByTestId('state-forbidden')).toBeTruthy();
    expect(screen.getByTestId('delivery-step-acceptance').textContent).toBe('已完成');
    expect(document.body.textContent).not.toContain('private-activity-denial');
  });

  it('labels required dependencies as upstream work', async () => {
    const { payloads } = setupAcceptance();
    Object.assign(payloads.snapshots[WORK].visible_contract, { required_dependencies: [PAGE] });
    await renderAcceptance('/projects/demo?tab=acceptance&work=issue-filter-api');
    expect(screen.getByText('依赖的工作')).toBeTruthy();
    expect(screen.queryByText('影响的下游工作')).toBeNull();
  });

  it.each([
    ['team_independent', '独立复核'],
    ['personal_self_review', '作者自查'],
    ['agent_review', 'Agent 复核'],
    ['future_private_review_kind', '复核方式未记录'],
  ])('describes the recorded %s review method in product language', async (kind, label) => {
    const { payloads } = setupAcceptance();
    Object.assign(payloads.completion.completion.approved_by, { independence_kind: kind });
    await renderAcceptance('/projects/demo?tab=acceptance&work=issue-filter-api');
    expect(screen.getByTestId('completion-independence').textContent).toBe(label);
    expect(document.body.textContent).not.toContain(kind);
  });

  it('accepts report-only work without requiring repository integration or a repository candidate', async () => {
    const { payloads } = setupAcceptance();
    const collaboration = payloads.snapshots[WORK].observation.collaboration;
    collaboration.delivery_required = false;
    collaboration.candidate = null as never;
    collaboration.integration = null;
    payloads.reviews[WORK].submission.delivery_required = false;
    payloads.reviews[WORK].submission.candidate_current = false;
    await renderAcceptance('/projects/demo?tab=acceptance&work=issue-filter-api');
    expect(screen.getByTestId(`acceptance-work-${WORK}`)).toBeTruthy();
    expect(screen.getByTestId('acceptance-evidence-chain').textContent).toContain('业务验收已完成');
    expect(screen.queryByTestId('acceptance-review-history')).toBeNull();
    expect(screen.getByTestId('delivery-step-integration').textContent).toContain('无需并入代码仓库');
  });

  it('keeps current completion separate from the historical review being read', async () => {
    const { payloads, calls } = setupAcceptance();
    const earlier = '01J000000000000000000000099';
    const earlierReview = structuredClone(payloads.reviews[WORK]);
    Object.assign(earlierReview, { round_id: earlier, round_index: 1, state: 'rejected', evidence_id: 'earlier-evidence', execution_id: 'earlier-execution' });
    const original = globalThis.fetch;
    vi.stubGlobal('fetch', async (input: RequestInfo | URL, init?: RequestInit) => {
      const query = init?.body ? JSON.parse(String(init.body)).query : null;
      if (query?.op === 'review.inspect' && query.review_round_id === earlier) {
        calls.push({ url: String(input), query });
        return json({ ok: true, result: envelope({ review: earlierReview }), freshness: freshness() });
      }
      return original(input, init);
    });
    await renderAcceptance(`/projects/demo?tab=acceptance&work=${WORK}&round=${earlier}`);
    expect(screen.getByTestId(`acceptance-review-${earlier}`)).toBeTruthy();
    expect(screen.queryByTestId('acceptance-completion-conflict')).toBeNull();
    expect(screen.getByTestId('acceptance-evidence-chain').textContent).toContain('业务验收已完成');
    expect(calls.some(call => call.query?.op === 'review.inspect' && call.query.review_round_id === ROUND)).toBe(true);
  });

  it('offers the exact primary report when a report-only review has no repository manifest', async () => {
    const { payloads, calls } = setupAcceptance();
    payloads.reviews[WORK].submission.artifacts = [];
    await renderAcceptance('/projects/demo?tab=acceptance&work=issue-filter-api');
    expect(await screen.findByTestId(`artifact-expand-${ARTIFACT}`)).toBeTruthy();
    expect(calls.some(call => call.query?.op === 'artifact.content')).toBe(false);
    await userEvent.setup().click(screen.getByTestId(`artifact-expand-${ARTIFACT}`));
    expect(await screen.findByTestId(`artifact-read-content-${ARTIFACT}`)).toBeTruthy();
    expect(calls.find(call => call.query?.op === 'artifact.content')?.query).toMatchObject({ work_id: WORK, artifact_id: ARTIFACT, expected_sha256: DIGEST });
  });

  it('reports an unreadable selected snapshot as unknown instead of absent records', async () => {
    setupAcceptance({ failOps: ['work.snapshot'] });
    await renderAcceptance('/projects/demo?tab=acceptance&work=issue-filter-api');
    const chain = await screen.findByTestId('acceptance-evidence-chain');
    expect(chain.textContent).toContain('结果未知');
    expect(chain.textContent).not.toContain('尚未生成');
    expect(chain.textContent).not.toContain('未记录');
    expect(await screen.findByTestId('acceptance-snapshot-error')).toBeTruthy();
  });

  it('keeps raw diagnostic groups and identifiers out of the interface', async () => {

    setupAcceptance();
    await renderAcceptance('/projects/demo?tab=acceptance&work=issue-filter-api');
    expect(document.querySelector('[data-testid="technical-details"]')).toBeNull();
    for (const value of [EVIDENCE, CLAIM, CONTRACT, CANDIDATE, COMPLETION]) expect(document.body.textContent).not.toContain(value);
    expect(screen.getByTestId('acceptance-evidence-chain').textContent).toContain('业务验收已完成');
  });

  it('does not call a completion accepted before mainline integration is applied', async () => {
    const { payloads } = setupAcceptance();
    const confirmation = payloads.integration.confirmation as { envelope: { record: { data: { outcome: string } } } };
    confirmation.envelope.record.data.outcome = 'recorded';
    await renderAcceptance('/projects/demo?tab=acceptance&work=issue-filter-api');
    const chain = await screen.findByTestId('acceptance-evidence-chain');
    expect(chain.textContent).toContain('待并入主线');
    expect(chain.textContent).toContain('完成记录待核对');
    expect(chain.textContent).not.toContain('已并入主线');
    expect(chain.textContent).not.toContain('已完成');
  });

  it('marks business acceptance unknown when the integration read fails', async () => {
    setupAcceptance({ failOps: ['delivery.integration.inspect'] });
    await renderAcceptance('/projects/demo?tab=acceptance&work=issue-filter-api');
    await screen.findByTestId('acceptance-evidence-chain');
    const step = screen.getByText('业务验收').closest('.ant-steps-item');
    expect(step?.textContent).toContain('结果未知');
  });

  it('marks the repository integration step unknown, not pending, when the integration read fails', async () => {
    setupAcceptance({ failOps: ['delivery.integration.inspect'] });
    await renderAcceptance('/projects/demo?tab=acceptance&work=issue-filter-api');
    await screen.findByTestId('acceptance-evidence-chain');
    const step = screen.getByText('仓库集成').closest('.ant-steps-item');
    expect(step?.textContent).toContain('结果未知');
    expect(step?.textContent).not.toContain('待并入主线');
    expect(step?.textContent).not.toContain('还没准备');
  });

  it('localizes review states in the outcome source list (review r1.4)', async () => {
    setupAcceptance();
    await renderAcceptance('/projects/demo?tab=acceptance&work=issue-filter-api');
    expect((await screen.findByTestId('acceptance-work-issue-filter-api')).textContent).toContain('已批准');
    expect(screen.getByTestId('acceptance-work-issue-filter-page').textContent).toContain('待复核');
  });

  it('uses a missing-receipt label instead of a runtime state in the completion step (review r1.6)', async () => {
    setupAcceptance();
    await renderAcceptance('/projects/demo?tab=acceptance&work=issue-filter-page');
    const chain = await screen.findByTestId('acceptance-evidence-chain');
    expect(chain.textContent).toContain('还没有完成记录');
    expect(chain.textContent).not.toContain('已认领');
    await i18n.changeLanguage('en-US');
    expect(chain.textContent).toContain('No completion record yet');
    await i18n.changeLanguage('zh-CN');
  });

  it('loads exact evidence, execution, claim, completion, integration and source facts', async () => {

    const { calls } = setupAcceptance();
    await renderAcceptance('/projects/demo?tab=acceptance&work=issue-filter-api');
    await waitFor(() => expect(calls.some(call => call.query?.op === 'completion.inspect')).toBe(true));
    for (const op of ['evidence.inspect', 'execution.inspect', 'claim.inspect', 'completion.inspect', 'delivery.integration.inspect', 'delivery.source.status']) expect(calls.some(call => call.query?.op === op)).toBe(true);
    for (const value of [EVIDENCE, EXECUTION, CLAIM, COMPLETION, INTEGRATION, CONTRACT]) expect(document.body.textContent).not.toContain(value);
    expect(screen.getByTestId('acceptance-evidence-chain').textContent).toContain('业务验收已完成');
  });

  it('describes a verification check without its internal operation name', async () => {
    setupAcceptance();
    await renderAcceptance('/projects/demo?tab=acceptance&work=issue-filter-api');
    const card = await screen.findByTestId('acceptance-verification');
    expect(card.textContent).toContain('本地内容与测试');
    expect(card.textContent).not.toContain('local_git.manifest');
    expect(document.body.textContent).not.toContain('local_git.manifest');
    await act(async () => { await i18n.changeLanguage('en-US'); });
    expect((await screen.findByTestId('acceptance-verification')).textContent).toContain('Local content and tests');
    await act(async () => { await i18n.changeLanguage('zh-CN'); });
  });

  it('uses exact selectors per object and never eagerly reads artifact bytes', async () => {
    const { calls } = setupAcceptance();
    await renderAcceptance('/projects/demo?tab=acceptance&work=issue-filter-api', { expand: false });
    const byOp = (op: string) => calls.filter((call) => call.query?.op === op).map((call) => call.query);
    await waitFor(() => {
      expect(byOp('completion.inspect')).not.toHaveLength(0);
      expect(byOp('claim.inspect')).not.toHaveLength(0);
    });
    expect(byOp('review.inspect')[0]).toMatchObject({ work_id: WORK, review_round_id: ROUND });
    expect(byOp('evidence.inspect')[0]).toMatchObject({ work_id: WORK, evidence_id: EVIDENCE });
    expect(byOp('execution.inspect')[0]).toMatchObject({ work_id: WORK, execution_id: EXECUTION });
    expect(byOp('claim.inspect')[0]).toMatchObject({ claim_id: CLAIM });
    expect(byOp('delivery.integration.inspect')[0]).toMatchObject({ work_id: WORK, request_id: INTEGRATION });
    expect(byOp('artifact.content')).toHaveLength(0);
    expect(byOp('artifact.segment')).toHaveLength(0);
  });

  it('switches work with round-scoped reads and clears the previous completion', async () => {
    setupAcceptance();
    await renderAcceptance('/projects/demo?tab=acceptance&work=issue-filter-api');
    await screen.findByTestId('acceptance-evidence-chain');
    const user = userEvent.setup();
    await user.click(screen.getByTestId(`acceptance-work-${PAGE}`));
    expect(await screen.findByTestId(`acceptance-review-${PAGE_ROUND}`)).toBeTruthy();
    expect(screen.queryByTestId('acceptance-completion')).toBeNull();
    expect(screen.getByTestId('acceptance-page').textContent).not.toContain(COMPLETION);
  });

  it('keeps an old candidate approval historical rather than current', async () => {
    const { payloads } = setupAcceptance();
    payloads.reviews[WORK].submission.candidate_current = false;
    await renderAcceptance('/projects/demo?tab=acceptance&work=issue-filter-api');
    expect(await screen.findByTestId('acceptance-review-history')).toBeTruthy();
    expect(screen.queryByTestId('acceptance-current-approval')).toBeNull();
  });

  it('labels findings with their original work, round and criterion', async () => {

    setupAcceptance();
    await renderAcceptance('/projects/demo?tab=acceptance&work=issue-filter-api');
    expect(screen.getByTestId('review-finding').textContent).toContain('Combine filters');
    expect(screen.getByTestId('review-finding').textContent).not.toContain(WORK);
    expect(document.querySelector('[data-testid="technical-details"]')).toBeNull();
  });

  it('shows rework reasons, findings and counterexamples as product facts', async () => {
    const { payloads } = setupAcceptance();
    payloads.snapshots[PAGE].observation.collaboration.review.state = 'rejected';
    const review = payloads.reviews[PAGE];
    review.state = 'rejected';
    review.decisions = [{
      ...payloads.reviews[WORK].decisions[0], decision: 'reject',
      reason: 'Return this version because an earlier response replaces the selected result.',
    }];
    Object.assign(review.findings.findings[0], {
      state: 'open', summary: 'The visible list can disagree with the chosen filter.',
      counterexample: { given: 'Two requests overlap.', when: 'The earlier request finishes last.', then: 'The previous results replace the current selection.' },
    });
    await renderAcceptance('/projects/demo?tab=acceptance&work=issue-filter-page', { expand: false });
    expect(await screen.findByText(review.decisions[0].reason)).toBeTruthy();
    expect(screen.getByText('The visible list can disagree with the chosen filter.')).toBeTruthy();
    expect(screen.getByTestId('review-finding').textContent).toContain('阻塞');
    expect(screen.getByText(/Two requests overlap\./)).toBeTruthy();
    expect(screen.getByText(/The earlier request finishes last\./)).toBeTruthy();
    expect(screen.getByText(/The previous results replace the current selection\./)).toBeTruthy();
    expect(screen.getByTestId(`acceptance-work-${PAGE}`).textContent).toContain('未通过');
    expect(screen.queryByTestId('acceptance-review')).toBeNull();
  });

  it.each(['zh-CN', 'en-US'])('keeps review meaning while removing machine references in %s', async (language) => {
    await i18n.changeLanguage(language);
    const { payloads } = setupAcceptance();
    const review = payloads.reviews[WORK];
    review.decisions[0].reason = `Check the selected result after delivery.finalize ${EXECUTION}.`;
    Object.assign(review.findings.findings[0], {
      summary: `Preserve the selected filter result ${CONTRACT}.`,
      criterion: 'Keep the result after session.checkpoint.',
      counterexample: { given: 'Two requests overlap.', when: 'internal_future_action finishes.', then: `The selected result ${EVIDENCE} must remain.` },
    });
    await renderAcceptance('/projects/demo?tab=acceptance&work=issue-filter-api', { expand: false });
    const explanation = await screen.findByTestId('review-explanation');
    expect(explanation.textContent).toContain('Check the selected result');
    expect(screen.getByTestId('review-finding').textContent).toContain('Preserve the selected filter result');
    for (const raw of [EXECUTION, CONTRACT, EVIDENCE, 'delivery.finalize', 'session.checkpoint', 'internal_future_action']) {
      expect(document.body.textContent).not.toContain(raw);
    }
    expect(document.querySelector('[data-testid="technical-details"]')).toBeNull();
  });

  it('does not carry a previous task explanation into another task review', async () => {
    const { payloads } = setupAcceptance();
    await renderAcceptance('/projects/demo?tab=acceptance&work=issue-filter-api', { expand: false });
    const reason = payloads.reviews[WORK].decisions[0].reason;
    expect(await screen.findByText(reason)).toBeTruthy();
    await userEvent.setup().click(screen.getByTestId(`acceptance-work-${PAGE}`));
    await screen.findByTestId(`acceptance-review-${PAGE_ROUND}`);
    await waitFor(() => expect(screen.queryByText(reason)).toBeNull());
    expect(screen.queryByTestId('acceptance-current-approval')).toBeNull();
  });

  it('reports unavailable review details without calling them unreported', async () => {
    setupAcceptance({ failOps: ['review.inspect'] });
    await renderAcceptance('/projects/demo?tab=acceptance&work=issue-filter-page', { expand: false });
    expect(await screen.findByTestId('acceptance-review-explanation-error')).toBeTruthy();
    expect(screen.queryByText('尚未提供复核说明。')).toBeNull();
  });

  it('distinguishes a readable review with no supplied explanation from a failed read', async () => {
    const { payloads } = setupAcceptance();
    payloads.reviews[PAGE].findings.findings = [];
    await renderAcceptance('/projects/demo?tab=acceptance&work=issue-filter-page', { expand: false });
    expect(await screen.findByText('尚未提供复核说明。')).toBeTruthy();
    expect(screen.queryByTestId('acceptance-review-explanation-error')).toBeNull();
  });

  it('shows the decision human_approval value instead of hiding it', async () => {

    setupAcceptance();
    await renderAcceptance('/projects/demo?tab=acceptance&work=issue-filter-api');
    expect(screen.getByTestId('decision-human-approval').textContent).toBe('未记录人工批准');
    expect(document.body.textContent).not.toContain('human_approval');
  });

  it('requires candidate_current true for current approval and reports the fact as not provided', async () => {
    const { payloads } = setupAcceptance();
    payloads.reviews[WORK].submission.candidate_current = undefined as never;
    await renderAcceptance('/projects/demo?tab=acceptance&work=issue-filter-api');
    expect(screen.queryByTestId('acceptance-current-approval')).toBeNull();
    expect((await screen.findByTestId('acceptance-review-currentness')).textContent).toBe('未提供');
  });

  it('disables segment mode when the review has no segment_query', async () => {

    const { payloads, calls } = setupAcceptance();
    payloads.reviews[WORK].submission.artifacts[0].segment_query = undefined as never;
    await renderAcceptance('/projects/demo?tab=acceptance&work=issue-filter-api');
    await userEvent.setup().click(screen.getByTestId(`artifact-expand-${ARTIFACT}`));
    await screen.findByTestId(`artifact-read-content-${ARTIFACT}`);
    expect(screen.queryByText('分段读取')).toBeNull();
    expect(calls.filter(call => call.query?.op === 'artifact.content')).toHaveLength(1);
    expect(calls.filter(call => call.query?.op === 'artifact.segment')).toHaveLength(0);
  });

  it('shows self-report and non-independence without inventing automatic sync', async () => {
    setupAcceptance();
    await renderAcceptance('/projects/demo?tab=acceptance&work=issue-filter-api');
    expect((await screen.findAllByText(/由提交者自己说明/)).length).toBeGreaterThan(0);
    expect((await screen.findAllByText(/尚未由他人独立检查/)).length).toBeGreaterThan(0);
    expect(screen.queryByText(/后台自动同步/)).toBeNull();
    expect(screen.getByTestId('acceptance-times')).toBeTruthy();
  });

  it('formats all four clocks with the shared formatter and keeps raw values in tooltips', async () => {

    setupAcceptance();
    await renderAcceptance('/projects/demo?tab=acceptance&work=issue-filter-api');
    const observed = 1793518656287;
    const recorded = 1793518656318;
    const times = screen.getByTestId('acceptance-times');
    expect(times.textContent).toContain(formatUnixMs(observed, 'zh-CN'));
    expect(times.textContent).toContain(formatUnixMs(recorded, 'zh-CN'));
    await userEvent.setup().hover(within(times).getAllByText(formatUnixMs(observed, 'zh-CN'))[0]);
    expect(document.querySelector('[role="tooltip"]')).toBeNull();
    expect(document.body.textContent).not.toContain(String(observed));
    await act(async () => { await i18n.changeLanguage('en-US'); });
    expect(times.textContent).toContain(formatUnixMs(observed, 'en-US'));
    await act(async () => { await i18n.changeLanguage('zh-CN'); });
  });

  it('marks missing clock facts as not provided instead of borrowing another clock', async () => {
    const { payloads } = setupAcceptance();
    const confirmation = payloads.integration.confirmation as { envelope: { record: { data: { provenance: Record<string, unknown> } } } };
    delete confirmation.envelope.record.data.provenance.observed_at_unix_ms;
    payloads.snapshots[WORK].observation.collaboration.source_state = undefined as never;
    await renderAcceptance('/projects/demo?tab=acceptance&work=issue-filter-api');
    const times = await screen.findByTestId('acceptance-times');
    expect(times.textContent).toContain('未提供');
    expect(times.textContent).not.toContain('1793518656287');
  });

  it('shows a version conflict instead of a mismatched completion endpoint', async () => {
    const { payloads } = setupAcceptance();
    payloads.completion.selected_completion_id = 'OTHER';
    await renderAcceptance('/projects/demo?tab=acceptance&work=issue-filter-api');
    expect(await screen.findByTestId('acceptance-completion-conflict')).toBeTruthy();
    expect(screen.queryByTestId('acceptance-completion')).toBeNull();
  });

  it('shows completion review identifiers and independence from the verbatim approved_by object', async () => {

    const { payloads } = setupAcceptance();
    installLiveCompletion(payloads);
    await renderAcceptance('/projects/demo?tab=acceptance&work=issue-filter-api');
    expect(screen.getByTestId('completion-independence').textContent).toBe('成员独立复核（模拟）');
    expect(document.body.textContent).not.toContain(ROUND);
    expect(document.body.textContent).not.toContain('01M6AZ4XY0MEJHBR91AHDR578A');
  });

  it('rejects completion when the snapshot selected id differs', async () => {
    await expectLiveCompletionConflict((payloads) => {
      payloads.snapshots[WORK].observation.runtime.selected_completion_id = 'OTHER';
    });
  });

  it('rejects completion when delivery selects another completion', async () => {
    await expectLiveCompletionConflict((payloads) => {
      payloads.deliveryByWork[WORK].delivery.awr_acceptance.selected_completion_id = 'OTHER';
    });
  });

  it('rejects completion when the contract hash differs', async () => {
    await expectLiveCompletionConflict((payloads) => {
      payloads.completion.completion.contract_hash = 'OTHER';
    });
  });

  it('rejects completion when the evidence id differs', async () => {
    await expectLiveCompletionConflict((payloads) => {
      payloads.completion.completion.evidence_id = 'OTHER';
    });
  });

  it('rejects completion when the execution id differs', async () => {
    await expectLiveCompletionConflict((payloads) => {
      payloads.completion.completion.execution_id = 'OTHER';
    });
  });

  it('rejects completion when its approving review round differs', async () => {
    await expectLiveCompletionConflict((payloads) => {
      (payloads.completion.completion.approved_by as unknown as Record<string, unknown>).review_round_id = 'OTHER';
    });
  });

  it('reads dispatch state from dispatch_receipt.data in the verbatim integration reply', async () => {

    const { payloads, calls } = setupAcceptance();
    Object.assign(payloads.integration, structuredClone(LIVE_INTEGRATION.data));
    await renderAcceptance('/projects/demo?tab=acceptance&work=issue-filter-api');
    expect(calls.some(call => call.query?.op === 'delivery.integration.inspect')).toBe(true);
    expect(screen.getByTestId('delivery-step-integration').textContent).toBeTruthy();
    expect(document.body.textContent).not.toContain('dispatch_receipt');
  });

  it('shows a rejected integration attempt as rejected with its intent state', async () => {

    const { payloads } = setupAcceptance();
    Object.assign(payloads.integration, structuredClone(REJECTED_INTEGRATION.data));
    payloads.snapshots[PAGE].observation.collaboration.integration = {
      id: REJECTED_INTEGRATION.data.integration_id, state: 'rejected', current_binding: true, target_changed: false, reviewed_execution_id: EXECUTION,
    };
    await renderAcceptance('/projects/demo?tab=acceptance&work=issue-filter-page');
    expect(screen.getByTestId('acceptance-integration-rejected').textContent).toContain('并入被拒');
    expect(document.body.textContent).not.toContain(REJECTED_INTEGRATION.data.integration_id);
  });

  describe('integration version binding', () => {
    it('shows the current confirmed integration and matching completion as delivered', async () => {
      setupAcceptance();
      await renderAcceptance('/projects/demo?tab=acceptance&work=issue-filter-api');
      const chain = screen.getByTestId('acceptance-evidence-chain');
      expect(chain.textContent).toContain('已并入主线');
      expect(chain.textContent).toContain('业务验收已完成');
      expect(screen.queryByTestId('acceptance-integration-history')).toBeNull();
    });

    it.each([
      { state: 'rejected', snapshotCurrent: false, inspectedCurrent: false, confirmationCurrent: true, historical: true, status: '还没准备' },
      { state: 'confirmed', snapshotCurrent: false, inspectedCurrent: false, confirmationCurrent: true, historical: true, status: '还没准备' },
      { state: 'dispatched', snapshotCurrent: false, inspectedCurrent: false, confirmationCurrent: false, historical: true, status: '结果未知' },
      { state: 'unknown', snapshotCurrent: false, inspectedCurrent: false, confirmationCurrent: false, historical: true, status: '结果未知' },
      { state: 'confirmed', snapshotCurrent: false, inspectedCurrent: true, confirmationCurrent: true, historical: false, status: '结果未知' },
      { state: 'rejected', snapshotCurrent: true, inspectedCurrent: false, confirmationCurrent: true, historical: false, status: '结果未知' },
      { state: 'confirmed', snapshotCurrent: true, inspectedCurrent: undefined, confirmationCurrent: true, historical: false, status: '结果未知' },
      { state: 'confirmed', snapshotCurrent: true, inspectedCurrent: true, confirmationCurrent: false, historical: false, status: '结果未知' },
    ])('keeps $state with snapshot=$snapshotCurrent and inspection=$inspectedCurrent out of the current verdict', async ({ state, snapshotCurrent, inspectedCurrent, confirmationCurrent, historical, status }) => {
      const { payloads } = setupAcceptance();
      Object.assign(payloads.snapshots[WORK].observation.collaboration.integration!, { state, current_binding: snapshotCurrent });
      Object.assign(payloads.integration, { state, current_binding: inspectedCurrent, confirmation_current: confirmationCurrent });
      await renderAcceptance('/projects/demo?tab=acceptance&work=issue-filter-api');
      const chain = screen.getByTestId('acceptance-evidence-chain');
      expect(chain.textContent).toContain(status);
      expect(screen.queryByTestId('acceptance-integration-rejected')).toBeNull();
      expect(Boolean(screen.queryByTestId('acceptance-integration-history'))).toBe(historical);
      // The original attempt remains inspectable even when it cannot decide the current version.
      expect(screen.getByTestId('delivery-step-integration').textContent).toBeTruthy();
      expect(chain.textContent).not.toContain('已完成');
    });

    it('treats a mismatched inspected request as unknown even when both replies claim current', async () => {
      const { payloads } = setupAcceptance();
      payloads.integration.integration_id = 'another-integration';
      await renderAcceptance('/projects/demo?tab=acceptance&work=issue-filter-api');
      expect(screen.getByTestId('acceptance-evidence-chain').textContent).toContain('结果未知');
      expect(screen.queryByTestId('acceptance-integration-history')).toBeNull();
    });
  });

  it('keeps a child read timeout local to its card', async () => {

    setupAcceptance({ failOps: ['evidence.inspect'] });
    await renderAcceptance('/projects/demo?tab=acceptance&work=issue-filter-api');
    expect(screen.getByTestId('acceptance-evidence-chain')).toBeTruthy();
    expect(screen.getByTestId('acceptance-evidence-error').textContent).toContain('这部分暂时读取不到');
    expect(document.body.textContent).not.toContain('ReadTimeout');
  });

  it('reads and verifies artifact bytes only after expansion, including segment mode', async () => {

    vi.stubGlobal('crypto', webcrypto);
    const { calls } = setupAcceptance();
    await renderAcceptance('/projects/demo?tab=acceptance&work=issue-filter-api');
    expect(calls.filter(call => ['artifact.content', 'artifact.segment'].includes(String(call.query?.op)))).toHaveLength(0);
    await userEvent.setup().click(screen.getByTestId(`artifact-expand-${ARTIFACT}`));
    await screen.findByTestId(`artifact-read-content-${ARTIFACT}`);
    expect(screen.getByTestId(`artifact-digest-${ARTIFACT}`).textContent).toBe('内容已核对');
    expect(calls.filter(call => call.query?.op === 'artifact.content')).toHaveLength(1);
    expect(screen.queryByText('分段读取')).toBeNull();
  });

  it('shows artifact text as unverified when Web Crypto is unavailable', async () => {
    vi.stubGlobal('crypto', {});
    setupAcceptance();
    await renderAcceptance('/projects/demo?tab=acceptance&work=issue-filter-api');
    const user = userEvent.setup();
    await user.click(await screen.findByTestId(`artifact-expand-${ARTIFACT}`));
    expect(await screen.findByTestId(`artifact-read-content-${ARTIFACT}`)).toBeTruthy();
    expect(screen.getByTestId(`artifact-digest-${ARTIFACT}`).textContent).toBe('尚未核对');
  });

  it('keeps operational artifact records in the backend and shows their product result', async () => {
    vi.stubGlobal('crypto', webcrypto);
    const { payloads } = setupAcceptance();
    const text = JSON.stringify({ codec: 'awr-operational-example-v1', request_id: 'synthetic-internal-request', execution_id: 'synthetic-internal-execution' });
    const digest = createHash('sha256').update(text).digest('hex');
    payloads.reviews[WORK].submission.artifacts[0].query.expected_sha256 = digest;
    const originalFetch = globalThis.fetch;
    vi.stubGlobal('fetch', async (input: RequestInfo | URL, init?: RequestInit) => {
      const request = init?.body ? JSON.parse(String(init.body)) : null;
      if (request?.query?.op === 'artifact.content') {
        return json({ ok: true, result: envelope({ work_id: WORK, artifact_id: ARTIFACT, byte_length: Buffer.byteLength(text), sha256: digest, text }, WORK) });
      }
      return originalFetch(input, init);
    });
    await renderAcceptance('/projects/demo?tab=acceptance&work=issue-filter-api');
    await userEvent.setup().click(screen.getByTestId(`artifact-expand-${ARTIFACT}`));
    expect(await screen.findByText('检查记录已保存，验证结果可在成果进展中查看')).toBeTruthy();
    expect(document.body.textContent).not.toContain('synthetic-internal-request');
    expect(document.body.textContent).not.toContain('execution_id');
    expect(screen.getByTestId(`artifact-digest-${ARTIFACT}`).textContent).toBe('内容已核对');
  });

  it('automatically reads bounded artifact pages when the complete read is unavailable', async () => {
    vi.stubGlobal('crypto', webcrypto);
    const { calls } = setupAcceptance({ failOps: ['artifact.content'], failWith: { 'artifact.content': 'Unavailable' } });
    await renderAcceptance('/projects/demo?tab=acceptance&work=issue-filter-api');
    await userEvent.setup().click(screen.getByTestId(`artifact-expand-${ARTIFACT}`));
    await screen.findByTestId(`artifact-read-content-${ARTIFACT}`);
    expect(calls.filter(call => call.query?.op === 'artifact.content')).toHaveLength(1);
    expect(calls.filter(call => call.query?.op === 'artifact.segment')).toHaveLength(1);
    expect(screen.getByTestId(`artifact-digest-${ARTIFACT}`).textContent).toBe('内容已核对');
    expect(screen.queryByText('分段读取')).toBeNull();
  });

  it('reads immutable artifact bytes only once across multiple poll intervals', async () => {
    vi.stubGlobal('crypto', webcrypto);
    const { calls } = setupAcceptance();
    await renderAcceptance('/projects/demo?tab=acceptance&work=issue-filter-api');
    const user = userEvent.setup();
    await user.click(await screen.findByTestId(`artifact-expand-${ARTIFACT}`));
    await screen.findByTestId(`artifact-read-content-${ARTIFACT}`);
    expect(calls.filter((call) => call.query?.op === 'artifact.content')).toHaveLength(1);
    await new Promise((resolve) => setTimeout(resolve, 11000));
    expect(calls.filter((call) => call.query?.op === 'artifact.content')).toHaveLength(1);
  }, 15000);

  it('does not retry an Unavailable artifact read or poll it later', async () => {

    const { calls, payloads } = setupAcceptance({ failOps: ['artifact.content'], failWith: { 'artifact.content': 'Unavailable' } });
    payloads.reviews[WORK].submission.artifacts[0].segment_query = undefined as never;
    await renderAcceptance('/projects/demo?tab=acceptance&work=issue-filter-api');
    await userEvent.setup().click(screen.getByTestId(`artifact-expand-${ARTIFACT}`));
    expect(await screen.findByText(/成果暂时无法读取，请稍后重试/)).toBeTruthy();
    expect(document.body.textContent).not.toContain('Unavailable');
    expect(calls.filter(call => call.query?.op === 'artifact.content')).toHaveLength(1);
    await new Promise(resolve => setTimeout(resolve, 11000));
    expect(calls.filter(call => call.query?.op === 'artifact.content')).toHaveLength(1);
  }, 15000);

  it('shows forbidden for an overview auth error', async () => {
    const fetchMock = vi.fn(async (input: RequestInfo | URL) => {
      if (String(input).includes('/api/team/projects')) return json(DEMO_PROJECTS);
      return json({ ok: false, error: { code: 'Forbidden', message: 'denied' } });
    });
    vi.stubGlobal('fetch', fetchMock);
    await renderAcceptance('/projects/demo?tab=acceptance', { expand: false });
    expect(await screen.findByTestId('state-forbidden')).toBeTruthy();
  });

  it('clears restricted evidence and silences reads after revocation', async () => {
    const { calls } = setupAcceptance();
    const { queryClient } = await renderAcceptance('/projects/demo?tab=acceptance&work=issue-filter-api', { expand: false });
    await waitFor(() => expect(calls.some((call) => call.query?.op === 'completion.inspect')).toBe(true));
    act(() => revokeObservationReads(queryClient));
    expect(await screen.findByTestId('state-forbidden')).toBeTruthy();
    expect(screen.queryByText(COMPLETION)).toBeNull();
    const settled = calls.length;
    await new Promise((resolve) => setTimeout(resolve, 6500));
    expect(calls.length).toBe(settled);
  }, 10000);
});
