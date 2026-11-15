import { act, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { createHash, webcrypto } from 'node:crypto';
import i18n from './i18n';
import { DEMO_OVERVIEW, DEMO_PROJECTS, renderApp } from './test-utils';

const AT = 1793534400000;
const PRIVATE_DOCUMENT = 'Confidential customer launch guide';
const PRIVATE_CHOICE = 'Confidential rollout choice';
const PRIVATE_BODY = '# Launch guide\n\nConfidential customer rollout instructions.';
const BODY_DIGEST = createHash('sha256').update(PRIVATE_BODY).digest('hex');

function documents() {
  return { ok: true, result: { data: { items: [{ document_position: 1, path: 'docs/launch.md',
    name: PRIVATE_DOCUMENT, summary: null, summary_kind: 'source_excerpt', sha256: BODY_DIGEST,
    byte_length: Buffer.byteLength(PRIVATE_BODY), source_kind: 'registered_source', readable: true, status: 'available', reason: null }],
    total: 1, next_cursor: null, truncated: false, as_of: { at_unix_ms: AT } } } };
}

function confirmations() {
  return { ok: true, result: { data: { items: [{ decision_id: 'choice-one', work_id: 'launch',
    workstream_id: 'delivery', work_title: 'Customer launch', question: PRIVATE_CHOICE,
    impact: 'Support needs the rollout dates.', reply: 'Keep the agreed date.', state: 'replied',
    baseline_matches_current: true, requester: { person_id: 'lin', display_name: 'Lin Chen' },
    allowed_responders: [{ person_id: 'maya', display_name: 'Maya Zhou' }],
    resolution: { person_id: 'maya', display_name: 'Maya Zhou', at_unix_ms: AT + 1000 },
    created_at_unix_ms: AT }], next_cursor: null, truncated: false,
    execution_authorized: false, as_of: { at_unix_ms: AT } } } };
}

afterEach(async () => {
  vi.unstubAllGlobals();
  await act(async () => { await i18n.changeLanguage('zh-CN'); });
});

describe('knowledge-view authorization on remount', () => {
  it.each(['library', 'legacy'] as const)('rechecks an explicit full-text read in the %s view before showing cached text', async (view) => {
    await i18n.changeLanguage('en-US');
    vi.stubGlobal('crypto', webcrypto);
    let rechecking = false;
    let finish!: (value: unknown) => void;
    const pending = new Promise((resolve) => { finish = resolve; });
    const bodyReads: Array<Record<string, unknown>> = [];
    vi.stubGlobal('fetch', vi.fn(async (input, init) => {
      const url = String(input);
      let payload: unknown;
      if (url.includes('/api/team/projects')) payload = DEMO_PROJECTS;
      else if (url.includes('/api/team/overview')) payload = {
        ...DEMO_OVERVIEW,
        works: [{ key: 'launch', title: 'Customer launch', status: 'unknown', owner_person: null,
          agent: null, next_step: null, detail_loaded: false, workstream_id: 'delivery', contract_hash: 'launch-contract' }],
      };
      else {
        const query = JSON.parse(String(init?.body)).query;
        if (query.op === 'documents.list') {
          const catalog = documents();
          Object.assign(catalog.result.data.items[0], { content_work_id: 'launch' });
          payload = view === 'legacy'
            ? { ok: false, error: { code: 'Unsupported', message: 'Document discovery unavailable' } }
            : catalog;
        } else if (query.op === 'work.snapshot') payload = { ok: true, result: { data: {
          work_id: 'launch', required_specs: [], visible_contract: { goals: [], acceptance: [] },
          authorized_readable_refs: [{ kind: 'source', path: 'docs/launch.md', sha256: BODY_DIGEST,
            byte_length: Buffer.byteLength(PRIVATE_BODY) }],
        } } };
        else if (query.op === 'source.content') {
          bodyReads.push(query);
          payload = rechecking ? await pending : { ok: true, result: { data: {
            text: PRIVATE_BODY, sha256: BODY_DIGEST, byte_length: Buffer.byteLength(PRIVATE_BODY),
          } } };
        } else payload = confirmations();
      }
      return new Response(JSON.stringify(payload), { headers: { 'content-type': 'application/json' } });
    }));
    renderApp('/projects/demo?tab=knowledge&knowledge=documents');
    await userEvent.click(await screen.findByRole('button', { name: 'Read the full text' }));
    const textId = view === 'legacy' ? 'read-text' : 'library-document-text';
    expect((await screen.findByTestId(textId)).textContent).toBe(PRIVATE_BODY);
    expect(bodyReads).toHaveLength(1);
    if (view === 'library') expect(bodyReads[0]?.work_id).toBe('launch');
    await userEvent.click(screen.getByRole('tab', { name: 'Confirmed decisions' }));
    await waitFor(() => expect(document.body.textContent).not.toContain(PRIVATE_BODY));
    rechecking = true;
    await userEvent.click(screen.getByRole('tab', { name: 'Documents' }));
    const exposures: string[] = [];
    const observer = new MutationObserver(() => exposures.push(document.querySelector('main')?.textContent ?? ''));
    observer.observe(document.body, { subtree: true, childList: true, characterData: true });
    try {
      await userEvent.click(await screen.findByRole('button', { name: 'Read the full text' }));
      await waitFor(() => expect(bodyReads).toHaveLength(2));
      expect(document.body.textContent).not.toContain(PRIVATE_BODY);
      expect(exposures.every((text) => !text.includes(PRIVATE_BODY))).toBe(true);
      await act(async () => { finish({ ok: false, error: { code: 'Forbidden', message: 'private-body-permission-change' } }); });
      await screen.findByTestId('state-forbidden');
      expect(document.body.textContent).not.toContain(PRIVATE_BODY);
      expect(document.body.textContent).not.toContain('private-body-permission-change');
    } finally { observer.disconnect(); }
  });

  it.each([
    ['documents', 'documents.list', PRIVATE_DOCUMENT, 'Confirmed decisions', 'Documents'],
    ['confirmations', 'decisions.list', PRIVATE_CHOICE, 'Documents', 'Confirmed decisions'],
  ] as const)('waits for a fresh %s read before displaying retained data after a hidden tab reopens', async (view, operation, privateText, otherTab, ownTab) => {
    await i18n.changeLanguage('en-US');
    let rechecking = false;
    let finish!: (value: unknown) => void;
    const pending = new Promise((resolve) => { finish = resolve; });
    const calls: string[] = [];
    vi.stubGlobal('fetch', vi.fn(async (input, init) => {
      const url = String(input);
      let payload: unknown;
      if (url.includes('/api/team/projects')) payload = DEMO_PROJECTS;
      else if (url.includes('/api/team/overview')) payload = DEMO_OVERVIEW;
      else {
        const query = JSON.parse(String(init?.body)).query;
        calls.push(query.op);
        payload = rechecking && query.op === operation ? await pending
          : query.op === 'documents.list' ? documents() : confirmations();
      }
      return new Response(JSON.stringify(payload), { headers: { 'content-type': 'application/json' } });
    }));
    const { queryClient } = renderApp(`/projects/demo?tab=knowledge&knowledge=${view}`);
    await screen.findAllByText(privateText);
    queryClient.setQueryDefaults(['team', operation], { staleTime: Infinity });
    await userEvent.click(screen.getByRole('tab', { name: otherTab }));
    await waitFor(() => expect(document.body.textContent).not.toContain(privateText));
    expect(queryClient.getQueriesData({ queryKey: ['team', operation] }).some(([, data]) => data)).toBe(true);
    rechecking = true;
    const exposures: string[] = [];
    const observer = new MutationObserver(() => exposures.push(document.querySelector('main')?.textContent ?? ''));
    observer.observe(document.body, { subtree: true, childList: true, characterData: true });
    try {
      await userEvent.click(screen.getByRole('tab', { name: ownTab }));
      await waitFor(() => expect(calls.filter((op) => op === operation).length).toBeGreaterThan(1));
      expect(document.body.textContent).not.toContain(privateText);
      expect(exposures.every((text) => !text.includes(privateText))).toBe(true);
      await act(async () => { finish({ ok: false, error: { code: 'Forbidden', message: 'private-permission-change' } }); });
      await screen.findByTestId('state-forbidden');
      expect(document.body.textContent).not.toContain(privateText);
      expect(document.body.textContent).not.toContain('private-permission-change');
    } finally { observer.disconnect(); }
  });
});
