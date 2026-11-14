import { QueryClientProvider } from '@tanstack/react-query';
import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';
import i18n from '../i18n';
import { createAppQueryClient } from '../lib/queries';
import { RevocationProvider } from '../lib/revocation';
import { PlanningChanges, SourceReadiness } from './PlanObservations';

const AT = 1793534400000;
const INTERNAL = '01J000000000000000000000001';
const DIGEST = 'a'.repeat(64);

function candidate(overrides = {}) {
  return {
    candidate_id: INTERNAL, candidate_digest: DIGEST, state: 'published', draft_revision: 2,
    author: { display_name: 'Maya', actor_id: INTERNAL }, updated_at_unix_ms: AT,
    baseline: { matches_current: true },
    current_diff: { counts: { added: 0, removed: 0, changed: 1, unchanged: 0 },
      items: [{ work_id: INTERNAL, title: 'Clarify sign-in requirements', kind: 'changed',
        fields: [{ field: 'scope_paths', before: { value: '/private/original' }, after: { value: '/private/implementation' } },
          { field: 'acceptance', before: { value: ['Password sign-in'] }, after: { value: ['Password and passkey sign-in'] } }] }] },
    approval: { value: { display_name: 'Kai' } },
    publication: { value: { source_writeback_pending: true } },
    activation: { value: null }, ...overrides,
  };
}

function documents(overrides = {}) {
  return { items: [{ source_position: 1, path: 'docs/GOALS.md', status: 'consistent', reason: null,
    expected_sha256: DIGEST, current_sha256: DIGEST }], total: 1, next_cursor: null,
    collection: { complete: true, required_total: 1, checked_at_unix_ms: AT },
    as_of: { at_unix_ms: AT }, ...overrides };
}

function response(data: unknown) {
  return new Response(JSON.stringify({ ok: true, result: { data }, freshness: {
    report_received: { status: 'not_measured' }, authoritative: { status: 'measured', at_unix_ms: AT },
    page_read: { status: 'measured', at_unix_ms: AT },
  } }), { headers: { 'content-type': 'application/json' } });
}

function mount(read: (query: Record<string, unknown>) => Response | Promise<Response>, project = 'portal') {
  const calls: Array<Record<string, unknown>> = [];
  vi.stubGlobal('fetch', vi.fn(async (_input, init) => {
    const body = JSON.parse(init.body);
    calls.push(body);
    return read(body.query);
  }));
  const client = createAppQueryClient();
  const view = render(<QueryClientProvider client={client}><RevocationProvider>
    <PlanningChanges project={project} stream="development" />
    <SourceReadiness project={project} stream="development" workId="sign-in" />
  </RevocationProvider></QueryClientProvider>);
  return { ...view, calls, client };
}

afterEach(async () => { vi.unstubAllGlobals(); await i18n.changeLanguage('zh-CN'); });

