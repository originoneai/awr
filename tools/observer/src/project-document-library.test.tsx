import { createHash, webcrypto } from 'node:crypto';
import { QueryClientProvider } from '@tanstack/react-query';
import { act, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';
import i18n from './i18n';
import { defaultVisibleText, findForbiddenVisibleText } from './lib/human-language-guard';
import { createAppQueryClient } from './lib/queries';
import { RevocationProvider } from './lib/revocation';
import { ProjectKnowledge } from './pages/ProjectKnowledge';
import type { ProjectDocument } from './lib/types';

const TEXT = '# Delivery guide\n\nCheck the outcome before sharing it.';
const DIGEST = createHash('sha256').update(TEXT).digest('hex');
const AT = 1793534400000;

function document(position = 1, overrides: Partial<ProjectDocument> = {}): ProjectDocument {
  return { document_position: position, path: 'docs/delivery-guide.md', name: 'Delivery guide',
    summary: 'Check the outcome before sharing it.', summary_kind: 'source_excerpt',
    sha256: DIGEST, byte_length: Buffer.byteLength(TEXT), source_kind: 'registered_source',
    readable: true, status: 'available', reason: null, ...overrides };
}

function catalog(items = [document()], total = items.length, nextCursor: string | null = null) {
  return { ok: true, result: { data: { work_id: null, items, total, next_cursor: nextCursor,
    truncated: nextCursor !== null, read_only: true, execution_authorized: false,
    scope: 'authorized_workstreams', content_read_limit_bytes: 262144,
    as_of: { at_unix_ms: AT, project_revision: '12', source_snapshot_id: 'private-snapshot' } } } };
}

function setup(respond: (query: Record<string, unknown>) => unknown = () => catalog(),
  discovery: () => unknown = () => ({ ok: true, result: { queries: ['documents.list'] } })) {
  const calls: Array<Record<string, unknown>> = [];
  vi.stubGlobal('crypto', webcrypto);
  vi.stubGlobal('fetch', vi.fn(async (_url, init) => {
    const query = JSON.parse(init.body).query;
    calls.push(query);
    const payload = query.op === 'capabilities'
      ? discovery()
      : respond(query);
    return new Response(JSON.stringify(payload), { status: 200, headers: { 'content-type': 'application/json' } });
  }));
  const client = createAppQueryClient();
  client.setDefaultOptions({ queries: { retry: false } });
  render(<QueryClientProvider client={client}><RevocationProvider>
    <ProjectKnowledge project="portal" />
  </RevocationProvider></QueryClientProvider>);
  return { calls, client };
}

afterEach(() => vi.unstubAllGlobals());

describe('project document library', () => {
  it('reports discovery failures and retries without issuing work reads', async () => {
    await i18n.changeLanguage('en-US');
    let failed = true;
    const { calls } = setup(() => catalog(), () => failed
      ? { ok: false, error: { code: 'InternalError', message: 'private-discovery-error' } }
      : { ok: true, result: { queries: ['documents.list'] } });
    await screen.findByText(i18n.t('knowledge.libraryFailed'));
    expect(calls.length).toBeGreaterThan(0);
    expect(calls.every((query) => query.op === 'capabilities')).toBe(true);
    expect(globalThis.document.body.textContent).not.toContain('private-discovery-error');
    failed = false;
    await userEvent.click(screen.getByRole('button', { name: 'Retry' }));
    await screen.findByRole('button', { name: /Delivery guide/ });
    expect(calls.some((query) => query.op === 'work.snapshot')).toBe(false);
  });
  it.each(['zh-CN', 'en-US'])('discovers the project without per-work reads and keeps technical metadata off the %s interface', async (language) => {
    await i18n.changeLanguage(language);
    const { calls } = setup();
    const page = await screen.findByTestId('project-document-library');
    await screen.findByRole('button', { name: /Delivery guide/ });
    expect(calls.map((query) => query.op)).toEqual(['capabilities', 'documents.list']);
    expect(calls.some((query) => 'work_id' in query)).toBe(false);
    const visible = defaultVisibleText(page);
    expect(findForbiddenVisibleText(visible)).toEqual([]);
    for (const value of [DIGEST, 'docs/delivery-guide.md', 'private-snapshot', 'source_excerpt', 'registered_source']) {
      expect(visible).not.toContain(value);
      expect(page.innerHTML).not.toContain(`title="${value}"`);
    }
    expect(screen.queryByTestId('library-document-text')).toBeNull();
  });

  it('reads the chosen document on request and verifies content before displaying it', async () => {
    await i18n.changeLanguage('en-US');
    const { calls } = setup((query) => query.op === 'source.content'
      ? { ok: true, result: { data: { text: TEXT, sha256: DIGEST, byte_length: Buffer.byteLength(TEXT) } } }
      : catalog());
    await userEvent.click(await screen.findByRole('button', { name: 'Read the full text' }));
    expect((await screen.findByTestId('library-document-text')).textContent).toBe(TEXT);
    expect(screen.getByText('Content unchanged')).not.toBeNull();
    expect(calls.filter((query) => query.op === 'source.content')).toEqual([
      { protocol_version: 1, op: 'source.content', source_path: 'docs/delivery-guide.md', expected_sha256: DIGEST },
    ]);
  });

  it('keeps changed text off the page and describes the failure in product language', async () => {
    await i18n.changeLanguage('en-US');
    setup((query) => query.op === 'source.content'
      ? { ok: true, result: { data: { text: 'Unverified private replacement' } } } : catalog());
    await userEvent.click(await screen.findByRole('button', { name: 'Read the full text' }));
    await screen.findByText('Content does not match; do not rely on it');
    expect(screen.queryByTestId('library-document-text')).toBeNull();
    expect(globalThis.document.body.textContent).not.toContain('Unverified private replacement');
  });

  it('searches across the project, navigates pages and returns to the first page after refresh', async () => {
    await i18n.changeLanguage('en-US');
    const { calls } = setup((query) => {
      if (query.search) return catalog([document(1, { name: 'Release guide' })]);
      return query.cursor ? catalog([document(21, { name: 'Support guide' })], 21)
        : catalog(Array.from({ length: 20 }, (_, i) => document(i + 1, { name: `Delivery guide ${i + 1}` })), 21, 'private-next-page');
    });
    await userEvent.click(await screen.findByRole('button', { name: 'Next' }));
    await screen.findByRole('button', { name: /Support guide/ });
    expect(calls.at(-1)?.cursor).toBe('private-next-page');
    await userEvent.click(screen.getByRole('button', { name: 'Refresh documents' }));
    await screen.findByRole('button', { name: /Delivery guide 1 / });
    const search = screen.getByRole('searchbox', { name: 'Search project documents' });
    await userEvent.type(search, 'Release{Enter}');
    await screen.findByRole('button', { name: /Release guide/ });
    expect(calls.at(-1)?.search).toBe('Release');
    expect(calls.at(-1)).not.toHaveProperty('cursor');
  });

  it.each(['zh-CN', 'en-US'].flatMap((language) =>
    (['missing', 'restricted', 'changed', 'unverified', 'too_large'] as const).map((status) => ({ language, status })),
  ))('explains $status documents in $language without offering a body read', async ({ language, status }) => {
    await i18n.changeLanguage(language);
    const unavailable = status === 'too_large'
      ? { byte_length: 262145 }
      : { name: null, summary: null, path: status === 'restricted' ? null : 'docs/guide.md', sha256: null, byte_length: null };
    const { calls } = setup(() => catalog([document(1, { ...unavailable,
      readable: false, status, reason: 'internal-reason' })]));
    const page = await screen.findByTestId('project-document-library');
    await screen.findAllByText(i18n.t(`knowledge.documentStatus.${status}`));
    expect(screen.queryByRole('button', { name: i18n.t('knowledge.readFull') })).toBeNull();
    expect(page.textContent).not.toContain('internal-reason');
    expect(findForbiddenVisibleText(defaultVisibleText(page))).toEqual([]);
    expect(calls.some((query) => query.op === 'source.content')).toBe(false);
  });

  it.each(['zh-CN', 'en-US'])('distinguishes empty, failed and recovered catalog reads in %s', async (language) => {
    await i18n.changeLanguage(language);
    let failed = false;
    const { client } = setup(() => failed
      ? { ok: false, error: { code: 'InternalError', message: 'private-query-diagnostics' } }
      : catalog([]));
    await screen.findByTestId('state-empty');
    failed = true;
    await act(async () => { await client.refetchQueries({ queryKey: ['team', 'documents.list'] }); });
    await screen.findByText(i18n.t('knowledge.libraryFailed'));
    expect(globalThis.document.body.textContent).not.toContain('private-query-diagnostics');
    failed = false;
    await userEvent.click(screen.getByRole('button', { name: i18n.t('knowledge.refreshLibrary') }));
    await waitFor(() => expect(screen.queryByText(i18n.t('knowledge.libraryFailed'))).toBeNull());
    expect(screen.getByTestId('state-empty')).not.toBeNull();
  });

  it('clears the catalog and opened body when access is revoked', async () => {
    await i18n.changeLanguage('en-US');
    let forbidden = false;
    const { client } = setup((query) => forbidden
      ? { ok: false, error: { code: 'Forbidden', message: 'Internal access decision' } }
      : query.op === 'source.content' ? { ok: true, result: { data: { text: TEXT } } } : catalog());
    await userEvent.click(await screen.findByRole('button', { name: 'Read the full text' }));
    await screen.findByTestId('library-document-text');
    forbidden = true;
    await act(async () => { await client.refetchQueries({ queryKey: ['team', 'documents.list'] }); });
    await screen.findByTestId('state-forbidden');
    expect(screen.queryByTestId('library-document-text')).toBeNull();
    expect(globalThis.document.body.textContent).not.toContain('Delivery guide');
    expect(globalThis.document.body.textContent).not.toContain('Internal access decision');
  });
});
