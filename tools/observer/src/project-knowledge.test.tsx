import { screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import { DEMO_OVERVIEW, DEMO_PROJECTS, renderApp } from './test-utils';

// Fixtures are shaped after the real samples: service-work-prepare-*.json
// (authorized_readable_refs / required_specs with embedded text) and
// service-source-content-support-md.json (path + sha256 provenance).
const SPEC = {
  path: 'docs/support.md',
  sha256: 'c101fd5c70986de856c0bd30d2da271b71074408acbec05fabc9fa095e1b80ac',
  byte_length: 433,
  content_included: true,
  next_step: null,
  text: '# Customer support queue\n\nBuild a useful filterable queue for support staff.',
};
const REF = {
  kind: 'source',
  path: 'docs/support.md',
  sha256: SPEC.sha256,
  byte_length: 433,
};

function snapshotFor(workId: string, overrides: Record<string, unknown> = {}) {
  return {
    ok: true,
    result: {
      protocol_version: 1,
      scope_id: 'main',
      source_snapshot_id: 'snap-1',
      coordinator_epoch: 'epoch-1',
      project_revision: '42',
      data: {
        work_id: workId,
        required_specs: [{ ...SPEC }],
        authorized_readable_refs: [{ ...REF }],
        visible_contract: { goals: ['Filter issues by label'], acceptance: ['Labels combine correctly'] },
        observation: { reporting: { client_reported_at_unix_ms: 1761839800000 }, progress: null, runtime: { state: 'claimed' } },
        guidance: { action: { op: 'session.checkpoint', note: 'Record the verified outcome.' } },
        execution_authorized: false,
        ...overrides,
      },
    },
    query: { protocol_version: 1, op: 'work.snapshot', work_id: workId },
    freshness: {
      report_received: { status: 'not_measured' },
      authoritative: { status: 'not_measured' },
      page_read: { status: 'measured', at_unix_ms: 1761840000000 },
    },
  };
}

const WORKS = [
  { key: 'W1', title: 'issue-filter-api', status: 'unknown', owner_person: null, agent: null, next_step: null, detail_loaded: false, workstream_id: 'WS1', contract_hash: 'c1' },
  { key: 'W2', title: 'issue-filter-page', status: 'unknown', owner_person: null, agent: null, next_step: null, detail_loaded: false, workstream_id: 'WS1', contract_hash: 'c2' },
];

function setupKnowledge({ snapshots }: { snapshots: Record<string, unknown> }) {
  const calls: Array<{ url: string; init?: RequestInit }> = [];
  const fetchMock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
    const url = String(input);
    calls.push({ url, init });
    if (url.includes('/api/team/projects')) return json(DEMO_PROJECTS);
    if (url.includes('/api/team/overview')) {
      return json({
        ...DEMO_OVERVIEW,
        works: WORKS,
        workstreams: [{ id: 'WS1', external_key: 'support', title: 'Customer support queue' }],
      });
    }
    if (url.includes('/api/team/read')) {
      const query = JSON.parse(String(init?.body)).query;
      if (query.op === 'work.snapshot') {
        const response = snapshots[query.work_id];
        if (response) return json(response);
      }
    }
    return json({ ok: false, error: { code: 'NoRoute', message: url } });
  });
  vi.stubGlobal('fetch', fetchMock);
  return { calls, fetchMock };
}

function json(payload: unknown) {
  return new Response(JSON.stringify(payload), {
    status: 200,
    headers: { 'content-type': 'application/json' },
  });
}

