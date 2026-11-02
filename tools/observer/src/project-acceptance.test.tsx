import { createHash, webcrypto } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { act, screen } from '@testing-library/react';
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
const LIVE_COMPLETION = JSON.parse(readFileSync(resolve('../inspector/test/fixtures/obs006/completion-inspect.json'), 'utf8'));
const LIVE_INTEGRATION = JSON.parse(readFileSync(resolve('../inspector/test/fixtures/obs006/delivery-integration-inspect.json'), 'utf8'));
const REJECTED_INTEGRATION = JSON.parse(readFileSync(resolve('../inspector/test/fixtures/obs006/delivery-integration-inspect-page-rejected.json'), 'utf8'));

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
  const payloads = { snapshots, reviews, completion, deliveryByWork, integration };
  const fetchMock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
    const url = String(input);
    let query: Record<string, unknown> | undefined;
    if (url.includes('/api/team/read')) query = JSON.parse(String(init?.body)).query;
    calls.push({ url, query });
    if (url.includes('/api/team/projects')) return json(DEMO_PROJECTS);
    if (url.includes('/api/team/overview')) {
      return json({ ...DEMO_OVERVIEW, works: WORKS, workstreams: [{ id: 'WS1', external_key: 'support', title: 'Support' }] });
    }
    if (query) {
      if (failOps.includes(String(query.op))) return json({ ok: false, error: { code: failWith[String(query.op)] ?? 'ReadTimeout', message: 'slow' } });
      let data: unknown;
      switch (query.op) {
        case 'work.snapshot': data = snapshots[query.work_id as keyof typeof snapshots]; break;
        case 'review.inspect': data = { review: reviews[query.work_id as keyof typeof reviews] }; break;
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

type AcceptancePayloads = ReturnType<typeof setupAcceptance>['payloads'];

function installLiveCompletion(payloads: AcceptancePayloads) {
  Object.assign(payloads.completion, structuredClone(LIVE_COMPLETION.data));
}

async function expectLiveCompletionConflict(mutate: (payloads: AcceptancePayloads) => void) {
  const { payloads } = setupAcceptance();
  installLiveCompletion(payloads);
  mutate(payloads);
  renderApp('/projects/demo?tab=acceptance&work=issue-filter-api');
  expect(await screen.findByTestId('acceptance-completion-conflict')).toBeTruthy();
  expect(screen.queryByTestId('acceptance-completion')).toBeNull();
  expect(screen.getByTestId('acceptance-delivery-complete').textContent).not.toBe('true');
}

afterEach(() => vi.unstubAllGlobals());

describe('project acceptance evidence (AWR-OBS-006)', () => {
  it('localizes review states in the outcome source list (review r1.4)', async () => {
    setupAcceptance();
    renderApp('/projects/demo?tab=acceptance&work=issue-filter-api');
    expect((await screen.findByTestId('acceptance-work-issue-filter-api')).textContent).toContain('已批准');
    expect(screen.getByTestId('acceptance-work-issue-filter-page').textContent).toContain('进行中');
  });

  it('uses a missing-receipt label instead of a runtime state in the completion step (review r1.6)', async () => {
    setupAcceptance();
    renderApp('/projects/demo?tab=acceptance&work=issue-filter-page');
    const chain = await screen.findByTestId('acceptance-evidence-chain');
    expect(chain.textContent).toContain('无完成回执');
    expect(chain.textContent).not.toContain('已认领');
    await i18n.changeLanguage('en-US');
    expect(chain.textContent).toContain('No completion receipt');
    await i18n.changeLanguage('zh-CN');
  });

  it('loads exact evidence, execution, claim, completion, integration and source facts', async () => {
    setupAcceptance();
    renderApp('/projects/demo?tab=acceptance&work=issue-filter-api');
    expect(await screen.findByTestId('acceptance-evidence')).toBeTruthy();
    expect(screen.getByTestId('acceptance-evidence').textContent).toContain(EVIDENCE);
    expect(screen.getByTestId('acceptance-execution').textContent).toContain(EXECUTION);
    expect((await screen.findByTestId('acceptance-claim')).textContent).toContain(CLAIM);
    expect(screen.getByTestId('acceptance-completion').textContent).toContain(COMPLETION);
    expect(screen.getByTestId('acceptance-integration').textContent).toContain(INTEGRATION);
    expect(screen.getByTestId('acceptance-source').textContent).toContain('sha256:confirmed');
    expect(screen.getByTestId('acceptance-version').textContent).toContain(CANDIDATE);
    expect(screen.getByTestId('acceptance-version').textContent).toContain(CONTRACT);
  });

  it('uses exact selectors per object and never eagerly reads artifact bytes', async () => {
    const { calls } = setupAcceptance();
    renderApp('/projects/demo?tab=acceptance&work=issue-filter-api');
    await screen.findByTestId('acceptance-completion');
    await screen.findByTestId('acceptance-claim');
    const byOp = (op: string) => calls.filter((call) => call.query?.op === op).map((call) => call.query);
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
    renderApp('/projects/demo?tab=acceptance&work=issue-filter-api');
    await screen.findByTestId('acceptance-completion');
    const user = userEvent.setup();
    await user.click(screen.getByTestId(`acceptance-work-${PAGE}`));
    expect(await screen.findByTestId(`acceptance-review-${PAGE_ROUND}`)).toBeTruthy();
    expect(screen.queryByTestId('acceptance-completion')).toBeNull();
    expect(screen.getByTestId('acceptance-page').textContent).not.toContain(COMPLETION);
  });

  it('keeps an old candidate approval historical rather than current', async () => {
    const { payloads } = setupAcceptance();
    payloads.reviews[WORK].submission.candidate_current = false;
    renderApp('/projects/demo?tab=acceptance&work=issue-filter-api');
    expect(await screen.findByTestId('acceptance-review-history')).toBeTruthy();
    expect(screen.queryByTestId('acceptance-current-approval')).toBeNull();
  });

  it('labels findings with their original work, round and criterion', async () => {
    setupAcceptance();
    renderApp('/projects/demo?tab=acceptance&work=issue-filter-api');
    const label = await screen.findByTestId('finding-original-finding-1');
    expect(label.textContent).toContain(WORK);
    expect(label.textContent).toContain('1');
    expect(label.textContent).toContain('Combine filters');
  });

  it('shows the decision human_approval value instead of hiding it', async () => {
    setupAcceptance();
    renderApp('/projects/demo?tab=acceptance&work=issue-filter-api');
    expect((await screen.findByTestId('decision-human-approval')).textContent).toBe('否');
  });

  it('requires candidate_current true for current approval and reports the fact as not provided', async () => {
    const { payloads } = setupAcceptance();
    payloads.reviews[WORK].submission.candidate_current = undefined as never;
    renderApp('/projects/demo?tab=acceptance&work=issue-filter-api');
    expect(screen.queryByTestId('acceptance-current-approval')).toBeNull();
    expect((await screen.findByTestId('acceptance-review-currentness')).textContent).toBe('未提供');
  });

  it('disables segment mode when the review has no segment_query', async () => {
    const { payloads } = setupAcceptance();
    payloads.reviews[WORK].submission.artifacts[0].segment_query = undefined as never;
    renderApp('/projects/demo?tab=acceptance&work=issue-filter-api');
    const user = userEvent.setup();
    await user.click(await screen.findByTestId(`artifact-expand-${ARTIFACT}`));
    const option = await screen.findByText('分段读取');
    expect(option.closest('.ant-segmented-item')?.classList.contains('ant-segmented-item-disabled')).toBe(true);
  });

  it('shows self-report and non-independence without inventing automatic sync', async () => {
    setupAcceptance();
    renderApp('/projects/demo?tab=acceptance&work=issue-filter-api');
    expect((await screen.findAllByText(/作者自报/)).length).toBeGreaterThan(0);
    expect((await screen.findAllByText(/非独立验证/)).length).toBeGreaterThan(0);
    expect(screen.queryByText(/后台自动同步/)).toBeNull();
    expect(screen.getByTestId('acceptance-times')).toBeTruthy();
  });

  it('formats all four clocks with the shared formatter and keeps raw values in tooltips', async () => {
    setupAcceptance();
    renderApp('/projects/demo?tab=acceptance&work=issue-filter-api');
    const observed = 1793518656287;
    const recorded = 1793518656318;
    const zhObserved = formatUnixMs(observed, 'zh-CN');
    const enObserved = formatUnixMs(observed, 'en-US');
    await screen.findByTestId('acceptance-integration');
    const times = await screen.findByTestId('acceptance-times');
    expect(times.textContent).toContain(zhObserved);
    expect(times.textContent).not.toContain(String(observed));
    const user = userEvent.setup();
    await user.hover(screen.getAllByText(zhObserved)[0]);
    expect(await screen.findByText(String(observed))).toBeTruthy();
    expect(times.textContent).toContain(formatUnixMs(recorded, 'zh-CN'));
    try {
      await i18n.changeLanguage('en-US');
      expect((await screen.findAllByText(enObserved)).length).toBeGreaterThan(0);
      expect(screen.getByTestId('acceptance-times').textContent).toContain(formatUnixMs(recorded, 'en-US'));
    } finally {
      await i18n.changeLanguage('zh-CN');
    }
  });

  it('marks missing clock facts as not provided instead of borrowing another clock', async () => {
    const { payloads } = setupAcceptance();
    const confirmation = payloads.integration.confirmation as { envelope: { record: { data: { provenance: Record<string, unknown> } } } };
    delete confirmation.envelope.record.data.provenance.observed_at_unix_ms;
    payloads.snapshots[WORK].observation.collaboration.source_state = undefined as never;
    renderApp('/projects/demo?tab=acceptance&work=issue-filter-api');
    const times = await screen.findByTestId('acceptance-times');
    expect(times.textContent).toContain('未提供');
    expect(times.textContent).not.toContain('1793518656287');
  });

  it('shows a version conflict instead of a mismatched completion endpoint', async () => {
    const { payloads } = setupAcceptance();
    payloads.completion.selected_completion_id = 'OTHER';
    renderApp('/projects/demo?tab=acceptance&work=issue-filter-api');
    expect(await screen.findByTestId('acceptance-completion-conflict')).toBeTruthy();
    expect(screen.queryByTestId('acceptance-completion')).toBeNull();
  });

  it('shows completion review identifiers and independence from the verbatim approved_by object', async () => {
    const { payloads } = setupAcceptance();
    installLiveCompletion(payloads);
    renderApp('/projects/demo?tab=acceptance&work=issue-filter-api');
    expect((await screen.findByTestId('completion-review-round')).textContent).toBe(ROUND);
    expect(screen.getByTestId('completion-review-decision').textContent).toBe('01M6AZ4XY0MEJHBR91AHDR578A');
    expect(screen.getByTestId('completion-independence').textContent).toBe('成员独立复核（模拟）');
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
    const { payloads } = setupAcceptance();
    Object.assign(payloads.integration, structuredClone(LIVE_INTEGRATION.data));
    renderApp('/projects/demo?tab=acceptance&work=issue-filter-api');
    expect((await screen.findByTestId('acceptance-dispatch-state')).textContent).toBe('已分派');
  });

  it('shows a rejected integration attempt as rejected with its intent state', async () => {
    const { payloads } = setupAcceptance();
    Object.assign(payloads.integration, structuredClone(REJECTED_INTEGRATION.data));
    payloads.snapshots[PAGE].observation.collaboration.integration = {
      id: REJECTED_INTEGRATION.data.integration_id, state: 'rejected', current_binding: true, target_changed: false, reviewed_execution_id: EXECUTION,
    };
    renderApp('/projects/demo?tab=acceptance&work=issue-filter-page');
    expect(await screen.findByTestId('acceptance-integration-rejected')).toBeTruthy();
    expect(screen.getByTestId('acceptance-integration-state').textContent).toBe('已拒绝');
    expect(screen.getByTestId('acceptance-integration-outcome').textContent).toBe('已拒绝');
    expect(screen.queryByText(/尚未交付/)).toBeNull();
  });

  it('keeps a child read timeout local to its card', async () => {
    setupAcceptance({ failOps: ['evidence.inspect'] });
    renderApp('/projects/demo?tab=acceptance&work=issue-filter-api');
    expect(await screen.findByTestId('acceptance-completion')).toBeTruthy();
    expect((await screen.findByTestId('acceptance-evidence-error', undefined, { timeout: 5000 })).textContent).toContain('ReadTimeout');
  });

  it('reads and verifies artifact bytes only after expansion, including segment mode', async () => {
    vi.stubGlobal('crypto', webcrypto);
    const { calls } = setupAcceptance();
    renderApp('/projects/demo?tab=acceptance&work=issue-filter-api');
    await screen.findByTestId(`acceptance-artifact-${ARTIFACT}`);
    expect(calls.filter((call) => ['artifact.content', 'artifact.segment'].includes(String(call.query?.op)))).toHaveLength(0);
    const user = userEvent.setup();
    await user.click(screen.getByTestId(`artifact-expand-${ARTIFACT}`));
    expect(await screen.findByTestId(`artifact-read-content-${ARTIFACT}`)).toBeTruthy();
    expect(screen.getByTestId(`artifact-digest-${ARTIFACT}`).textContent).toBe('摘要已校验');
    await user.click(screen.getByText('分段读取'));
    expect(await screen.findByTestId(`artifact-read-segment-${ARTIFACT}`)).toBeTruthy();
    expect(calls.filter((call) => call.query?.op === 'artifact.content')).toHaveLength(1);
    expect(calls.filter((call) => call.query?.op === 'artifact.segment')).toHaveLength(1);
  });

  it('shows artifact text as unverified when Web Crypto is unavailable', async () => {
    vi.stubGlobal('crypto', {});
    setupAcceptance();
    renderApp('/projects/demo?tab=acceptance&work=issue-filter-api');
    const user = userEvent.setup();
    await user.click(await screen.findByTestId(`artifact-expand-${ARTIFACT}`));
    expect(await screen.findByTestId(`artifact-read-content-${ARTIFACT}`)).toBeTruthy();
    expect(screen.getByTestId(`artifact-digest-${ARTIFACT}`).textContent).toBe('未校验');
  });

  it('reads immutable artifact bytes only once across multiple poll intervals', async () => {
    vi.stubGlobal('crypto', webcrypto);
    const { calls } = setupAcceptance();
    renderApp('/projects/demo?tab=acceptance&work=issue-filter-api');
    const user = userEvent.setup();
    await user.click(await screen.findByTestId(`artifact-expand-${ARTIFACT}`));
    await screen.findByTestId(`artifact-read-content-${ARTIFACT}`);
    expect(calls.filter((call) => call.query?.op === 'artifact.content')).toHaveLength(1);
    await new Promise((resolve) => setTimeout(resolve, 11000));
    expect(calls.filter((call) => call.query?.op === 'artifact.content')).toHaveLength(1);
  }, 15000);

  it('does not retry an Unavailable artifact read or poll it later', async () => {
    const { calls } = setupAcceptance({ failOps: ['artifact.content'], failWith: { 'artifact.content': 'Unavailable' } });
    renderApp('/projects/demo?tab=acceptance&work=issue-filter-api');
    const user = userEvent.setup();
    await user.click(await screen.findByTestId(`artifact-expand-${ARTIFACT}`));
    expect(await screen.findByText(/正文读取失败：Unavailable/, undefined, { timeout: 5000 })).toBeTruthy();
    expect(calls.filter((call) => call.query?.op === 'artifact.content')).toHaveLength(1);
    await new Promise((resolve) => setTimeout(resolve, 11000));
    expect(calls.filter((call) => call.query?.op === 'artifact.content')).toHaveLength(1);
  }, 15000);

  it('shows forbidden for an overview auth error', async () => {
    const fetchMock = vi.fn(async (input: RequestInfo | URL) => {
      if (String(input).includes('/api/team/projects')) return json(DEMO_PROJECTS);
      return json({ ok: false, error: { code: 'Forbidden', message: 'denied' } });
    });
    vi.stubGlobal('fetch', fetchMock);
    renderApp('/projects/demo?tab=acceptance');
    expect(await screen.findByTestId('state-forbidden')).toBeTruthy();
  });

  it('clears restricted evidence and silences reads after revocation', async () => {
    const { calls } = setupAcceptance();
    const { queryClient } = renderApp('/projects/demo?tab=acceptance&work=issue-filter-api');
    await screen.findByTestId('acceptance-completion');
    act(() => revokeObservationReads(queryClient));
    expect(await screen.findByTestId('state-forbidden')).toBeTruthy();
    expect(screen.queryByText(COMPLETION)).toBeNull();
    const settled = calls.length;
    await new Promise((resolve) => setTimeout(resolve, 1200));
    expect(calls.length).toBe(settled);
  });
});
