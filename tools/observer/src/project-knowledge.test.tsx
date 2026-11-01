import { createHash, webcrypto } from 'node:crypto';
import { QueryClientProvider } from '@tanstack/react-query';
import { act, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { MemoryRouter } from 'react-router';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { RevocationProvider } from './lib/revocation';
import { createAppQueryClient, queryKeys, revokeObservationReads } from './lib/queries';
import { ProjectKnowledge } from './pages/ProjectKnowledge';
import { DEMO_OVERVIEW, DEMO_PROJECTS, renderApp } from './test-utils';

// Fixtures follow the real response shapes in live-samples/
// lin-upstream-work-snapshot-api.json and service-work-prepare-*.json:
// required_specs carry embedded text, authorized_readable_refs carry provenance,
// and visible_contract belongs to the citing work (not to the source file).
interface SpecFixture {
  path: string;
  sha256: string;
  byte_length: number;
  content_included?: boolean;
  next_step: null;
  text: string;
}

function sha256(text: string) {
  return createHash('sha256').update(text, 'utf8').digest('hex');
}

function utf8Length(text: string) {
  return new TextEncoder().encode(text).length;
}

const SPEC_TEXT = '# Customer support queue\n\nBuild a useful filterable queue for support staff.';

function specFor(text = SPEC_TEXT, overrides: Partial<SpecFixture> = {}): SpecFixture {
  return {
    path: 'docs/support.md',
    sha256: sha256(text),
    byte_length: utf8Length(text),
    content_included: true,
    next_step: null,
    text,
    ...overrides,
  };
}

function refFor(spec: SpecFixture) {
  return {
    kind: 'source',
    path: spec.path,
    sha256: spec.sha256,
    byte_length: spec.byte_length,
  };
}

function sourceTestId(spec: SpecFixture) {
  return `knowledge-source-${spec.path}-${spec.sha256.slice(0, 12)}`;
}

function snapshotFor(workId: string, overrides: Record<string, unknown> = {}) {
  const spec = specFor();
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
        required_specs: [spec],
        authorized_readable_refs: [refFor(spec)],
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

function snapshotWithSpec(
  workId: string,
  spec: SpecFixture,
  contract?: { goals: string[]; acceptance: string[] },
) {
  return snapshotFor(workId, {
    required_specs: [spec],
    authorized_readable_refs: [refFor(spec)],
    ...(contract ? { visible_contract: contract } : {}),
  });
}

const WORKS = [
  { key: 'W1', title: 'issue-filter-api', status: 'unknown', owner_person: null, agent: null, next_step: null, detail_loaded: false, workstream_id: 'WS1', contract_hash: 'c1' },
  { key: 'W2', title: 'issue-filter-page', status: 'unknown', owner_person: null, agent: null, next_step: null, detail_loaded: false, workstream_id: 'WS1', contract_hash: 'c2' },
];

function setupKnowledge({
  snapshots,
  works = WORKS,
}: {
  snapshots: Record<string, unknown>;
  works?: typeof WORKS;
}) {
  const calls: Array<{ url: string; init?: RequestInit }> = [];
  const fetchMock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
    const url = String(input);
    calls.push({ url, init });
    if (url.includes('/api/team/projects')) return json(DEMO_PROJECTS);
    if (url.includes('/api/team/overview')) {
      return json({
        ...DEMO_OVERVIEW,
        works,
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

afterEach(() => {
  vi.unstubAllGlobals();
});

describe('project knowledge tab (AWR-OBS-009)', () => {
  it('assembles one entry per (path, sha256) with every citing work linked', async () => {
    const spec = specFor();
    setupKnowledge({ snapshots: { W1: snapshotFor('W1'), W2: snapshotFor('W2') } });
    renderApp('/projects/demo?tab=knowledge');
    expect((await screen.findAllByTestId(sourceTestId(spec))).length).toBe(1);
    expect(screen.getByTestId('knowledge-citing-works').textContent).toContain('issue-filter-api');
    expect(screen.getByTestId('knowledge-citing-works').textContent).toContain('issue-filter-page');
    expect(screen.getByTestId('knowledge-from-works').textContent).toBe('来自 2／2 个工作项');
  });

  it('keeps same-title works distinct by key and gives both encoded work links', async () => {
    const works = [
      { ...WORKS[0], key: 'W/1 ?', title: 'Support filter' },
      { ...WORKS[1], key: 'W/2 ?', title: 'Support filter' },
    ];
    setupKnowledge({
      works,
      snapshots: { 'W/1 ?': snapshotFor('W/1 ?'), 'W/2 ?': snapshotFor('W/2 ?') },
    });
    renderApp('/projects/demo?tab=knowledge');
    expect(await screen.findByTestId('knowledge-citing-works')).toHaveProperty('textContent', '引用它的工作项: Support filter, Support filter');
    const links = screen.getAllByRole('link', { name: '打开 Support filter' });
    expect(links.map((link) => link.getAttribute('href'))).toEqual([
      '/projects/demo?tab=plan&work=W%2F1%20%3F',
      '/projects/demo?tab=plan&work=W%2F2%20%3F',
    ]);
  });

  it('renders embedded required_specs text as pre-wrap with its provenance', async () => {
    setupKnowledge({ snapshots: { W1: snapshotFor('W1'), W2: snapshotFor('W2') } });
    renderApp('/projects/demo?tab=knowledge');
    const paragraph = await screen.findByText((_, element) => (
      element?.classList.contains('ant-typography') === true && element.textContent === SPEC_TEXT
    ));
    expect((paragraph as HTMLElement).style.whiteSpace).toBe('pre-wrap');
    expect(screen.getByText(/[a-f0-9]{12}…/)).toBeTruthy();
  });

  it('shows the full digest in a tooltip', async () => {
    const spec = specFor();
    setupKnowledge({ snapshots: { W1: snapshotFor('W1'), W2: snapshotFor('W2') } });
    renderApp('/projects/demo?tab=knowledge');
    await screen.findByTestId(sourceTestId(spec));
    const user = userEvent.setup();
    await user.hover(screen.getByText(`${spec.sha256.slice(0, 12)}…`));
    expect(await screen.findByText(spec.sha256)).toBeTruthy();
  });

  it('shows only metadata when content_included is false', async () => {
    const spec = specFor(SPEC_TEXT, { content_included: false });
    const snapshot = snapshotWithSpec('W1', spec);
    setupKnowledge({ snapshots: { W1: snapshot, W2: snapshot } });
    renderApp('/projects/demo?tab=knowledge');
    await screen.findByTestId(sourceTestId(spec));
    expect(screen.queryByText(/Build a useful filterable queue/)).toBeNull();
    expect((await screen.findAllByText(/正文未随响应提供/)).length).toBeGreaterThan(0);
  });

  it('treats a missing content_included flag as included when text is present', async () => {
    const spec = specFor(SPEC_TEXT, { content_included: undefined });
    const snapshot = snapshotWithSpec('W1', spec);
    setupKnowledge({ snapshots: { W1: snapshot, W2: snapshot } });
    renderApp('/projects/demo?tab=knowledge');
    expect(await screen.findByText(/Build a useful filterable queue/)).toBeTruthy();
  });

  it.each([
    ['Chinese', '# 客户支持队列\n\n按标签筛选。'],
    ['emoji', '# Support queue 😀😀\n'],
  ])('does not mark a complete %s document partial', async (_name, text) => {
    const spec = specFor(text);
    const snapshot = snapshotWithSpec('W1', spec);
    setupKnowledge({ snapshots: { W1: snapshot, W2: snapshot } });
    renderApp('/projects/demo?tab=knowledge');
    expect(await screen.findByText((_, element) => (
      element?.classList.contains('ant-typography') === true && element.textContent === text
    ))).toBeTruthy();
    expect(screen.queryByText(/部分（长度/)).toBeNull();
  });

  it('still marks truncated ASCII text partial using UTF-8 byte length', async () => {
    const spec = specFor('abcdef', { byte_length: 7 });
    const snapshot = snapshotWithSpec('W1', spec);
    setupKnowledge({ snapshots: { W1: snapshot, W2: snapshot } });
    renderApp('/projects/demo?tab=knowledge');
    expect(await screen.findByText(/部分（长度/)).toBeTruthy();
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

  it('lets both digests of one path be selected without claiming which is older', async () => {
    const first = specFor('FIRST DOCUMENT');
    const second = specFor('SECOND DOCUMENT');
    setupKnowledge({
      snapshots: { W1: snapshotWithSpec('W1', first), W2: snapshotWithSpec('W2', second) },
    });
    renderApp('/projects/demo?tab=knowledge');
    const firstRow = await screen.findByTestId(sourceTestId(first));
    const secondRow = screen.getByTestId(sourceTestId(second));
    expect(firstRow).not.toBe(secondRow);
    expect(await screen.findByText('FIRST DOCUMENT')).toBeTruthy();
    expect(screen.getByTestId('path-digest-notice').textContent).toContain('2 个摘要');
    expect(screen.getByTestId('path-digest-notice').textContent).not.toContain('旧文本');

    const user = userEvent.setup();
    await user.click(secondRow);
    expect(await screen.findByText('SECOND DOCUMENT')).toBeTruthy();
    expect(screen.queryByText('FIRST DOCUMENT')).toBeNull();
    expect(screen.getByTestId('knowledge-citing-works').textContent).toContain('issue-filter-page');
    expect(screen.getByTestId('knowledge-citing-works').textContent).not.toContain('issue-filter-api');
    await user.click(firstRow);
    expect(await screen.findByText('FIRST DOCUMENT')).toBeTruthy();
  });

  it('treats Unavailable as a failed read, not confirmed source changes', async () => {
    setupKnowledge({
      snapshots: {
        W1: { ok: false, error: { code: 'Unavailable', message: 'request outcome unavailable' } },
        W2: snapshotFor('W2'),
      },
    });
    renderApp('/projects/demo?tab=knowledge');
    expect(await screen.findByText(/读取失败/, undefined, { timeout: 5000 })).toBeTruthy();
    expect(screen.queryByTestId('path-digest-notice')).toBeNull();
  });

  it('shows each citing work contract with that work label', async () => {
    const spec = specFor();
    setupKnowledge({
      snapshots: {
        W1: snapshotWithSpec('W1', spec, { goals: ['GOAL OF issue-filter-api'], acceptance: ['ACCEPT API'] }),
        W2: snapshotWithSpec('W2', spec, { goals: ['GOAL OF issue-filter-page'], acceptance: ['ACCEPT PAGE'] }),
      },
    });
    renderApp('/projects/demo?tab=knowledge');
    expect(await screen.findByText('GOAL OF issue-filter-api')).toBeTruthy();
    expect(screen.getByText('GOAL OF issue-filter-page')).toBeTruthy();
    expect(screen.getByText('合同目标与验收（issue-filter-api）')).toBeTruthy();
    expect(screen.getByText('合同目标与验收（issue-filter-page）')).toBeTruthy();
    expect(screen.getByText('ACCEPT API')).toBeTruthy();
    expect(screen.getByText('ACCEPT PAGE')).toBeTruthy();
  });

  it('verifies an untouched document digest with Web Crypto', async () => {
    vi.stubGlobal('crypto', webcrypto);
    setupKnowledge({ snapshots: { W1: snapshotFor('W1'), W2: snapshotFor('W2') } });
    renderApp('/projects/demo?tab=knowledge');
    expect(await screen.findByText('摘要已校验')).toBeTruthy();
    expect(screen.queryByText('摘要不匹配，正文不可信')).toBeNull();
  });

  it('flags a same-length text change as digest mismatch', async () => {
    vi.stubGlobal('crypto', webcrypto);
    const spec = specFor('abc', { sha256: sha256('abd') });
    const snapshot = snapshotWithSpec('W1', spec);
    setupKnowledge({ snapshots: { W1: snapshot, W2: snapshot } });
    renderApp('/projects/demo?tab=knowledge');
    expect(await screen.findByText('摘要不匹配，正文不可信')).toBeTruthy();
    expect(screen.queryByText('摘要已校验')).toBeNull();
  });

  it('says not verified when crypto.subtle is unavailable', async () => {
    vi.stubGlobal('crypto', {});
    setupKnowledge({ snapshots: { W1: snapshotFor('W1'), W2: snapshotFor('W2') } });
    renderApp('/projects/demo?tab=knowledge');
    expect(await screen.findByText('未校验')).toBeTruthy();
    expect(screen.queryByText('摘要已校验')).toBeNull();
  });

  it('filters loaded sources case-insensitively and only within loaded content', async () => {
    const spec = specFor();
    setupKnowledge({ snapshots: { W1: snapshotFor('W1'), W2: snapshotFor('W2') } });
    renderApp('/projects/demo?tab=knowledge');
    const input = await screen.findByPlaceholderText(/筛选当前页/);
    const user = userEvent.setup();
    await user.type(input, 'SUPPORT.MD');
    expect(screen.getByTestId(sourceTestId(spec))).toBeTruthy();
    await user.clear(input);
    await user.type(input, 'no-such-file');
    expect(screen.queryByTestId(sourceTestId(spec))).toBeNull();
    await user.clear(input);
    expect(screen.getByTestId(sourceTestId(spec))).toBeTruthy();
  });

  it('shows the empty state when no readable reference exists', async () => {
    const empty = snapshotFor('W1', { required_specs: [], authorized_readable_refs: [] });
    setupKnowledge({ snapshots: { W1: empty, W2: empty } });
    renderApp('/projects/demo?tab=knowledge');
    expect(await screen.findByTestId('state-empty')).toBeTruthy();
  });

  it('shows forbidden when the overview read has an auth error', async () => {
    const fetchMock = vi.fn(async (input: RequestInfo | URL) => {
      if (String(input).includes('/api/team/projects')) return json(DEMO_PROJECTS);
      return json({ ok: false, error: { code: 'Forbidden', message: 'denied' } });
    });
    vi.stubGlobal('fetch', fetchMock);
    renderApp('/projects/demo?tab=knowledge');
    expect(await screen.findByTestId('state-forbidden')).toBeTruthy();
    expect(screen.queryByTestId('page-knowledge')).toBeNull();
  });

  it('clears restricted content and silences snapshot reads after revocation', async () => {
    const { calls } = setupKnowledge({ snapshots: { W1: snapshotFor('W1'), W2: snapshotFor('W2') } });
    const queryClient = createAppQueryClient({ retry: false });
    // Mount this page without the shell: the shell legitimately unmounts the
    // whole page on revocation, which would hide this page's own enabled guard.
    render(
      <QueryClientProvider client={queryClient}>
        <RevocationProvider>
          <MemoryRouter>
            <ProjectKnowledge project="demo" />
          </MemoryRouter>
        </RevocationProvider>
      </QueryClientProvider>,
    );
    await screen.findByTestId(sourceTestId(specFor()));
    await screen.findByText('来自 2／2 个工作项');
    const readsBeforeRevoke = calls.filter(({ url }) => url.includes('/api/team/read')).length;
    expect(readsBeforeRevoke).toBeGreaterThan(0);

    act(() => revokeObservationReads(queryClient));
    expect(await screen.findByTestId('state-forbidden')).toBeTruthy();
    expect(screen.queryByText(/Build a useful filterable queue/)).toBeNull();
    const settled = calls.length;
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 1200));
    });
    expect(calls.length).toBe(settled);

    // Even a cached overview reappearing after revocation must not reactivate
    // per-work reads: the enabled flag is the last client-side guard.
    act(() => {
      queryClient.setQueryData(queryKeys.overview('demo'), {
        ...DEMO_OVERVIEW,
        works: WORKS,
        workstreams: [{ id: 'WS1', external_key: 'support', title: 'Customer support queue' }],
      });
    });
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 50));
    });
    expect(calls.filter(({ url }) => url.includes('/api/team/read')).length).toBe(readsBeforeRevoke);
    expect(calls.length).toBe(settled);
  });
});
