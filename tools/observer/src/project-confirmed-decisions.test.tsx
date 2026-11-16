import { act, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';
import i18n from './i18n';
import { defaultVisibleText, findHumanLanguageViolations } from './lib/human-language-guard';
import type { OrdinaryDecision } from './lib/types';
import { DEMO_OVERVIEW, DEMO_PROJECTS, mockViewport, renderApp } from './test-utils';


const AT = 1793534400000;
const INTERNAL = '01M68M12NPA7JRA43BFT56VBYF';

function decision(overrides: Partial<OrdinaryDecision> = {}): OrdinaryDecision {
  return {
    decision_id: INTERNAL, work_id: INTERNAL, workstream_id: 'stream-private',
    work_title: 'Prepare the customer portal', question: 'Should we keep the agreed launch date?',
    impact: 'Support can finish training before launch.',
    reply: 'Decision: Keep the launch date.\nUse customer reference ACME-2026-42.',
    state: 'replied', baseline_matches_current: true,
    requester: { person_id: 'requester-private', display_name: 'Lin Chen' },
    allowed_responders: [{ person_id: 'responder-private', display_name: 'Maya Zhou' }],
    resolution: { person_id: 'responder-private', display_name: 'Maya Zhou', at_unix_ms: AT + 1000 },
    created_at_unix_ms: AT, ...overrides,
  };
}

function page(items = [decision()], cursor: string | null = null) {
  return { ok: true, result: { data: { items, next_cursor: cursor, truncated: cursor !== null,
    execution_authorized: false, as_of: { at_unix_ms: AT } } } };
}

function setup(respond: (query: Record<string, unknown>) => unknown = () => page()) {
  const calls: Array<Record<string, unknown>> = [];
  vi.stubGlobal('fetch', vi.fn(async (input, init) => {
    const url = String(input);
    let payload: unknown;
    if (url.includes('/api/team/projects')) payload = DEMO_PROJECTS;
    else if (url.includes('/api/team/overview')) payload = DEMO_OVERVIEW;
    else if (url.includes('/api/team/read')) {
      const query = JSON.parse(String(init?.body)).query;
      calls.push(query);
      payload = query.op === 'documents.list'
        ? { ok: true, result: { data: { items: [], total: 0, next_cursor: null, as_of: { at_unix_ms: AT } } } }
        : query.op === 'capabilities'
        ? { ok: false, error: { code: 'Forbidden', message: 'Project discovery is restricted' } }
        : await respond(query);
    } else payload = { ok: false, error: { code: 'NoRoute', message: 'Unavailable' } };
    return new Response(JSON.stringify(payload), { status: 200, headers: { 'content-type': 'application/json' } });
  }));
  return { ...renderApp('/projects/demo?tab=knowledge&knowledge=confirmations'), calls };
}

afterEach(async () => {
  vi.unstubAllGlobals();
  await act(async () => { await i18n.changeLanguage('zh-CN'); });
});

describe('project confirmed decisions', () => {
  it('preserves the recorded software-task name in its project question', async () => {
    setup(() => page([decision({ work_title: 'Add last_event_at to graph nodes' })]));
    const card = await screen.findByTestId('ordinary-question');
    expect(within(card).getByRole('link', { name: 'Add last_event_at to graph nodes' })).toBeTruthy();
  });
  it.each(['zh-CN', 'en-US'])('reads confirmed choices directly and preserves business content and attribution in %s', async (language) => {
    await i18n.changeLanguage(language);
    const { calls, router } = setup();
    const card = await screen.findByTestId('ordinary-question');
    const text = defaultVisibleText(card);
    for (const expected of ['Decision: Keep the launch date.', 'ACME-2026-42', 'Lin Chen', 'Maya Zhou', 'Prepare the customer portal']) {
      expect(text).toContain(expected);
    }
    expect(calls).toEqual([{ protocol_version: 1, op: 'decisions.list', decision_state: 'replied', limit: 10, max_context_bytes: 262144 }]);
    expect(router.state.location.search).toContain('knowledge=confirmations');
    expect(within(card).getByRole('link', { name: 'Prepare the customer portal' }).getAttribute('href')).toContain('?tab=plan&work=');
    expect(within(card).queryAllByRole('button')).toHaveLength(0);
    expect(within(screen.getByTestId('project-questions')).queryAllByRole('tab')).toHaveLength(0);
    expect(card.querySelector('details,[aria-expanded]')).toBeNull();
    expect(findHumanLanguageViolations(card)).toEqual([]);
    for (const internal of [INTERNAL, 'stream-private', 'requester-private', 'responder-private']) expect(text).not.toContain(internal);
  });

  it('lets a task-scoped reader switch knowledge views without project-wide discovery', async () => {
    await i18n.changeLanguage('en-US');
    const { calls } = setup();
    await screen.findByTestId('ordinary-question');
    await userEvent.click(screen.getByRole('tab', { name: 'Documents' }));
    await waitFor(() => expect(calls.some((query) => query.op === 'documents.list')).toBe(true));
    await userEvent.click(screen.getByRole('tab', { name: 'Confirmed decisions' }));
    await screen.findByTestId('ordinary-question');
    expect(screen.queryByTestId('state-forbidden')).toBeNull();
    expect(calls.filter((query) => query.op === 'decisions.list').every((query) => query.decision_state === 'replied')).toBe(true);
    expect(calls.some((query) => query.op === 'capabilities')).toBe(false);
  });

  it('keeps the confirmed-only selection while paging and refreshing from the beginning', async () => {
    await i18n.changeLanguage('en-US');
    const { calls } = setup((query) => query.cursor
      ? page([decision({ question: 'Should support receive the approved training schedule?' })]) : page([decision()], 'next-confirmations'));
    await userEvent.click(await screen.findByRole('button', { name: 'More decisions' }));
    await screen.findByText('Should support receive the approved training schedule?');
    await userEvent.click(screen.getByRole('button', { name: 'Refresh decisions' }));
    await screen.findByText('Should we keep the agreed launch date?');
    expect(calls.every((query) => query.decision_state === 'replied')).toBe(true);
    expect(calls.some((query) => query.cursor === 'next-confirmations')).toBe(true);
    expect(calls.at(-1)?.cursor).toBeUndefined();
  });

  it.each(['zh-CN', 'en-US'])('marks a historical choice for renewed checking when its work changes in %s', async (language) => {
    await i18n.changeLanguage(language);
    setup(() => page([decision({ baseline_matches_current: false })]));
    const card = await screen.findByTestId('ordinary-question');
    expect(defaultVisibleText(card)).toContain(i18n.t('inbox.status.changed'));
    expect(defaultVisibleText(card)).toContain(i18n.t('inbox.changed'));
    expect(defaultVisibleText(card)).toContain('Decision: Keep the launch date.');
  });

  it.each([
    ['Unsupported', 'knowledge.confirmationsUnavailable'],
    ['InternalError', 'knowledge.confirmationsFailed'],
  ] as const)('distinguishes %s from a successfully empty decision history', async (code, label) => {
    await i18n.changeLanguage('en-US');
    setup(() => ({ ok: false, error: { code, message: 'private-service-diagnostic' } }));
    await screen.findByText(i18n.t(label));
    expect(screen.queryByText(i18n.t('knowledge.confirmationsEmpty'))).toBeNull();
    expect(document.body.textContent).not.toContain('private-service-diagnostic');
  });

  it('shows an empty confirmed history without requesting unconfirmed work', async () => {
    await i18n.changeLanguage('en-US');
    const { calls } = setup(() => page([]));
    await screen.findByText('No confirmed decisions recorded yet');
    expect(calls).toHaveLength(1);
    expect(calls[0].decision_state).toBe('replied');
  });

  it('hides a previously visible confirmation when its authorization is revoked', async () => {
    await i18n.changeLanguage('en-US');
    let forbidden = false;
    const { queryClient } = setup(() => forbidden
      ? { ok: false, error: { code: 'Forbidden', message: 'private-access-change' } } : page());
    await screen.findByTestId('ordinary-question');
    forbidden = true;
    await act(async () => { await queryClient.refetchQueries({ queryKey: ['team', 'decisions.list'] }); });
    await screen.findByTestId('state-forbidden');
    expect(screen.queryByTestId('ordinary-question')).toBeNull();
    expect(document.body.textContent).not.toContain('private-access-change');
  });

  it('keeps the same business record and responsive card on a phone', async () => {
    await i18n.changeLanguage('en-US');
    const viewport = mockViewport(390);
    try {
      setup();
      const card = await screen.findByTestId('ordinary-question');
      expect(defaultVisibleText(card)).toContain('Decision: Keep the launch date.');
      expect((card.querySelector('.ant-card-head-title') as HTMLElement).style.whiteSpace).toBe('normal');
      expect(findHumanLanguageViolations(card)).toEqual([]);
    } finally { viewport.restore(); }
  });
});
