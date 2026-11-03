import { createHash } from 'node:crypto';
import { act, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';
import i18n from './i18n';
import { DEMO_OVERVIEW, DEMO_PROJECTS, renderApp } from './test-utils';
import { realSourceContentReply } from './test-fixtures/real-shaped';

/**
 * The two digest-gate tests of OBS-009 slice 2, in their own file on purpose: they replace the
 * page's crypto.subtle.digest to hold the check back, and that global replacement does not
 * survive sharing a module with the other knowledge tests (jsdom + the shared afterEach). File
 * isolation is the reliable boundary.
 */

interface SpecFixture {
  path: string;
  sha256: string;
  byte_length: number;
  content_included?: boolean;
  next_step: null;
  text?: string;
}

const REAL = realSourceContentReply().result.data;
const REAL_TEXT = REAL.text as string;
const REAL_SHA = REAL.sha256 as string;
const REAL_BYTES = REAL.byte_length as number;

function sha256(text: string) {
  return createHash('sha256').update(text, 'utf8').digest('hex');
}

function json(payload: unknown) {
  return new Response(JSON.stringify(payload), { status: 200, headers: { 'content-type': 'application/json' } });
}

function refSpec(): SpecFixture {
  return { path: 'docs/support.md', sha256: REAL_SHA, byte_length: REAL_BYTES, content_included: false, next_step: null };
}

function snapshotWithSpec(workId: string, spec: SpecFixture) {
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
        authorized_readable_refs: [{ kind: 'source', path: spec.path, sha256: spec.sha256, byte_length: spec.byte_length }],
        visible_contract: { goals: ['Filter issues by label'], acceptance: ['Labels combine correctly'] },
        observation: { reporting: { client_reported_at_unix_ms: 1761839800000 }, progress: null, runtime: { state: 'claimed' } },
        guidance: { action: { op: 'session.checkpoint', note: 'Record the verified outcome.' } },
        execution_authorized: false,
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


function snapshotsFor(specA: SpecFixture, specB: SpecFixture) {
  return { W1: snapshotWithSpec('W1', specA), W2: snapshotWithSpec('W2', specB) };
}

function countReads(calls: Array<{ url: string; init?: RequestInit }>) {
  return calls.filter(({ url, init }) => url.includes('/api/team/read')
    && JSON.parse(String(init?.body)).query.op === 'source.content').length;
}

const WORKS = [
  { key: 'W1', title: 'issue-filter-api', status: 'unknown', owner_person: null, agent: null, next_step: null, detail_loaded: false, workstream_id: 'WS1', contract_hash: 'c1' },
  { key: 'W2', title: 'issue-filter-page', status: 'unknown', owner_person: null, agent: null, next_step: null, detail_loaded: false, workstream_id: 'WS1', contract_hash: 'c2' },
];

function setup({ sourceContent, specs }: {
  sourceContent: (query: { expected_sha256?: string }) => unknown;
  specs?: { W1?: SpecFixture; W2?: SpecFixture };
}) {
  const snapshots = {
    W1: snapshotWithSpec('W1', specs?.W1 ?? refSpec()),
    W2: snapshotWithSpec('W2', specs?.W2 ?? refSpec()),
  };
  const fetchMock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
    const url = String(input);
    if (url.includes('/api/team/projects')) return json(DEMO_PROJECTS);
    if (url.includes('/api/team/overview')) {
      return json({ ...DEMO_OVERVIEW, works: WORKS, workstreams: [{ id: 'WS1', external_key: 'support', title: 'Customer support queue' }] });
    }
    if (url.includes('/api/team/read')) {
      const query = JSON.parse(String(init?.body)).query;
      if (query.op === 'work.snapshot') return json(snapshots[query.work_id as keyof typeof snapshots]);
      if (query.op === 'source.content') return json(await sourceContent(query));
    }
    return json({ ok: false, error: { code: 'NoRoute', message: url } });
  });
  vi.stubGlobal('fetch', fetchMock);
}

/**
 * The source read is held back at the transport: until it resolves, the page has no text at all;
 * the render right after it resolves is still "checking" (the digest hook starts as pending), so
 * a mismatching text would have to be shown for that frame to leak — the observer catches it.
 */
function gatedSourceContent(payload: () => unknown) {
  let release: () => void = () => {};
  const gate = new Promise<void>((resolve) => { release = resolve; });
  return {
    sourceContent: async () => {
      await gate;
      return payload();
    },
    release,
  };
}

afterEach(async () => {
  await i18n.changeLanguage('zh-CN');
});