describe('project knowledge tab (AWR-OBS-009)', () => {
  it('assembles one deduplicated source entry per (path, sha256) with every citing work linked', async () => {
    setupKnowledge({ snapshots: { W1: snapshotFor('W1'), W2: snapshotFor('W2') } });
    renderApp('/projects/demo?tab=knowledge');
    // Two works cite the same file: one entry, both works listed under it.
    const items = await screen.findAllByTestId('knowledge-source-docs/support.md');
    expect(items.length).toBe(1);
    expect(screen.getByTestId('knowledge-citing-works').textContent).toContain('issue-filter-api');
    expect(screen.getByTestId('knowledge-citing-works').textContent).toContain('issue-filter-page');
    expect(screen.getByTestId('knowledge-from-works').textContent).toContain('2');
  });

  it('renders the embedded required_specs text with provenance, not a second fact store', async () => {
    setupKnowledge({ snapshots: { W1: snapshotFor('W1'), W2: snapshotFor('W2') } });
    renderApp('/projects/demo?tab=knowledge');
    expect(await screen.findByText(/Build a useful filterable queue/)).toBeTruthy();
    expect(screen.getByText(/c101fd5c/)).toBeTruthy();
  });

  it('shows only metadata when content_included is false', async () => {
    const withoutText = snapshotFor('W1', {
      required_specs: [{ ...SPEC, content_included: false }],
    });
    setupKnowledge({ snapshots: { W1: withoutText, W2: withoutText } });
    renderApp('/projects/demo?tab=knowledge');
    await screen.findByTestId('knowledge-source-docs/support.md');
    expect(screen.queryByText(/Build a useful filterable queue/)).toBeNull();
    expect((await screen.findAllByText(/正文未随响应提供/)).length).toBeGreaterThan(0);
  });

  it('marks a text whose length differs from byte_length as partial, not the whole document', async () => {
    const truncated = snapshotFor('W1', {
      required_specs: [{ ...SPEC, byte_length: 4330 }],
    });
    setupKnowledge({ snapshots: { W1: truncated, W2: snapshotFor('W2') } });
    renderApp('/projects/demo?tab=knowledge');
    expect(await screen.findByText(/部分/)).toBeTruthy();
  });

  it('discloses a failed work read instead of silently dropping it', async () => {
    setupKnowledge({
      snapshots: {
        W1: snapshotFor('W1'),
        W2: { ok: false, error: { code: 'ReadTimeout', message: 'slow' } },
      },
    });
    renderApp('/projects/demo?tab=knowledge');
    expect(await screen.findByText(/读取失败/, undefined, { timeout: 5000 })).toBeTruthy();
    expect(screen.getByTestId('knowledge-from-works').textContent).toBe('来自 1／2 个工作项');
  });

  it('shows source changed only when the returned digest differs from the reference', async () => {
    const changed = snapshotFor('W1', {
      required_specs: [{ ...SPEC, sha256: 'deadbeef00000000000000000000000000000000000000000000000000000000' }],
    });
    setupKnowledge({ snapshots: { W1: changed, W2: snapshotFor('W2') } });
    renderApp('/projects/demo?tab=knowledge');
    expect(await screen.findByText(/来源已变化/, undefined, { timeout: 5000 })).toBeTruthy();
  });

  it('treats an Unavailable reply as a failed read, not a confirmed source change', async () => {
    setupKnowledge({
      snapshots: {
        W1: { ok: false, error: { code: 'Unavailable', message: 'request outcome unavailable; inspect a command before retrying with its original identity' } },
        W2: snapshotFor('W2'),
      },
    });
    renderApp('/projects/demo?tab=knowledge');
    expect(await screen.findByText(/读取失败/, undefined, { timeout: 5000 })).toBeTruthy();
    expect(screen.queryByText(/来源已变化/)).toBeNull();
  });

  it('the search box filters only what is loaded and says so', async () => {
    setupKnowledge({ snapshots: { W1: snapshotFor('W1'), W2: snapshotFor('W2') } });
    renderApp('/projects/demo?tab=knowledge');
    expect(await screen.findByPlaceholderText(/筛选当前页/)).toBeTruthy();
    const user = userEvent.setup();
    await user.type(screen.getByPlaceholderText(/筛选当前页/), 'no-such-file');
    expect(screen.queryByTestId('knowledge-source-docs/support.md')).toBeNull();
    await user.clear(screen.getByPlaceholderText(/筛选当前页/));
    expect(screen.getByTestId('knowledge-source-docs/support.md')).toBeTruthy();
  });

  it('shows the empty state when no readable reference exists', async () => {
    const empty = snapshotFor('W1', { required_specs: [], authorized_readable_refs: [] });
    setupKnowledge({ snapshots: { W1: empty, W2: empty } });
    renderApp('/projects/demo?tab=knowledge');
    expect(await screen.findByTestId('state-empty')).toBeTruthy();
  });

  it('shows the contract goals and acceptance of the citing work', async () => {
    setupKnowledge({ snapshots: { W1: snapshotFor('W1'), W2: snapshotFor('W2') } });
    renderApp('/projects/demo?tab=knowledge');
    expect(await screen.findByText('Filter issues by label')).toBeTruthy();
    expect(screen.getByText('Labels combine correctly')).toBeTruthy();
  });

  it('a revoked session shows the forbidden state and silences reads', async () => {
    const fetchMock = vi.fn(async () => json({ ok: false, error: { code: 'Unauthenticated', message: 'revoked' } }));
    vi.stubGlobal('fetch', fetchMock);
    renderApp('/projects/demo?tab=knowledge');
    expect(await screen.findByTestId('state-forbidden')).toBeTruthy();
    const settled = fetchMock.mock.calls.length;
    await new Promise((resolve) => setTimeout(resolve, 1200));
    expect(fetchMock.mock.calls.length).toBe(settled);
  });
});
