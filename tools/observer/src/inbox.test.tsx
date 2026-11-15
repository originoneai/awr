import { act, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';
import i18n from './i18n';
import { defaultVisibleText, findHumanLanguageViolations } from './lib/human-language-guard';
import type { OrdinaryDecision } from './lib/types';
import { DEMO_OVERVIEW, DEMO_PROJECTS, mockViewport, renderApp } from './test-utils';

const AT = 1793534400000;
const INTERNAL = '01M68M12NPA7JRA43BFT56VBYF';

function question(overrides: Partial<OrdinaryDecision> = {}): OrdinaryDecision {
  return {
    decision_id: INTERNAL, work_id: INTERNAL, workstream_id: 'stream-private',
    work_title: 'Prepare the customer portal', question: 'Should the portal use the blue theme?',
    impact: 'The answer lets the design work continue.', reply: null, state: 'open', baseline_matches_current: true,
    requester: { person_id: 'person-private', display_name: 'Lin Chen' },
    allowed_responders: [{ person_id: 'responder-private', display_name: 'Maya Zhou' }], resolution: null,
    created_at_unix_ms: AT, ...overrides,
  };
}

function page(items = [question()], cursor: string | null = null) {
  return { ok: true, result: { data: { items, next_cursor: cursor, truncated: cursor !== null,
    execution_authorized: false, as_of: { at_unix_ms: AT } } } };
}

function setup(respond: (query: Record<string, unknown>) => unknown = () => page(),
  discovery: () => unknown = () => ({ ok: true, result: { queries: ['decisions.list'] } })) {
  const calls: Array<Record<string, unknown>> = [];
  vi.stubGlobal('fetch', vi.fn(async (input, init) => {
    const url = String(input);
    let payload: unknown;
    if (url.includes('/api/team/projects')) payload = DEMO_PROJECTS;
    else if (url.includes('/api/team/overview')) payload = DEMO_OVERVIEW;
    else if (url.includes('/api/team/read')) {
      const query = JSON.parse(init.body).query;
      calls.push(query);
      payload = await (query.op === 'capabilities' ? discovery() : respond(query));
    } else payload = { ok: false, error: { code: 'NoRoute', message: 'Not available' } };
    return new Response(JSON.stringify(payload), { status: 200, headers: { 'content-type': 'application/json' } });
  }));
  return { ...renderApp('/inbox?scope=demo'), calls };
}

afterEach(async () => {
  vi.unstubAllGlobals();
  await act(async () => { await i18n.changeLanguage('zh-CN'); });
});

describe('ordinary project questions', () => {
  it.each(['zh-CN', 'en-US'])('shows questions, impact and real participant names in %s with no action or diagnostic controls', async (language) => {
    await i18n.changeLanguage(language);
    const { calls } = setup();
    const card = await screen.findByTestId('ordinary-question');
    expect(defaultVisibleText(card)).toContain('Should the portal use the blue theme?');
    expect(defaultVisibleText(card)).toContain('The answer lets the design work continue.');
    expect(defaultVisibleText(card)).toContain('Lin Chen');
    expect(defaultVisibleText(card)).toContain('Maya Zhou');
    expect(defaultVisibleText(card)).toContain(i18n.t('inbox.responders'));
    expect(defaultVisibleText(card)).not.toContain('Waiting for');
    expect(within(card).queryAllByRole('button')).toHaveLength(0);
    expect(card.querySelector('details,[aria-expanded]')).toBeNull();
    expect(findHumanLanguageViolations(card)).toEqual([]);
    for (const internal of [INTERNAL, 'stream-private', 'person-private', 'responder-private']) {
      expect(defaultVisibleText(card)).not.toContain(internal);
      expect(card.querySelector(`[title="${internal}"],[aria-label="${internal}"]`)).toBeNull();
    }
    expect(calls.map((query) => query.op)).toEqual(['decisions.list']);
    expect(calls[0]).toEqual({ protocol_version: 1, op: 'decisions.list', decision_state: 'open', limit: 10, max_context_bytes: 262144 });
  });

  it.each(['zh-CN', 'en-US'])('keeps internal diagnostics out of question text and names in %s', async (language) => {
    await i18n.changeLanguage(language);
    setup(() => page([question({ question: 'status=claimed client=client_obs_658 path=/var/lib/private',
      impact: '/Users/private/project/secret-note', work_title: 'client_obs_658',
      requester: { person_id: INTERNAL, display_name: 'client_obs_658' },
      allowed_responders: [{ person_id: INTERNAL, display_name: 'status claimed actor client_obs_658' }],
      state: 'replied', reply: 'source_snapshot_id=private-snapshot reason=Forbidden',
      resolution: { person_id: INTERNAL, display_name: INTERNAL, at_unix_ms: AT + 1000 },
    })]));
    const card = await screen.findByTestId('ordinary-question');
    const text = defaultVisibleText(card);
    for (const internal of ['claimed', 'client_obs_658', '/var/lib', '/Users/private', 'source_snapshot_id', 'Forbidden', INTERNAL]) {
      expect(text).not.toContain(internal);
      expect([...card.querySelectorAll('[title],[aria-label],[alt]')].map((element) => element.outerHTML).join('')).not.toContain(internal);
    }
    expect(findHumanLanguageViolations(card)).toEqual([]);
    expect(text).toContain(i18n.t('inbox.questionMissing'));
    expect(text).toContain(i18n.t('inbox.nameMissing'));
  });

  it('preserves multiline business descriptions and long questions on mobile', async () => {
    await i18n.changeLanguage('en-US');
    const viewport = mockViewport(390);
    try {
      setup(() => page([question({ impact: 'Keep the agreed launch date.\nAllow support to prepare training.',
        question: 'Should we keep the current design so the support team can finish preparing the customer launch?' })]));
      const card = await screen.findByTestId('ordinary-question');
      expect(defaultVisibleText(card)).toContain('Allow support to prepare training.');
      expect((card.querySelector('.ant-card-head-title') as HTMLElement).style.whiteSpace).toBe('normal');
      expect(findHumanLanguageViolations(card)).toEqual([]);
    } finally { viewport.restore(); }
  });

  it('shows only a confirmed recorded answer and its actual responder, with no approval action', async () => {
    await i18n.changeLanguage('en-US');
    setup((query) => query.decision_state === 'open' ? page([]) : page([question({ state: 'replied',
      reply: 'Use the blue theme and keep the agreed launch date.',
      resolution: { person_id: 'maya-private', display_name: 'Maya Zhou', at_unix_ms: AT + 1000 } })]));
    await screen.findByText('No questions awaiting an answer');
    await userEvent.click(screen.getByRole('tab', { name: 'All questions' }));
    const card = await screen.findByTestId('ordinary-question');
    expect(defaultVisibleText(card)).toContain('Use the blue theme and keep the agreed launch date.');
    expect(defaultVisibleText(card)).toContain('Answer recorded');
    expect(defaultVisibleText(card)).toContain('Answered by');
    expect(defaultVisibleText(card)).not.toContain('Approved');
    expect(within(card).queryAllByRole('button')).toHaveLength(0);
  });

  it.each(['open', 'replied'] as const)('marks a changed project baseline for a %s question', async (state) => {
    await i18n.changeLanguage('en-US');
    setup(() => page([question({ state, baseline_matches_current: false,
      reply: state === 'replied' ? 'Keep the blue theme.' : null,
      resolution: state === 'replied' ? { person_id: 'maya', display_name: 'Maya Zhou', at_unix_ms: AT + 1000 } : null })]));
    const card = await screen.findByTestId('ordinary-question');
    expect(defaultVisibleText(card)).toContain('Needs another look');
    expect(defaultVisibleText(card)).toContain('The work has changed.');
  });

  it('retains withdrawal reason and attribution without labeling the question approved', async () => {
    await i18n.changeLanguage('en-US');
    setup(() => page([question({ state: 'cancelled', baseline_matches_current: false, reply: 'The launch was postponed.',
      resolution: { person_id: 'maya', display_name: 'Maya Zhou', at_unix_ms: AT + 1000 } })]));
    const card = await screen.findByTestId('ordinary-question');
    expect(defaultVisibleText(card)).toContain('Withdrawn');
    expect(defaultVisibleText(card)).toContain('The launch was postponed.');
    expect(defaultVisibleText(card)).not.toContain('Needs another look');
  });

  it('distinguishes an older service from a successfully empty inbox', async () => {
    await i18n.changeLanguage('en-US');
    const { calls } = setup(() => ({ ok: false, error: { code: 'Unsupported', message: 'private-protocol-version' } }));
    await screen.findByText('Questions are not enabled for this project yet');
    expect(screen.queryByText('No questions awaiting an answer')).toBeNull();
    expect(calls).toHaveLength(1);
    expect(calls[0].op).toBe('decisions.list');
    expect(screen.getByRole('link', { name: 'Open the workbench' }).getAttribute('href')).toBe('/workbench?scope=demo&tab=needs-me');
  });

  it('retries failed question reads without showing raw service messages', async () => {
    await i18n.changeLanguage('en-US');
    let failed = true;
    const { calls } = setup(() => failed
      ? { ok: false, error: { code: 'InternalError', message: 'private-discovery-diagnostics' } }
      : page());
    await screen.findByText('Questions cannot be loaded right now');
    expect(calls.every((query) => query.op === 'decisions.list')).toBe(true);
    failed = false;
    await userEvent.click(screen.getByRole('button', { name: 'Try again' }));
    await screen.findByTestId('ordinary-question');
    expect(document.body.textContent).not.toContain('private-discovery-diagnostics');
  });

  it('reads authorized questions when project-wide capability discovery would be forbidden', async () => {
    await i18n.changeLanguage('en-US');
    const { calls } = setup(() => page(), () => ({ ok: false, error: { code: 'Forbidden', message: 'Task scope only' } }));
    await screen.findByTestId('ordinary-question');
    expect(calls.map((query) => query.op)).toEqual(['decisions.list']);
    expect(screen.queryByTestId('state-forbidden')).toBeNull();
  });

  it.each(['zh-CN', 'en-US'])('preserves business punctuation, measurements and external references in %s', async (language) => {
    await i18n.changeLanguage(language);
    const content = 'Decision: Keep the blue theme.\n客户编号 ACME-2026-42，目标 P95 低于 200 ms。\nSee https://example.com/customer/ACME-2026-42 for the agreed plan.';
    setup(() => page([question({ question: 'Decision: Which launch plan should we use?', impact: content,
      state: 'replied', reply: content, resolution: { person_id: 'maya', display_name: 'Maya Zhou', at_unix_ms: AT + 1000 } })]));
    const card = await screen.findByTestId('ordinary-question');
    expect(defaultVisibleText(card)).toContain('Decision: Which launch plan should we use?');
    expect(defaultVisibleText(card).split(content)).toHaveLength(3);
    expect(defaultVisibleText(card)).not.toContain(i18n.t('inbox.replyMissing'));
    expect(defaultVisibleText(card)).not.toContain(i18n.t('inbox.impactMissing'));
    expect(defaultVisibleText(card)).toContain(i18n.t('inbox.answeredBy'));
  });

  it.each(['zh-CN', 'en-US'])('preserves business content mixed with diagnostics in %s', async (language) => {
    await i18n.changeLanguage(language);
    setup(() => page([question({ impact: 'Decision: Keep the launch date. source_snapshot_id=private-snapshot path=/var/lib/private' })]));
    const card = await screen.findByTestId('ordinary-question');
    expect(defaultVisibleText(card)).toContain('Decision: Keep the launch date.');
    expect(defaultVisibleText(card)).not.toContain(language.startsWith('zh') ? '相关记录' : 'the related record');
    for (const internal of ['source_snapshot_id', 'private-snapshot', '/var/lib/private', 'client_obs_658']) {
      expect(defaultVisibleText(card)).not.toContain(internal);
    }
    expect(findHumanLanguageViolations(card)).toEqual([]);
  });

  it.each(['zh-CN', 'en-US'])('removes internal URLs and colon-form diagnostics while retaining business reasons in %s', async (language) => {
    await i18n.changeLanguage(language);
    setup(() => page([question({ impact: 'Decision: Keep the launch date. reason: customer deadline.\nStatus: claimed. reason: Forbidden Actor: client_obs_658\nSESSION_ID：PRIVATE-900\nHTTP://127.0.0.1:7381/work?session_id=PRIVATE-123\nhttp://localhost.:7381/work\nhttp://[::1]:7381/work\nhttps://example.com/plan?customer=ACME-2026-42&SOURCE_SNAPSHOT_ID=PRIVATE-456#session_id=PRIVATE-789' })]));
    const card = await screen.findByTestId('ordinary-question');
    const visible = defaultVisibleText(card);
    expect(visible).toContain('Decision: Keep the launch date. reason: customer deadline.');
    expect(visible).toContain('https://example.com/plan?customer=ACME-2026-42');
    for (const internal of ['127.0.0.1', 'localhost', '[::1]', 'PRIVATE-', 'session_id', 'SOURCE_SNAPSHOT_ID', 'Status:', 'reason: Forbidden', 'Actor:', 'claimed', 'client_obs_658']) {
      expect(visible).not.toContain(internal);
    }
  });

  it.each([
    ['https://example.com/customer/client_obs_658', 'client_obs_658'],
    ['https://example.com/?customer=client_obs_658', 'client_obs_658'],
    ['https://example.com/customer/01M7F3XWDGSJFQJKFRAQ8T82CS', '01M7F3XWDGSJFQJKFRAQ8T82CS'],
    ['https://example.com/?customer_id=4bd70c66-41a8-45ec-803b-b5ee3e8c0d91', '4bd70c66-41a8-45ec-803b-b5ee3e8c0d91'],
    [`https://example.com/?customer=${'a'.repeat(64)}`, 'a'.repeat(64)],
    ['https://example.com/customer/%63lient_obs_658', 'client_obs_658'],
    ['https://client-658.example.com/plan', 'client-658'],
    ['https://example.com/plan#4bd70c66-41a8-45ec-803b-b5ee3e8c0d91', '4bd70c66-41a8-45ec-803b-b5ee3e8c0d91'],
  ])('applies the shared identifier policy to URL components in %s', async (url, internal) => {
    await i18n.changeLanguage('en-US');
    setup(() => page([question({ impact: `Decision: Keep the launch date.\n${url}` })]));
    const card = await screen.findByTestId('ordinary-question');
    expect(defaultVisibleText(card)).toContain('Decision: Keep the launch date.');
    expect(defaultVisibleText(card)).not.toContain(internal);
    expect(defaultVisibleText(card)).not.toContain(url);
    expect(within(card).queryAllByRole('button')).toHaveLength(0);
  });

  it('navigates bounded pages, resets pages on tab changes and refreshes from the beginning', async () => {
    await i18n.changeLanguage('en-US');
    const { calls } = setup((query) => query.cursor ? page([question({ question: 'Can support proceed?' })])
      : page([question()], 'private-page-two'));
    await screen.findByTestId('ordinary-question');
    await userEvent.click(screen.getByRole('button', { name: 'More questions' }));
    await screen.findByText('Can support proceed?');
    expect(calls.at(-1)?.cursor).toBe('private-page-two');
    await userEvent.click(screen.getByRole('button', { name: 'Refresh questions' }));
    await screen.findByText('Should the portal use the blue theme?');
    expect(calls.at(-1)).not.toHaveProperty('cursor');
    await userEvent.click(screen.getByRole('tab', { name: 'All questions' }));
    await waitFor(() => expect(calls.at(-1)?.decision_state).toBe('all'));
    expect(calls.at(-1)).not.toHaveProperty('cursor');
  });

  it('immediately hides stale cursor data and clears retained pages before a narrowed refresh returns', async () => {
    await i18n.changeLanguage('en-US');
    let changed = false;
    let finishRefresh!: (value: unknown) => void;
    const refresh = new Promise((resolve) => { finishRefresh = resolve; });
    const { queryClient } = setup((query) => query.cursor
      ? changed ? { ok: false, error: { code: 'CursorExpired', message: 'private-grant-change' } }
        : page([question({ question: 'Previously visible private question' })])
      : changed ? refresh : page([question()], 'page-two'));
    await userEvent.click(await screen.findByRole('button', { name: 'More questions' }));
    await screen.findByText('Previously visible private question');
    changed = true;
    await act(async () => { await queryClient.refetchQueries({ queryKey: ['team', 'decisions.list', 'demo', 'open', 'page-two'] }); });
    await waitFor(() => expect(document.body.textContent).not.toContain('Previously visible private question'));
    expect(document.body.textContent).not.toContain('Should the portal use the blue theme?');
    expect(queryClient.getQueryData(['team', 'decisions.list', 'demo', 'open', 'page-two'])).toBeUndefined();
    await act(async () => { finishRefresh(page([question({ question: 'Current public question' })])); });
    await screen.findByText('Current public question');
  });

  it('clears questions when access is revoked', async () => {
    await i18n.changeLanguage('en-US');
    let forbidden = false;
    const { queryClient } = setup(() => forbidden
      ? { ok: false, error: { code: 'Forbidden', message: 'private-access-rule' } } : page());
    await screen.findByTestId('ordinary-question');
    forbidden = true;
    await act(async () => { await queryClient.refetchQueries({ queryKey: ['team', 'decisions.list'] }); });
    await screen.findByTestId('state-forbidden');
    expect(screen.queryByTestId('ordinary-question')).toBeNull();
    expect(document.body.textContent).not.toContain('private-access-rule');
  });
});
