import { QueryClientProvider } from '@tanstack/react-query';
import { act, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { MemoryRouter } from 'react-router';
import { afterEach, describe, expect, it, vi } from 'vitest';
import i18n from '../i18n';
import { createAppQueryClient, revokeObservationReads } from '../lib/queries';
import { RevocationProvider, useRevocation } from '../lib/revocation';
import { OutcomeActivity, OutcomeReviewHistory } from './OutcomeObservations';

const WORK = `01J${'0'.repeat(22)}1`;
const EARLIER = `01J${'0'.repeat(22)}2`;
const LATEST = `01J${'0'.repeat(22)}3`;
const RESTRICTED = `01J${'0'.repeat(22)}4`;
const ACTOR = `01J${'0'.repeat(22)}5`;
const AT = 1793534400000;

function round(overrides = {}) {
  return { round_id: EARLIER, round_index: 1, round_position: 1, readable: true,
    state: 'rejected', contract_matches_current: false, reviewer_role: 'agent_reviewer',
    reviewer_actor_id: ACTOR, reviewer_person_id: 'private-reviewer', decided_at_unix_ms: AT,
    reason_first_line: `Keep the selected filter after delivery.finalize ${WORK}.`, ...overrides };
}

function response(data: unknown) {
  return new Response(JSON.stringify({ ok: true, result: { data }, freshness: {
    report_received: { status: 'not_measured' }, authoritative: { status: 'measured', at_unix_ms: AT },
    page_read: { status: 'measured', at_unix_ms: AT },
  } }), { headers: { 'content-type': 'application/json' } });
}

function Harness({ workId = WORK }: { workId?: string }) {
  const { revoked } = useRevocation();
  const [selected, setSelected] = useState(LATEST);
  return <>
    <OutcomeReviewHistory project="portal" workId={workId} stream="delivery" currentRound={LATEST}
      selectedRound={selected} onSelect={setSelected} revoked={revoked} />
    <OutcomeActivity project="portal" workId={workId} revoked={revoked} />
  </>;
}

function mount(read: (query: Record<string, unknown>) => Response | Promise<Response>, activityOverrides = {}) {
  const calls: Array<{ url: string; query?: Record<string, unknown> }> = [];
  vi.stubGlobal('fetch', vi.fn(async (input, init) => {
    const url = String(input);
    const query = init?.body ? JSON.parse(String(init.body)).query : undefined;
    calls.push({ url, query });
    if (query) return read(query);
    return new Response(JSON.stringify({ ok: true, data: { scope: 'self', next_cursor: null,
      items: [{ id: 'private-event', work_id: WORK, action: 'session.checkpoint', actor_id: ACTOR,
        client_id: 'private-client', credential_id: 'private-credential', created_at_unix_ms: AT, result: 'unknown' }],
      chat_text_collected: false, tool_io_collected: false, ...activityOverrides } }), { headers: { 'content-type': 'application/json' } });
  }));
  const client = createAppQueryClient();
  const view = render(<QueryClientProvider client={client}><RevocationProvider><MemoryRouter>
    <Harness />
  </MemoryRouter></RevocationProvider></QueryClientProvider>);
  return { ...view, calls, client };
}

afterEach(async () => { vi.unstubAllGlobals(); await act(async () => { await i18n.changeLanguage('zh-CN'); }); });

describe('outcome observation product views', () => {
  it.each(['zh-CN', 'en-US'])('shows review history and scoped work activity without diagnostics in %s', async (language) => {
    await i18n.changeLanguage(language);
    const { calls, container } = mount(() => response({ work_id: WORK,
      items: [round(), round({ round_id: LATEST, round_index: 2, round_position: 2, state: 'approved', contract_matches_current: true })],
      total: 2, next_cursor: null, as_of: { at_unix_ms: AT } }));
    const panel = within(await screen.findByTestId('outcome-review-history'));
    await panel.findByRole('button', { name: language === 'zh-CN' ? '第 1 次复核' : 'Review 1' });
    expect(panel.getAllByText(/Keep the selected filter/)).toHaveLength(2);
    expect(panel.getByText(language === 'zh-CN' ? '此后验收要求已发生变化' : 'Acceptance requirements have changed since this review')).toBeTruthy();
    await screen.findByTestId('outcome-activity');
    await waitFor(() => expect(screen.getByTestId('outcome-activity').textContent).toContain(language === 'zh-CN' ? '结果未知' : 'Outcome unknown'));
    const rendered = container.textContent;
    for (const raw of [WORK, EARLIER, LATEST, ACTOR, 'private-reviewer', 'private-event', 'private-client', 'private-credential', 'delivery.finalize', 'session.checkpoint', String(AT)]) expect(rendered).not.toContain(raw);
    expect(container.querySelector('details, pre, code, [data-testid="technical-details"]')).toBeNull();
    expect(calls.find(call => call.query)?.query).toMatchObject({ op: 'review.rounds', work_id: WORK, workstream_id: 'delivery', limit: 20 });
    const activityRead = calls.find(call => call.url.includes('/api/team/activity'));
    expect(new URL(activityRead!.url, 'http://localhost').searchParams.get('work_id')).toBe(WORK);
    expect(screen.getByRole('link', { name: language === 'zh-CN' ? '查看这项工作的全部动态' : 'See all activity for this work' }).getAttribute('href')).toContain(`work=${WORK}`);
    const user = userEvent.setup();
    await user.click(panel.getByRole('button', { name: language === 'zh-CN' ? '第 1 次复核' : 'Review 1' }));
    expect(panel.getByRole('button', { name: language === 'zh-CN' ? '第 1 次复核' : 'Review 1' }).getAttribute('aria-pressed')).toBe('true');
    await user.click(panel.getByRole('button', { name: language === 'zh-CN' ? '返回最近一次复核' : 'Return to latest review' }));
    expect(calls.filter(call => call.query && call.query.op !== 'review.rounds')).toHaveLength(0);
  });

  it('keeps restricted review metadata out of the page and disables selecting it', async () => {
    mount(() => response({ work_id: WORK, items: [round({ round_id: RESTRICTED, readable: false,
      reason_first_line: 'private-hidden-reason', state: null, reviewer_role: null })], total: 1,
      next_cursor: null, as_of: { at_unix_ms: AT } }));
    const button = await screen.findByRole('button', { name: '第 1 次复核' });
    expect(button.hasAttribute('disabled')).toBe(true);
    expect(screen.getByText('这次复核的详情不可查看')).toBeTruthy();
    expect(document.body.textContent).not.toContain('private-hidden-reason');
    expect(document.body.textContent).not.toContain('private-reviewer');
  });

  it('uses bounded cursors and refreshes from the first page after a changed history', async () => {
    let changed = false;
    const { calls } = mount(query => query.cursor && changed
      ? new Response(JSON.stringify({ ok: false, error: { code: 'PreconditionsChanged', message: 'private-cursor-diagnostic' } }))
      : response({ work_id: WORK, items: [round({ round_id: query.cursor ? LATEST : EARLIER, round_index: query.cursor ? 2 : 1 })], total: 2,
        next_cursor: query.cursor ? null : 'private-next-cursor', as_of: { at_unix_ms: AT } }));
    const panel = within(await screen.findByTestId('outcome-review-history'));
    const user = userEvent.setup();
    await user.click(await panel.findByRole('button', { name: '下一页' }));
    await panel.findByRole('button', { name: /第 2 次复核/ });
    changed = true;
    await user.click(panel.getByRole('button', { name: '刷新复核历程' }));
    await panel.findByRole('button', { name: '第 1 次复核' });
    expect(calls.filter(call => call.query?.op === 'review.rounds').map(call => call.query?.cursor)).toContain('private-next-cursor');
    expect(calls.filter(call => call.query?.op === 'review.rounds').at(-1)?.query?.cursor).toBeUndefined();
    expect(document.body.textContent).not.toContain('private-next-cursor');
  });

  it.each(['zh-CN', 'en-US'])('explains a failed history read without claiming an empty history in %s', async (language) => {
    await i18n.changeLanguage(language);
    mount(() => new Response(JSON.stringify({ ok: false, error: { code: 'ReadTimeout', message: 'private-upstream-diagnostic' } })));
    await screen.findByText(language === 'zh-CN' ? '复核历程暂时读取不到' : 'Review history cannot be loaded right now');
    expect(screen.queryByText(language === 'zh-CN' ? '还没有复核记录' : 'No review records yet')).toBeNull();
    expect(document.body.textContent).not.toMatch(/ReadTimeout|private-upstream-diagnostic/);
  });

  it('removes review and activity records on revocation and stops reads', async () => {
    const { calls, client } = mount(() => response({ work_id: WORK, items: [round()], total: 1,
      next_cursor: null, as_of: { at_unix_ms: AT } }));
    await screen.findByText(/Keep the selected filter/);
    act(() => revokeObservationReads(client));
    await screen.findAllByTestId('state-forbidden');
    expect(screen.queryByText(/Keep the selected filter/)).toBeNull();
    const before = calls.length;
    await act(async () => { await new Promise(resolve => setTimeout(resolve, 50)); });
    expect(calls).toHaveLength(before);
  });
});