describe('plan observation product views', () => {
  it.each(['zh-CN', 'en-US'])('shows recorded plan and document facts without any diagnostic disclosures in %s', async (language) => {
    await i18n.changeLanguage(language);
    const { container, calls } = mount((q) => response(q.op === 'sources.inspect' ? documents() : {
      items: [candidate()], total: 1, next_cursor: null, as_of: { at_unix_ms: AT },
    }));
    await screen.findByText('Clarify sign-in requirements');
    await screen.findByText(language === 'zh-CN' ? '所需资料已齐备' : 'Required documents are ready');
    expect(screen.getByTestId('plan-materials').textContent).toContain(language === 'zh-CN' ? '项目目标' : 'Project goals');
    expect(screen.getByTestId('plan-changes').textContent).toContain('Password and passkey sign-in');
    expect(screen.getByTestId('plan-changes').textContent).toContain(language === 'zh-CN' ? '等待生效' : 'Waiting to take effect');
    expect(container.textContent).not.toMatch(new RegExp(`${INTERNAL}|${DIGEST}|scope_paths|source_snapshot_id|/private/implementation|技术细节|Technical details`));
    expect(container.querySelector('details, pre, code, [data-testid="technical-details"]')).toBeNull();
    expect(screen.queryByRole('button', { name: /批准|发布|认领|Approve|Publish|Claim|Copy|复制/i })).toBeNull();
    expect(calls.map((c) => c.query)).toEqual(expect.arrayContaining([
      expect.objectContaining({ op: 'planning.candidates', workstream_id: 'development' }),
      expect.objectContaining({ op: 'sources.inspect', work_id: 'sign-in' }),
    ]));
  });

  it('uses whole-collection readiness across pages and restarts from the first page on refresh', async () => {
    const user = userEvent.setup();
    const { calls } = mount((q) => response(q.op === 'sources.inspect' ? documents({
      items: q.cursor ? [{ source_position: 2, path: null, status: 'forbidden', reason: 'required_spec_forbidden' }]
        : documents().items,
      total: 2, next_cursor: q.cursor ? null : 'second-page',
      collection: { complete: false, required_total: 2, checked_at_unix_ms: AT },
    }) : { items: [], total: 0, next_cursor: null, as_of: { at_unix_ms: AT } }));
    await screen.findByText('可以使用');
    const panel = within(screen.getByTestId('plan-materials'));
    expect(panel.getByText('部分资料需要处理')).toBeTruthy();
    expect(panel.queryByText('所需资料已齐备')).toBeNull();
    await user.click(panel.getByRole('button', { name: '下一页' }));
    await panel.findByText('需要访问权限，请联系项目负责人确认');
    await user.click(panel.getByRole('button', { name: '刷新' }));
    await panel.findByText('可以使用');
    const reads = calls.filter((c) => (c.query as Record<string, unknown>).op === 'sources.inspect');
    expect((reads.at(-1)?.query as Record<string, unknown>).cursor).toBeUndefined();
  });

  it('keeps applied, previously applied, approved and unreviewed changes distinct', async () => {
    const statuses = [
      { state: 'published', activation: { value: { matches_current_snapshot: true } } },
      { state: 'published', activation: { value: { matches_current_snapshot: false } } },
      { state: 'approved', activation: { value: null }, publication: { value: null } },
      { state: 'drafting', activation: { value: null }, publication: { value: null }, approval: { value: null }, baseline: { matches_current: false } },
    ];
    mount((q) => response(q.op === 'sources.inspect' ? documents() : {
      items: statuses.map((s, index) => candidate({ ...s, candidate_id: `proposal-${index}` })),
      total: 4, next_cursor: null, as_of: { at_unix_ms: AT },
    }));
    for (const label of ['已生效', '此前已生效', '已批准，尚未生效', '等待复核']) await screen.findByText(label, { exact: true });
    expect(screen.getByText('提出此调整后，项目计划已有变化，需要 Agent 重新核对')).toBeTruthy();
  });

  it.each(['changed', 'missing', 'forbidden', 'future_condition'])('explains %s documents without exposing raw reasons or metadata', async (status) => {
    const { container } = mount((q) => response(q.op === 'sources.inspect' ? documents({
      items: [{ source_position: 1, path: null, status, reason: 'sensitive_backend_reason', current_sha256: DIGEST }],
      collection: { complete: false, required_total: 1, checked_at_unix_ms: AT },
    }) : { items: [], total: 0, next_cursor: null, as_of: { at_unix_ms: AT } }));
    await screen.findByText('部分资料需要处理');
    expect(container.textContent).not.toContain('sensitive_backend_reason');
    expect(container.textContent).not.toContain(DIGEST);
    expect(screen.getByTestId('plan-materials').textContent).toMatch(/请让 Agent|请联系项目负责人/);
  });

  it('does not call an empty required set ready', async () => {
    mount((q) => response(q.op === 'sources.inspect' ? documents({ items: [], total: 0,
      collection: { complete: true, required_total: 0, checked_at_unix_ms: AT } })
      : { items: [], total: 0, next_cursor: null, as_of: { at_unix_ms: AT } }));
    await screen.findByText('尚未登记所需资料');
    expect(screen.queryByText('所需资料已齐备')).toBeNull();
  });

  it('shows a safe failure and retry for malformed responses instead of presenting incomplete data as verified', async () => {
    mount(() => response({ items: [], next_cursor: null, collection: { complete: true } }));
    await screen.findByText('暂时无法核对项目资料', {}, { timeout: 4000 });
    expect(screen.queryByText('所需资料已齐备')).toBeNull();
    expect(screen.getByTestId('plan-materials').textContent).not.toContain('InvalidResponse');
  });

  it('retains previously confirmed material only with an explicit warning after a failed refresh', async () => {
    let failed = false;
    const user = userEvent.setup();
    mount((q) => q.op === 'sources.inspect' && failed
      ? new Response(JSON.stringify({ ok: false, error: { code: 'BridgeError', message: `internal_debug ${INTERNAL}` } }))
      : response(q.op === 'sources.inspect' ? documents() : { items: [], total: 0, next_cursor: null, as_of: { at_unix_ms: AT } }));
    await screen.findByText('所需资料已齐备');
    failed = true;
    await user.click(within(screen.getByTestId('plan-materials')).getByRole('button', { name: '刷新' }));
    await screen.findByText('暂时无法核对项目资料', {}, { timeout: 4000 });
    expect(screen.getByText('以下是上次已确认的信息，最新更新暂未成功')).toBeTruthy();
    expect(document.body.textContent).not.toMatch(/internal_debug|BridgeError/);
    expect(document.body.textContent).not.toContain(INTERNAL);
  });

  it('clears documents and plan facts after access is revoked', async () => {
    let revoked = false;
    const user = userEvent.setup();
    mount((q) => revoked ? new Response(JSON.stringify({ ok: false, error: { code: 'Forbidden', message: INTERNAL } }))
      : response(q.op === 'sources.inspect' ? documents() : { items: [candidate()], total: 1, next_cursor: null, as_of: { at_unix_ms: AT } }));
    await screen.findByText('Clarify sign-in requirements');
    await screen.findByText('所需资料已齐备');
    revoked = true;
    await user.click(within(screen.getByTestId('plan-materials')).getByRole('button', { name: '刷新' }));
    await waitFor(() => expect(screen.queryByText('Clarify sign-in requirements')).toBeNull());
    expect(screen.queryByText('所需资料已齐备')).toBeNull();
    expect(document.body.textContent).not.toContain(INTERNAL);
  });
});