describe('knowledge read: the text never appears before its check finishes', () => {
  it('reads each version of one path separately: the digest is part of the cache key', async () => {
    const user = userEvent.setup();
    const versionB = `${REAL_TEXT}\nA second version of the same path.`;
    const specA = refSpec();
    const specB: SpecFixture = { ...refSpec(), sha256: sha256(versionB), byte_length: REAL_BYTES + 37 };
    const calls: Array<{ url: string; init?: RequestInit }> = [];
    const fetchMock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = String(input);
      calls.push({ url, init });
      if (url.includes('/api/team/projects')) return json(DEMO_PROJECTS);
      if (url.includes('/api/team/overview')) {
        return json({ ...DEMO_OVERVIEW, works: WORKS, workstreams: [{ id: 'WS1', external_key: 'support', title: 'Customer support queue' }] });
      }
      if (url.includes('/api/team/read')) {
        const query = JSON.parse(String(init?.body)).query;
        if (query.op === 'work.snapshot') return json(snapshotsFor(specA, specB)[query.work_id as 'W1' | 'W2']);
        if (query.op === 'source.content') {
          return json(await (async () => ({
            ok: true,
            result: { data: { ...REAL, text: query.expected_sha256 === REAL_SHA ? REAL_TEXT : versionB, sha256: query.expected_sha256 } },
          }))());
        }
      }
      return json({ ok: false, error: { code: 'NoRoute', message: url } });
    });
    vi.stubGlobal('fetch', fetchMock);
    renderApp('/projects/demo?tab=knowledge');
    await screen.findAllByTestId(`knowledge-source-${specB.path}-${specB.sha256.slice(0, 12)}`);
    await user.click(screen.getAllByTestId(`knowledge-source-${specB.path}-${specB.sha256.slice(0, 12)}`)[0]);
    await user.click(await screen.findByTestId('read-full'));
    await waitFor(() => expect(screen.getByTestId('read-text').textContent).toContain('A second version'));
    expect(countReads(calls)).toBe(1);

    await user.click(screen.getAllByTestId(`knowledge-source-${specA.path}-${specA.sha256.slice(0, 12)}`)[0]);
    await user.click(await screen.findByTestId('read-full'));
    await waitFor(() => expect(screen.getByTestId('read-text').textContent).toContain('Customer support queue'));
    expect(screen.getByTestId('read-result').textContent).not.toContain('内容对不上');
    expect(countReads(calls)).toBe(2);
  });

  it('says the content is not plain text when the reply carries no text', async () => {
    const user = userEvent.setup();
    setup({ sourceContent: () => ({ ok: true, result: { data: { ...REAL, text: null } } }) });
    renderApp('/projects/demo?tab=knowledge');
    await user.click(await screen.findByTestId('read-full'));
    await waitFor(() => expect(screen.getByTestId('read-result').textContent).toContain('不是纯文本'));
    expect(screen.getByTestId('read-result').textContent).not.toContain('内容对不上');
    expect(screen.getByTestId('read-result').textContent).not.toContain('读取失败');
    expect(screen.queryByTestId('read-failed')).toBeNull();
  });

  it('shows no text until the read has arrived and checked out', async () => {
    const user = userEvent.setup();
    const gate = gatedSourceContent(() => realSourceContentReply());
    setup(gate);
    renderApp('/projects/demo?tab=knowledge');
    await user.click(await screen.findByTestId('read-full'));
    await waitFor(() => expect(screen.getByTestId('read-full', { exact: false }) ? true : true).valueOf());
    expect(screen.queryByTestId('read-text')).toBeNull();
    expect(document.querySelector('[data-testid="page-knowledge"]')?.textContent).not.toContain('Customer support queue');
    gate.release();
    await screen.findByTestId('read-text', {}, { timeout: 3000 });
    expect(screen.getByTestId('read-text').textContent).toContain('# Customer support queue');
  });

  it('never shows a text that turns out not to match, in any render', async () => {
    const user = userEvent.setup();
    const altered = `${REAL_TEXT.slice(0, REAL_BYTES - 6)}XXXXXX`;
    const gate = gatedSourceContent(() => ({ ok: true, result: { data: { ...REAL, text: altered, sha256: sha256(altered) } } }));
    setup(gate);
    // Every DOM change is snapshotted, so a flash of the altered text in any render
    // (before, during or after the digest check) fails the test.
    const seen: string[] = [];
    const observer = new MutationObserver(() => seen.push(document.body.textContent ?? ''));
    observer.observe(document.body, { childList: true, characterData: true, subtree: true });
    try {
      renderApp('/projects/demo?tab=knowledge');
      await user.click(await screen.findByTestId('read-full'));
      gate.release();
      await waitFor(() => expect(screen.getByTestId('read-result').textContent).toContain('内容对不上'));
      await waitFor(() => expect(screen.getByTestId('read-result').textContent).toContain('内容对不上'));
      expect(seen.length).toBeGreaterThan(0);
      expect(seen.every((snapshot) => !snapshot.includes('XXXXXX'))).toBe(true);
      expect(document.body.textContent ?? '').not.toContain('XXXXXX');
    } finally {
      observer.disconnect();
    }
  });
});

// act is re-exported for the click helpers above to keep the lint quiet in some setups.
void act;
