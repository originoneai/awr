import { createHash, webcrypto } from 'node:crypto';
import { QueryClientProvider } from '@tanstack/react-query';
import { act, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { MemoryRouter } from 'react-router';
import { afterEach, describe, expect, it, vi } from 'vitest';
import i18n from './i18n';
import { defaultVisibleText, findForbiddenVisibleText } from './lib/human-language-guard';
import { focusManager, onlineManager } from '@tanstack/react-query';
import { RevocationProvider } from './lib/revocation';
import { createAppQueryClient, queryKeys, revokeObservationReads } from './lib/queries';
import { ProjectKnowledge } from './pages/ProjectKnowledge';
import { DEMO_OVERVIEW, DEMO_PROJECTS, renderApp } from './test-utils';
import { realSourceContentReply, realSourceContentWrongDigestReply } from './test-fixtures/real-shaped';

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
  /** Absent when the citing response carried no text (a readable reference). */
  text?: string;
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
  sourceContent,
}: {
  snapshots: Record<string, unknown>;
  works?: typeof WORKS;
  sourceContent?: (query: { source_path?: string; expected_sha256?: string }) => unknown;
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
      if (query.op === 'source.content' && sourceContent) {
        return json(sourceContent(query));
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
  vi.useRealTimers();
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

  it('uses the 60-second list cadence for every visible work snapshot', async () => {
    const { calls } = setupKnowledge({ snapshots: { W1: snapshotFor('W1'), W2: snapshotFor('W2') } });
    const snapshotReads = () => calls.filter(({ init }) => String(init?.body ?? '').includes('"work.snapshot"'));
    const { queryClient } = renderApp('/projects/demo?tab=knowledge');
    await waitFor(() => expect(snapshotReads()).toHaveLength(2));
    const snapshots = queryClient.getQueryCache().getAll().filter((query) => (
      query.queryKey[0] === 'team' && query.queryKey[1] === 'work.snapshot'
    ));
    expect(snapshots).toHaveLength(2);
    expect(snapshots.map((query) => (
      'refetchInterval' in query.options ? query.options.refetchInterval : undefined
    ))).toEqual([60000, 60000]);
  });

  it('hydrates only the visible 60-work page and reads the next work on demand', async () => {
    const user = userEvent.setup();
    const works = Array.from({ length: 61 }, (_, index) => ({
      key: `W${index + 1}`,
      title: `Knowledge work ${index + 1}`,
      status: 'unknown',
      owner_person: null,
      agent: null,
      next_step: null,
      detail_loaded: false,
      workstream_id: 'WS1',
      contract_hash: `c${index + 1}`,
    }));
    const snapshots = Object.fromEntries(works.map((work) => [work.key, snapshotFor(work.key)]));
    const { calls } = setupKnowledge({ works, snapshots });
    const snapshotReads = () => calls.filter(({ init }) => String(init?.body ?? '').includes('"work.snapshot"'));
    const { router } = renderApp('/projects/demo?tab=knowledge');
    await waitFor(() => expect(screen.getByRole('link', { name: 'Knowledge work 1' })).toBeTruthy(), { timeout: 10000 });
    await waitFor(() => expect(snapshotReads()).toHaveLength(60), { timeout: 10000 });
    expect(screen.queryByRole('link', { name: 'Knowledge work 61' })).toBeNull();

    await user.click(screen.getByRole('listitem', { name: '下一页' }));
    await screen.findByRole('link', { name: 'Knowledge work 61' });
    await waitFor(() => expect(snapshotReads()).toHaveLength(61));
    expect(new URLSearchParams(router.state.location.search).get('page')).toBe('2');
  }, 20000);

  it('clamps an out-of-range page and hydrates only the last visible page', async () => {
    const works = Array.from({ length: 61 }, (_, index) => ({
      ...WORKS[index % WORKS.length],
      key: `W${index + 1}`,
      title: `Knowledge work ${index + 1}`,
      contract_hash: `c${index + 1}`,
    }));
    const snapshots = Object.fromEntries(works.map((work) => [work.key, snapshotFor(work.key)]));
    const { calls } = setupKnowledge({ works, snapshots });
    const snapshotReads = () => calls.filter(({ init }) => String(init?.body ?? '').includes('"work.snapshot"'));
    renderApp('/projects/demo?tab=knowledge&page=99');

    expect(await screen.findByRole('link', { name: 'Knowledge work 61' })).toBeTruthy();
    expect(screen.queryByRole('link', { name: 'Knowledge work 1' })).toBeNull();
    await waitFor(() => expect(snapshotReads()).toHaveLength(1));
    expect(String(snapshotReads()[0].init?.body)).toContain('"work_id":"W61"');
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
    expect(await screen.findByTestId('knowledge-citing-works')).toHaveProperty('textContent', '引用它的工作项: Support filter · Support filter');
    const links = screen.getAllByRole('link', { name: 'Support filter' });
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
    // the document is named by its file name; neither its path nor a hash is in the default view
    expect(screen.getAllByText('support')).toHaveLength(2);
    expect(screen.queryByText(/[a-f0-9]{12}…/)).toBeNull();
  });

  it('keeps the path, the full digest and the size in the technical details and none of them in the default view', async () => {
    const spec = specFor();
    setupKnowledge({ snapshots: { W1: snapshotFor('W1'), W2: snapshotFor('W2') } });
    renderApp('/projects/demo?tab=knowledge');
    await screen.findByTestId(sourceTestId(spec));
    const page = screen.getByTestId('page-knowledge');
    expect(findForbiddenVisibleText(defaultVisibleText(page))).toEqual([]);
    expect(defaultVisibleText(page)).not.toContain(spec.sha256.slice(0, 12));
    expect(defaultVisibleText(page)).not.toContain('docs/support.md');
    await userEvent.setup().click(within(page).getByRole('button', { name: /技术细节/ }));
    expect((await screen.findByTestId('technical-detail-sha256')).textContent).toContain(spec.sha256);
    expect(screen.getByTestId('technical-detail-path').textContent).toContain('docs/support.md');
    expect(screen.getByTestId('technical-detail-byte_length').textContent).toContain(String(spec.byte_length));
  });

  it('shows only metadata when content_included is false', async () => {
    const spec = specFor(SPEC_TEXT, { content_included: false });
    const snapshot = snapshotWithSpec('W1', spec);
    setupKnowledge({ snapshots: { W1: snapshot, W2: snapshot } });
    renderApp('/projects/demo?tab=knowledge');
    await screen.findByTestId(sourceTestId(spec));
    expect(screen.queryByText(/Build a useful filterable queue/)).toBeNull();
    expect((await screen.findAllByText(/没有收到正文/)).length).toBeGreaterThan(0);
  });

  it('treats a missing content_included flag as included when text is present', async () => {
    const spec = specFor(SPEC_TEXT, { content_included: undefined });
    const snapshot = snapshotWithSpec('W1', spec);
    setupKnowledge({ snapshots: { W1: snapshot, W2: snapshot } });
    renderApp('/projects/demo?tab=knowledge');
    expect(await screen.findByText(/Build a useful filterable queue/)).toBeTruthy();
  });

  it('does not mark a complete multi-byte document partial', async () => {
    const text = '# 客户支持队列\n\n按标签筛选。';
    const spec = specFor(text);
    const snapshot = snapshotWithSpec('W1', spec);
    setupKnowledge({ snapshots: { W1: snapshot, W2: snapshot } });
    renderApp('/projects/demo?tab=knowledge');
    expect(await screen.findByText((_, element) => (
      element?.classList.contains('ant-typography') === true && element.textContent === text
    ))).toBeTruthy();
    expect(screen.queryByText(/内容不完整/)).toBeNull();
  });

  it('still marks truncated ASCII text partial using UTF-8 byte length', async () => {
    const spec = specFor('abcdef', { byte_length: 7 });
    const snapshot = snapshotWithSpec('W1', spec);
    setupKnowledge({ snapshots: { W1: snapshot, W2: snapshot } });
    renderApp('/projects/demo?tab=knowledge');
    expect(await screen.findByText(/内容不完整/)).toBeTruthy();
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
    expect(screen.getByTestId('path-digest-notice').textContent).toContain('2 个不同版本');
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

  it('shows a goal or acceptance entry that is not a plain sentence as its exact JSON, a number or a flag as its text', async () => {
    const spec = specFor();
    const snapshot = snapshotFor('W1', {
      required_specs: [spec],
      authorized_readable_refs: [refFor(spec)],
      visible_contract: { goals: [{ kind: 'metric', target: 5 }, 7, true], acceptance: [{ check: 'empty list' }] },
    });
    setupKnowledge({ snapshots: { W1: snapshot, W2: snapshot } });
    renderApp('/projects/demo?tab=knowledge');
    expect((await screen.findAllByText('{"kind":"metric","target":5}')).length).toBeGreaterThan(0);
    expect(screen.getAllByText('7').length).toBeGreaterThan(0);
    expect(screen.getAllByText('true').length).toBeGreaterThan(0);
    expect(screen.getAllByText('{"check":"empty list"}').length).toBeGreaterThan(0);
    expect(screen.queryByText('[object Object]')).toBeNull();
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
    expect(screen.getByText('目标与验收（issue-filter-api）')).toBeTruthy();
    expect(screen.getByText('目标与验收（issue-filter-page）')).toBeTruthy();
    expect(screen.getByText('ACCEPT API')).toBeTruthy();
    expect(screen.getByText('ACCEPT PAGE')).toBeTruthy();
  });

  it('verifies an untouched document digest with Web Crypto', async () => {
    vi.stubGlobal('crypto', webcrypto);
    setupKnowledge({ snapshots: { W1: snapshotFor('W1'), W2: snapshotFor('W2') } });
    renderApp('/projects/demo?tab=knowledge');
    expect(await screen.findByText('内容未被改动')).toBeTruthy();
    expect(screen.queryByText('内容对不上，请勿采信')).toBeNull();
  });

  it('flags a text that does not hash to its declared digest', async () => {
    vi.stubGlobal('crypto', webcrypto);
    const spec = specFor('abc', { sha256: sha256('a different text') });
    const snapshot = snapshotWithSpec('W1', spec);
    setupKnowledge({ snapshots: { W1: snapshot, W2: snapshot } });
    renderApp('/projects/demo?tab=knowledge');
    expect(await screen.findByText('内容对不上，请勿采信')).toBeTruthy();
    expect(screen.queryByText('内容未被改动')).toBeNull();
  });

  it('says not verified when crypto.subtle is unavailable', async () => {
    vi.stubGlobal('crypto', {});
    setupKnowledge({ snapshots: { W1: snapshotFor('W1'), W2: snapshotFor('W2') } });
    renderApp('/projects/demo?tab=knowledge');
    expect(await screen.findByText('尚未核对内容')).toBeTruthy();
    expect(screen.queryByText('内容未被改动')).toBeNull();
  });

  it('filters loaded sources case-insensitively and only within loaded content', async () => {
    const spec = specFor();
    setupKnowledge({ snapshots: { W1: snapshotFor('W1'), W2: snapshotFor('W2') } });
    renderApp('/projects/demo?tab=knowledge');
    const input = await screen.findByPlaceholderText(/按名称筛选/);
    const user = userEvent.setup();
    await user.type(input, 'SUPPORT.MD');
    expect(screen.getByTestId(sourceTestId(spec))).toBeTruthy();
    await user.clear(input);
    await user.type(input, 'no-such-file');
    expect(screen.queryByTestId(sourceTestId(spec))).toBeNull();
    await user.clear(input);
    expect(screen.getByTestId(sourceTestId(spec))).toBeTruthy();
  });

  it('names two works that cite the same document by what they agree on, once, under both names', async () => {
    const spec = specFor();
    setupKnowledge({ snapshots: { W1: snapshotFor('W1'), W2: snapshotFor('W2') } });
    renderApp('/projects/demo?tab=knowledge');
    expect(await screen.findByText('目标与验收（issue-filter-api、issue-filter-page）')).toBeTruthy();
    expect(screen.getAllByText('Filter issues by label')).toHaveLength(1);
    expect(screen.queryByTestId(sourceTestId(spec))).toBeTruthy();
  });

  it('groups the documents the works must follow apart from the references they may read', async () => {
    const spec = specFor();
    const reference = { path: 'docs/notes/team_glossary.md', sha256: sha256('glossary'), byte_length: 8 };
    const snapshot = snapshotFor('W1', { required_specs: [spec], authorized_readable_refs: [refFor(spec), { kind: 'source', ...reference }] });
    setupKnowledge({ snapshots: { W1: snapshot, W2: snapshot } });
    renderApp('/projects/demo?tab=knowledge');
    await screen.findByTestId(sourceTestId(spec));
    expect(screen.getByText('要遵守的文档')).toBeTruthy();
    expect(screen.getByText('可读的参考')).toBeTruthy();
    expect(screen.getByText('team glossary')).toBeTruthy();
    expect(screen.getAllByText('引用它的工作项：2')).toHaveLength(2);
    await userEvent.setup().click(screen.getByTestId(`knowledge-source-${reference.path}-${reference.sha256.slice(0, 12)}`));
    expect(await screen.findAllByText('没有收到正文')).not.toHaveLength(0);
    // the filter finds a document by the name it is shown under as well as by its path
    const user = userEvent.setup();
    await user.type(screen.getByPlaceholderText(/按名称筛选/), 'team glossary');
    expect(screen.getByTestId(`knowledge-source-${reference.path}-${reference.sha256.slice(0, 12)}`)).toBeTruthy();
    expect(screen.queryByTestId(sourceTestId(spec))).toBeNull();
  });

  it('numbers a document whose file name the guard would flag, and still finds it by its path', async () => {
    const spec = specFor('FLAGGED', { path: 'docs/a.b.c.md' });
    const snapshot = snapshotWithSpec('W1', spec);
    setupKnowledge({ snapshots: { W1: snapshot, W2: snapshot } });
    renderApp('/projects/demo?tab=knowledge');
    await screen.findByTestId(sourceTestId(spec));
    expect(screen.getAllByText('文档 1')).toHaveLength(2);
    expect(findForbiddenVisibleText(defaultVisibleText(screen.getByTestId('page-knowledge')))).toEqual([]);
    const user = userEvent.setup();
    await user.type(screen.getByPlaceholderText(/按名称筛选/), 'a.b.c');
    expect(screen.getByTestId(sourceTestId(spec))).toBeTruthy();
  });

  it.each([
    ['a missing digest', { sha256: undefined }, '尚未核对内容', 'support'],
    ['a missing path', { path: undefined }, '内容未被改动', '文档 1'],
    ['a missing size', { byte_length: undefined }, '内容未被改动', 'support'],
    ['no digest, path or size at all', { sha256: undefined, path: undefined, byte_length: undefined }, '尚未核对内容', '文档 1'],
  ])('does not throw on a document with %s', async (_name, missing, verdict, name) => {
    vi.stubGlobal('crypto', webcrypto);
    const spec = { ...specFor(), ...missing };
    const snapshot = snapshotFor('W1', { required_specs: [spec], authorized_readable_refs: [] });
    setupKnowledge({ snapshots: { W1: snapshot, W2: snapshot } });
    renderApp('/projects/demo?tab=knowledge');
    await waitFor(() => expect(screen.getByTestId('digest-check').textContent).toBe(verdict));
    expect(screen.queryByText(/Unexpected Application Error/)).toBeNull();
    expect(screen.getAllByText(name).length).toBeGreaterThan(0);
    expect(findForbiddenVisibleText(defaultVisibleText(screen.getByTestId('page-knowledge')))).toEqual([]);
  });

  it('reads in English: documents to follow, content unchanged, no engineering word', async () => {
    vi.stubGlobal('crypto', webcrypto);
    setupKnowledge({ snapshots: { W1: snapshotFor('W1'), W2: snapshotFor('W2') } });
    renderApp('/projects/demo?tab=knowledge');
    await screen.findByTestId('digest-check');
    await act(() => i18n.changeLanguage('en-US'));
    expect(await screen.findByText('Documents to follow')).toBeTruthy();
    expect(await screen.findByText('Content unchanged')).toBeTruthy();
    expect(screen.getAllByText('Work items citing it: 2')).toHaveLength(1);
    expect(findForbiddenVisibleText(defaultVisibleText(screen.getByTestId('page-knowledge')))).toEqual([]);
    await act(() => i18n.changeLanguage('zh-CN'));
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
    vi.useFakeTimers();
    await act(async () => {
      await vi.advanceTimersByTimeAsync(61000);
    });
    vi.useRealTimers();
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

describe('knowledge full-text read (OBS-009 slice 2)', () => {
  // Real fixture replies: the run's own support.md (433 bytes) and the 503 body the
  // service answers when expected_sha256 does not match.
  const REAL_TEXT = realSourceContentReply().result.data.text as string;
  const REAL_SHA = realSourceContentReply().result.data.sha256 as string;
  const REAL_BYTES = realSourceContentReply().result.data.byte_length as number;

  function refOnlySnapshots() {
    // A work that cites the document as a readable reference (no text in the response).
    const spec = specFor(REAL_TEXT, { sha256: REAL_SHA, byte_length: REAL_BYTES, content_included: false });
    const withoutText = { ...spec, text: undefined } as typeof spec;
    const snapshots = { W1: snapshotWithSpec('W1', withoutText), W2: snapshotWithSpec('W2', withoutText) };
    return { spec, snapshots };
  }

  function countSourceReads(calls: Array<{ url: string; init?: RequestInit }>) {
    return calls.filter(({ url, init }) => url.includes('/api/team/read')
      && JSON.parse(String(init?.body)).query.op === 'source.content').length;
  }

  function sourceReadCall(calls: Array<{ url: string; init?: RequestInit }>) {
    return calls
      .filter(({ url, init }) => url.includes('/api/team/read') && init?.body != null)
      .map(({ url, init }) => ({ url, body: JSON.parse(String(init?.body)) as { query?: { op?: string; source_path?: string; expected_sha256?: string } } }))
      .find(({ body }) => body.query?.op === 'source.content');
  }

  it('sends no source read before the button is pressed, and asks correctly once it is', async () => {
    const user = userEvent.setup();
    const { snapshots } = refOnlySnapshots();
    const { calls } = setupKnowledge({
      snapshots,
      sourceContent: () => realSourceContentReply(),
    });
    renderApp('/projects/demo?tab=knowledge');
    const spec = specFor(REAL_TEXT, { sha256: REAL_SHA });
    await screen.findAllByTestId(sourceTestId(spec));
    await waitFor(() => expect(screen.getByTestId('read-full')).toBeTruthy());
    expect(countSourceReads(calls)).toBe(0);

    await user.click(screen.getByTestId('read-full'));
    await waitFor(() => expect(screen.getByTestId('read-text')).toBeTruthy());
    const call = sourceReadCall(calls);
    expect(call?.body.query?.source_path).toBe('docs/support.md');
    expect(call?.body.query?.expected_sha256).toBe(REAL_SHA);
  });

  it('shows the whole text with the verified stamp for a matching full read', async () => {
    const user = userEvent.setup();
    const { snapshots } = refOnlySnapshots();
    setupKnowledge({ snapshots, sourceContent: () => realSourceContentReply() });
    renderApp('/projects/demo?tab=knowledge');
    await user.click(await screen.findByTestId('read-full'));
    const text = await screen.findByTestId('read-text');
    expect(text.textContent).toContain('# Customer support queue');
    await waitFor(() => expect(screen.getByTestId('read-digest').textContent).toContain('内容未被改动'));
    expect(screen.getByTestId('read-result').textContent).not.toContain('只显示');
  });

  it('says only the front part is shown when the service cut the read short', async () => {
    const user = userEvent.setup();
    const { snapshots } = refOnlySnapshots();
    const cut = { ...realSourceContentReply() };
    cut.result.data = { ...cut.result.data, text: REAL_TEXT.slice(0, 40) };
    setupKnowledge({ snapshots, sourceContent: () => cut });
    renderApp('/projects/demo?tab=knowledge');
    await user.click(await screen.findByTestId('read-full'));
    await screen.findByTestId('read-text');
    expect(screen.getByTestId('read-result').textContent).toContain('只显示了前一部分');
    expect(screen.getByTestId('read-text').textContent).not.toContain('Build a useful filterable queue');
    expect(screen.getByTestId('read-result').textContent).not.toContain('内容对不上');
  });

  it('drops the text and says the content does not match when the digests disagree', async () => {
    const user = userEvent.setup();
    const withText = specFor(REAL_TEXT, { sha256: sha256('a different document'), byte_length: REAL_BYTES, content_included: false });
    const spec = { ...withText, text: undefined } as typeof withText;
    setupKnowledge({
      snapshots: { W1: snapshotWithSpec('W1', spec), W2: snapshotWithSpec('W2', spec) },
      sourceContent: () => realSourceContentReply(),
    });
    renderApp('/projects/demo?tab=knowledge');
    await user.click(await screen.findByTestId('read-full'));
    await waitFor(() => expect(screen.getByTestId('read-result').textContent).toContain('内容对不上，请勿采信'));
    expect(screen.queryByTestId('read-text')).toBeNull();
  });

  it('says the read failed with a retry for a 503, never a confirmed change', async () => {
    const user = userEvent.setup();
    const { snapshots } = refOnlySnapshots();
    setupKnowledge({ snapshots, sourceContent: () => realSourceContentWrongDigestReply() });
    renderApp('/projects/demo?tab=knowledge');
    await user.click(await screen.findByTestId('read-full'));
    const failed = await screen.findByTestId('read-failed');
    expect(failed.textContent).toContain('读取失败');
    expect(screen.getByTestId('read-retry')).toBeTruthy();
    const visible = defaultVisibleText(screen.getByTestId('page-knowledge'));
    expect(visible).not.toContain('变化');
    expect(visible).not.toContain('摘要');
    expect(findForbiddenVisibleText(visible)).toEqual([]);
  });

  it('sends one more read when the person retries a failed one', async () => {
    const user = userEvent.setup();
    const { snapshots } = refOnlySnapshots();
    let replies = 0;
    const { calls } = setupKnowledge({
      snapshots,
      sourceContent: () => {
        replies += 1;
        return replies === 1 ? realSourceContentWrongDigestReply() : realSourceContentReply();
      },
    });
    renderApp('/projects/demo?tab=knowledge');
    await user.click(await screen.findByTestId('read-full'));
    await screen.findByTestId('read-failed');
    expect(countSourceReads(calls)).toBe(1);

    await user.click(screen.getByTestId('read-retry'));
    await waitFor(() => expect(screen.getByTestId('read-text')).toBeTruthy());
    expect(countSourceReads(calls)).toBe(2);
  });

  it('offers the entry for a truncated embedded text as well', async () => {
    const spec = specFor(REAL_TEXT.slice(0, 20), { sha256: REAL_SHA, byte_length: REAL_BYTES });
    setupKnowledge({
      snapshots: { W1: snapshotWithSpec('W1', spec), W2: snapshotWithSpec('W2', spec) },
      sourceContent: () => realSourceContentReply(),
    });
    renderApp('/projects/demo?tab=knowledge');
    expect(await screen.findByTestId('read-full')).toBeTruthy();
  });

  it('offers no entry when the embedded text is whole and checks out', async () => {
    const spec = specFor(REAL_TEXT, { sha256: REAL_SHA, byte_length: REAL_BYTES });
    setupKnowledge({ snapshots: { W1: snapshotWithSpec('W1', spec), W2: snapshotWithSpec('W2', spec) } });
    renderApp('/projects/demo?tab=knowledge');
    await screen.findAllByTestId(sourceTestId(spec));
    await waitFor(() => expect(screen.getByTestId('digest-check').textContent).toContain('内容未被改动'));
    expect(screen.queryByTestId('read-full')).toBeNull();
  });

  it('offers no entry and no request for a reference the bridge would refuse', async () => {
    const withText = specFor(REAL_TEXT, { path: '/etc/passwd', sha256: REAL_SHA, byte_length: REAL_BYTES, content_included: false });
    const spec = { ...withText, text: undefined } as typeof withText;
    const { calls } = setupKnowledge({
      snapshots: { W1: snapshotWithSpec('W1', spec), W2: snapshotWithSpec('W2', spec) },
    });
    renderApp('/projects/demo?tab=knowledge');
    await screen.findAllByTestId(sourceTestId(spec)); // the list entry exists, the read entry does not
    expect(screen.queryByTestId('read-full')).toBeNull();
    expect(countSourceReads(calls)).toBe(0);
    expect(screen.getByTestId('read-unavailable').textContent).toContain('暂时不能读取全文');
  });

  it('reads once and keeps the result when the same document is asked again', async () => {
    const user = userEvent.setup();
    const { snapshots } = refOnlySnapshots();
    const { calls } = setupKnowledge({ snapshots, sourceContent: () => realSourceContentReply() });
    renderApp('/projects/demo?tab=knowledge');
    await user.click(await screen.findByTestId('read-full'));
    await screen.findByTestId('read-text');
    // Re-render (component remount shares the query cache) asks nothing again.
    renderApp('/projects/demo?tab=knowledge');
    await screen.findByTestId('read-text');
    expect(countSourceReads(calls)).toBe(1);
  });

  it('silences the read and hides its result after revocation', async () => {
    const user = userEvent.setup();
    const { snapshots } = refOnlySnapshots();
    const { calls } = setupKnowledge({ snapshots, sourceContent: () => realSourceContentReply() });
    const queryClient = createAppQueryClient({ retry: false });
    // Mounted without the shell, like the existing revocation test: the page's own
    // enabled guard is what must keep the read silent.
    render(
      <QueryClientProvider client={queryClient}>
        <RevocationProvider>
          <MemoryRouter>
            <ProjectKnowledge project="demo" />
          </MemoryRouter>
        </RevocationProvider>
      </QueryClientProvider>,
    );
    await user.click(await screen.findByTestId('read-full'));
    await waitFor(() => expect(screen.getByTestId('read-text').textContent).toContain('# Customer support queue'));
    const readsAtRevoke = countSourceReads(calls);

    act(() => revokeObservationReads(queryClient));
    expect(await screen.findByTestId('state-forbidden')).toBeTruthy();
    expect(screen.queryByText(/Customer support queue/)).toBeNull();
    vi.useFakeTimers();
    await act(async () => {
      await vi.advanceTimersByTimeAsync(61000);
    });
    vi.useRealTimers();
    expect(countSourceReads(calls)).toBe(readsAtRevoke);
  });

  it('words the read in English after a language switch', async () => {
    const user = userEvent.setup();
    const { snapshots } = refOnlySnapshots();
    setupKnowledge({ snapshots, sourceContent: () => realSourceContentWrongDigestReply() });
    renderApp('/projects/demo?tab=knowledge');
    await act(() => i18n.changeLanguage('en-US'));
    expect(await screen.findByTestId('read-full')).toBeTruthy();
    expect(screen.getByTestId('read-full').textContent).toContain('Read the full text');
    await user.click(screen.getByTestId('read-full'));
    const failed = await screen.findByTestId('read-failed');
    expect(failed.textContent).toContain('The read failed');
    expect(screen.getByTestId('read-retry').textContent).toContain('Retry');
    const visible = defaultVisibleText(screen.getByTestId('page-knowledge'));
    expect(visible).not.toMatch(/chang/i);
    expect(visible).not.toMatch(/digest/i);
  });
});

describe('knowledge read cadence and promises (review)', () => {
  const REAL_TEXT2 = realSourceContentReply().result.data.text as string;
  const REAL_SHA2 = realSourceContentReply().result.data.sha256 as string;
  const REAL_BYTES2 = realSourceContentReply().result.data.byte_length as number;

  function refSpec(overrides: Partial<SpecFixture> = {}) {
    const base = specFor(REAL_TEXT2, { sha256: REAL_SHA2, byte_length: REAL_BYTES2, content_included: false });
    return { ...base, text: undefined, ...overrides };
  }

  function countReads(calls: Array<{ url: string; init?: RequestInit }>) {
    return calls.filter(({ url, init }) => url.includes('/api/team/read')
      && JSON.parse(String(init?.body)).query.op === 'source.content').length;
  }

  afterEach(() => {
    focusManager.setFocused(false);
    onlineManager.setOnline(true);
  });

  it('never re-reads on its own after a successful read: time, focus and reconnect', async () => {
    const user = userEvent.setup();
    const { calls } = setupKnowledge({
      snapshots: { W1: snapshotWithSpec('W1', refSpec()), W2: snapshotWithSpec('W2', refSpec()) },
      sourceContent: () => realSourceContentReply(),
    });
    renderApp('/projects/demo?tab=knowledge');
    await user.click(await screen.findByTestId('read-full'));
    await screen.findByTestId('read-text');
    expect(countReads(calls)).toBe(1);

    vi.useFakeTimers();
    await act(async () => { await vi.advanceTimersByTimeAsync(61000); });
    act(() => { focusManager.setFocused(true); });
    await act(async () => { await vi.advanceTimersByTimeAsync(61000); });
    act(() => { onlineManager.setOnline(false); onlineManager.setOnline(true); });
    await act(async () => { await vi.advanceTimersByTimeAsync(61000); });
    vi.useRealTimers();
    expect(countReads(calls)).toBe(1);
  });

  it('never re-reads on its own after a failed read: time, focus and reconnect; retry adds exactly one', async () => {
    const user = userEvent.setup();
    const { calls } = setupKnowledge({
      snapshots: { W1: snapshotWithSpec('W1', refSpec()), W2: snapshotWithSpec('W2', refSpec()) },
      sourceContent: () => realSourceContentWrongDigestReply(),
    });
    renderApp('/projects/demo?tab=knowledge');
    await user.click(await screen.findByTestId('read-full'));
    await screen.findByTestId('read-failed');
    expect(countReads(calls)).toBe(1);

    vi.useFakeTimers();
    await act(async () => { await vi.advanceTimersByTimeAsync(61000); });
    act(() => { focusManager.setFocused(true); });
    await act(async () => { await vi.advanceTimersByTimeAsync(61000); });
    act(() => { onlineManager.setOnline(false); onlineManager.setOnline(true); });
    await act(async () => { await vi.advanceTimersByTimeAsync(61000); });
    vi.useRealTimers();
    expect(countReads(calls)).toBe(1);

    await user.click(screen.getByTestId('read-retry'));
    await waitFor(() => expect(screen.getByTestId('read-failed')).toBeTruthy());
    expect(countReads(calls)).toBe(2);
  });
});
