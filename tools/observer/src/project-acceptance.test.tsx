import { act, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { revokeObservationReads } from './lib/queries';
import { DEMO_OVERVIEW, DEMO_PROJECTS, renderApp } from './test-utils';

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
const DIGEST = '37f728ac073d23df94311a460b8c403a6a81be9b8c36b7479a2011a2869f9a87';

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

function setupAcceptance({ failOps = [] }: { failOps?: string[] } = {}) {
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
  const payloads = { snapshots, reviews, completion };
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
      if (failOps.includes(String(query.op))) return json({ ok: false, error: { code: 'ReadTimeout', message: 'slow' } });
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
        case 'delivery.inspect': {
          const delivered = query.work_id === WORK;
          data = { delivery: { work_id: query.work_id, awr_acceptance: { complete: delivered, runtime_state: delivered ? 'completed' : 'claimed', selected_completion_id: delivered ? COMPLETION : null }, cannot_skip_acceptance_via: ['green_ci', 'already_merged'], github: { approved: false, merged: false, submitted: false }, pr: null, webhook_auto_sync: false } };
          break;
        }
        case 'delivery.neutral.inspect':
          data = { work_id: WORK, connectors: [{ connector_id: 'repository-api', connector_version: '1', current_epoch: true, enabled: true, fact_source: 'adapter_observation', provider: 'local_git', resource: 'local-repository:synthetic' }], connectors_truncated: false, selected_current: true, selection_version: '1' };
          break;
        case 'delivery.submission.describe':
          data = { work_id: WORK, read_only: true, stored_selection: { binding_digest: CANDIDATE, current: true, selection_version: '1' }, connectors: [{ provider: 'local_git', enabled: true, current_epoch: true }], local_git: { basis: 'local_git.manifest verifies committed bytes' } };
          break;
        case 'delivery.integration.inspect':
          data = {
            integration_id: INTEGRATION,
            integration_request: { binding: { target: { reference: 'refs/heads/main', resource: 'local-repository:synthetic', precondition: { kind: 'exact', revision: { value: 'a516342436e434d0072ef9c138f419f2f128caac' } } }, source_revision: { value: '5f7eb2fe9c563e4d355aa64a527e0aef754116c1' } } },
            dispatch_receipt: { state: 'dispatched', lease_id: 'lease-1', recorded_at_unix_ms: 1793518656318 },
            confirmation: { envelope: { record: { data: { outcome: 'applied', result_revision: { value: '5f7eb2fe9c563e4d355aa64a527e0aef754116c1' }, provenance: { observed_at_unix_ms: 1793518656287, recorded_at_unix_ms: 1793518656318, source: 'adapter_observation' } } } } },
            current_binding: true, confirmation_current: true, target_changed: false, request_binding: 'original_candidate_and_read_set', dispatched_at: '2026-11-01T07:37:35Z',
          };
          break;
        case 'delivery.source.status':
          data = { work_id: WORK, source_observation: 'observed', confirmed_fingerprint: 'sha256:confirmed', source_fingerprint: 'sha256:confirmed', source_synchronized: true, cursor_current: true, projection_current: true, pending_publication_id: null, metadata_revision: '5', history: [{ phase: 'confirmed', source_synchronized_at_commit: true, source_written_observed: true, confirmation: { operation_effects: { source_metadata_written: true, work_finalized: false }, effect_basis: 'this_publication_operation; not current_work_state' } }], history_truncated: false };
          break;
        case 'delivery.exports': data = { adoption_available: true, adoption_version_policies: ['current_contract', 'fixed_delivery'], items: [], next_cursor: null, upstream_source_access: false }; break;
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

afterEach(() => vi.unstubAllGlobals());

describe('project acceptance evidence (AWR-OBS-006)', () => {
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

  it('shows self-report and non-independence without inventing automatic sync', async () => {
    setupAcceptance();
    renderApp('/projects/demo?tab=acceptance&work=issue-filter-api');
    expect((await screen.findAllByText(/作者自报/)).length).toBeGreaterThan(0);
    expect((await screen.findAllByText(/非独立验证/)).length).toBeGreaterThan(0);
    expect(screen.queryByText(/后台自动同步/)).toBeNull();
    expect(screen.getByTestId('acceptance-times').textContent).toContain('1793518656287');
    expect(screen.getByTestId('acceptance-times').textContent).toContain('1793518656318');
  });

  it('shows a version conflict instead of a mismatched completion endpoint', async () => {
    const { payloads } = setupAcceptance();
    payloads.completion.selected_completion_id = 'OTHER';
    renderApp('/projects/demo?tab=acceptance&work=issue-filter-api');
    expect(await screen.findByTestId('acceptance-completion-conflict')).toBeTruthy();
    expect(screen.queryByTestId('acceptance-completion')).toBeNull();
  });

  it('keeps a child read timeout local to its card', async () => {
    setupAcceptance({ failOps: ['evidence.inspect'] });
    renderApp('/projects/demo?tab=acceptance&work=issue-filter-api');
    expect(await screen.findByTestId('acceptance-completion')).toBeTruthy();
    expect((await screen.findByTestId('acceptance-evidence-error', undefined, { timeout: 5000 })).textContent).toContain('ReadTimeout');
  });

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
