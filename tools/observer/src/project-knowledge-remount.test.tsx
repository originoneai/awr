import { act, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';
import i18n from './i18n';
import { DEMO_OVERVIEW, DEMO_PROJECTS, renderApp } from './test-utils';

const AT = 1793534400000;
const PRIVATE_DOCUMENT = 'Confidential customer launch guide';
const PRIVATE_CHOICE = 'Confidential rollout choice';

function documents() {
  return { ok: true, result: { data: { items: [{ document_position: 1, path: 'docs/launch.md',
    name: PRIVATE_DOCUMENT, summary: null, summary_kind: 'source_excerpt', sha256: 'b'.repeat(64),
    byte_length: 100, source_kind: 'registered_source', readable: true, status: 'available', reason: null }],
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
