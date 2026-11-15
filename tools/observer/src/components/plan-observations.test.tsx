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

function recorded(value: Record<string, unknown>) {
  return { value: { actor_id: 'reviewer', at_unix_ms: AT, ...value }, reason: null };
}

function absent(reason: string) {
  return { value: null, reason };
}

function candidate(overrides = {}) {
  return {
    candidate_id: INTERNAL, candidate_digest: DIGEST, state: 'published', draft_revision: 2,
    author: { display_name: 'Maya', actor_id: INTERNAL }, updated_at_unix_ms: AT,
    baseline: { matches_current: true },
    current_diff: { counts: { added: 0, removed: 0, changed: 1, unchanged: 0 },
      items: [{ work_id: INTERNAL, title: 'Clarify sign-in requirements', kind: 'changed',
        fields: [{ field: 'scope_paths', before: { value: '/private/original' }, after: { value: '/private/implementation' } },
          { field: 'acceptance', before: { value: ['Password sign-in'] }, after: { value: ['Password and passkey sign-in'] } }] }] },
    approval: recorded({ display_name: 'Kai', self_approved: false }),
    publication: recorded({ source_writeback_pending: true }),
    activation: absent('not_applied'), ...overrides,
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
      { state: 'published', activation: recorded({ activated_snapshot_id: INTERNAL, matches_current_snapshot: true }), publication: recorded({ source_writeback_pending: false }) },
      { state: 'published', activation: recorded({ activated_snapshot_id: INTERNAL, matches_current_snapshot: false }), publication: recorded({ source_writeback_pending: false }) },
      { state: 'approved', activation: absent('not_applied'), publication: absent('not_published') },
      { state: 'drafting', activation: absent('not_applied'), publication: absent('not_published'), approval: absent('not_approved'), baseline: { matches_current: false } },
    ];
    mount((q) => response(q.op === 'sources.inspect' ? documents() : {
      items: statuses.map((s, index) => candidate({ ...s, candidate_id: `proposal-${index}` })),
      total: 4, next_cursor: null, as_of: { at_unix_ms: AT },
    }));
    for (const label of ['已生效', '此前已生效', '已批准，尚未生效', '等待复核']) await screen.findByText(label, { exact: true });
    expect(screen.getByText('提出此调整后，项目计划已有变化，需要 Agent 重新核对')).toBeTruthy();
  });

  it.each(['zh-CN', 'en-US'])('explains a recorded returned plan and its next step in %s', async (language) => {
    await i18n.changeLanguage(language);
    const { container,calls } = mount((q) => response(q.op === 'sources.inspect' ? documents() : {
      items: [candidate({ state:'rejected', approval:absent('not_approved'), publication:absent('not_published'),
        baseline:{ matches_current:false }, rejection:recorded({ display_name:'Kai',
          reason:'Clarify the acceptance criteria before adding this work' }) })],
      total:1,next_cursor:null,as_of:{at_unix_ms:AT},
    }));
    await screen.findByText(language === 'zh-CN' ? '已退回，需调整' : 'Returned for revision');
    expect(screen.getByText('Clarify the acceptance criteria before adding this work')).toBeTruthy();
    const panel=screen.getByTestId('plan-changes');
    expect(panel.textContent).toContain(language === 'zh-CN' ? 'Kai 退回' : 'Returned by Kai');
    expect(panel.textContent).toContain(i18n.t('plan.changes.returnedNext'));
    expect(screen.queryByText(i18n.t('plan.changes.planChanged'))).toBeNull();
    expect(container.textContent).not.toMatch(new RegExp(`${INTERNAL}|${DIGEST}|candidate_digest|source_snapshot_id`));
    expect(panel.querySelector('details,pre,code')).toBeNull();
    expect(within(panel).queryByRole('button',{name:/退回|批准|发布|Reject|Approve|Publish/i})).toBeNull();
    expect(calls.every((c)=>(c.query as Record<string,unknown>).op === 'planning.candidates'
      || (c.query as Record<string,unknown>).op === 'sources.inspect')).toBe(true);
  });

  it('keeps a permitted returned state visible when decision details are restricted', async () => {
    mount((q)=>response(q.op === 'sources.inspect' ? documents() : {
      items:[candidate({state:'rejected',approval:absent('not_approved'),publication:absent('not_published'),
        rejection:absent('permission_restricted')})],total:1,next_cursor:null,as_of:{at_unix_ms:AT},
    }));
    await screen.findByText('已退回，需调整');
    expect(within(screen.getByTestId('plan-changes')).getByText(i18n.t('plan.changes.restricted'))).toBeTruthy();
    expect(document.body.textContent).not.toContain('permission_restricted');
  });

  it('hides technical decision text without a disclosure or raw-copy control', async () => {
    mount((q)=>response(q.op === 'sources.inspect' ? documents() : {
      items:[candidate({state:'rejected',approval:absent('not_approved'),publication:absent('not_published'),
        rejection:recorded({display_name:INTERNAL,reason:`candidate_digest=${DIGEST} source_snapshot_id=${INTERNAL}`})})],
      total:1,next_cursor:null,as_of:{at_unix_ms:AT},
    }));
    await screen.findByText('已退回，需调整');
    const panel=screen.getByTestId('plan-changes');
    expect(panel.textContent).not.toMatch(new RegExp(`${INTERNAL}|${DIGEST}|candidate_digest|source_snapshot_id`));
    expect(panel.querySelector('details,pre,code,[data-testid="technical-details"]')).toBeNull();
  });

  it.each(['zh-CN', 'en-US'])('uses the product fallback for payload-shaped returned reasons in %s', async (language) => {
    await i18n.changeLanguage(language);
    const reasons = [
      'payload={"token":"synthetic-session-value","reason":"clarify scope"}',
      '{"reason":"clarify scope","candidate_id":"proposal-123"}',
      'candidate_id=proposal-123',
      'token: synthetic-session-value',
      ...['client_secret', 'auth_token', 'session_token', 'private_key', 'x-api-key', 'clientSecret']
        .flatMap((field) => [`${field}=synthetic-session-value`, `Review https://example.com/guide?${field}=synthetic-session-value`]),
      'Review https://example.com/guide#%61%75%74%68%5f%74%6f%6b%65%6e=synthetic-session-value',
      'Review https://example.com/%63%6c%69%65%6e%74%5f%73%65%63%72%65%74=synthetic-session-value',
    ];
    const { container } = mount((q) => response(q.op === 'sources.inspect' ? documents() : {
      items: reasons.map((reason, index) => candidate({ candidate_id: `${INTERNAL}-${index}`,
        state: 'rejected', approval: absent('not_approved'), publication: absent('not_published'),
        rejection: recorded({ display_name: 'Kai', reason }),
      })), total: reasons.length, next_cursor: null, as_of: { at_unix_ms: AT },
    }));
    await screen.findAllByText(i18n.t('plan.changes.returnedReasonUnavailable'));
    expect(screen.getAllByText(i18n.t('plan.changes.returnedReasonUnavailable')))
      .toHaveLength(reasons.filter((reason) => !reason.startsWith('Review https://')).length);
    expect(container.textContent).not.toMatch(/synthetic-session-value|proposal-123|payload|candidate_id|clarify scope/);
    expect(container.querySelector('details,pre,code,[data-testid="technical-details"]')).toBeNull();
    expect(within(screen.getByTestId('plan-changes')).queryByRole('button', { name: /copy|复制/i })).toBeNull();
  });

  it.each([
    {state:'rejected'},
    {state:'rejected',rejection:recorded({display_name:'Kai',reason:' '})},
    {state:'rejected',rejection:absent('not_rejected')},
    {state:'rejected',rejection:absent('permission_restricted')},
    {state:'rejected',rejection:recorded({display_name:'Kai',reason:'Clarify the outcome'})},
    {state:'drafting',rejection:recorded({display_name:'Kai',reason:'Clarify the outcome'})},
  ])('refuses missing or contradictory returned-plan records', async (change) => {
    mount((q)=>response(q.op === 'sources.inspect' ? documents() : {
      items:[candidate(change)],total:1,next_cursor:null,as_of:{at_unix_ms:AT},
    }));
    await screen.findByText('暂时无法读取计划变更',{}, {timeout:4000});
    expect(screen.queryByText('已退回，需调整')).toBeNull();
    expect(document.body.textContent).not.toMatch(/InvalidResponse|candidate_digest/);
  });

  it.each(['changed', 'missing', 'forbidden', 'unverified'])('explains %s documents without exposing raw reasons or metadata', async (status) => {
    const { container } = mount((q) => response(q.op === 'sources.inspect' ? documents({
      items: [{ source_position: 1, path: null, status, reason: 'sensitive_backend_reason', current_sha256: DIGEST }],
      collection: { complete: false, required_total: 1, checked_at_unix_ms: AT },
    }) : { items: [], total: 0, next_cursor: null, as_of: { at_unix_ms: AT } }));
    await screen.findByText('部分资料需要处理');
    expect(container.textContent).not.toContain('sensitive_backend_reason');
    expect(container.textContent).not.toContain(DIGEST);
    expect(screen.getByTestId('plan-materials').textContent).toMatch(/请让 Agent|请联系项目负责人/);
  });

  it.each([
    { approval: { value: {} } },
    { publication: recorded({}) },
    { activation: recorded({ activated_snapshot_id: INTERNAL }) },
    { activation: recorded({ activated_snapshot_id: INTERNAL, matches_current_snapshot: true }) },
  ])('refuses incomplete or contradictory recorded plan events', async (overrides) => {
    mount((q) => response(q.op === 'sources.inspect' ? documents() : {
      items: [candidate(overrides)], total: 1, next_cursor: null, as_of: { at_unix_ms: AT },
    }));
    await screen.findByText('暂时无法读取计划变更', {}, { timeout: 4000 });
    expect(screen.queryByText('已生效', { exact: true })).toBeNull();
    expect(screen.queryByText('此前已生效', { exact: true })).toBeNull();
    expect(document.body.textContent).not.toContain(INTERNAL);
  });

  it.each(['missing', 'changed', 'forbidden', 'unverified', 'future_condition'])('does not show ready for a contradictory or unknown %s document', async (status) => {
    mount((q) => response(q.op === 'sources.inspect' ? documents({
      items: [{ source_position: 1, path: null, status, reason: 'backend_detail' }],
    }) : { items: [], total: 0, next_cursor: null, as_of: { at_unix_ms: AT } }));
    await screen.findByText('暂时无法核对项目资料', {}, { timeout: 4000 });
    expect(screen.queryByText('所需资料已齐备')).toBeNull();
    expect(document.body.textContent).not.toContain('backend_detail');
  });

  it.each(['zh-CN', 'en-US'])('describes policy and restricted dependency changes in product language in %s', async (language) => {
    await i18n.changeLanguage(language);
    mount((q) => response(q.op === 'sources.inspect' ? documents() : {
      items: [candidate({ current_diff: { counts: { added: 0, removed: 0, changed: 1, unchanged: 0 }, items: [{
        work_id: INTERNAL, title: 'Clarify collaboration requirements', kind: 'changed', fields: [
          { field: 'dependency_acceptance', before: { value: null }, after: { value: { [INTERNAL]: 'agent_reviewed_caller_asserted_reconciled' } } },
          { field: 'execution_settlement', before: { value: { mode: 'independent_workspace_v1', workspace_id: INTERNAL } }, after: { value: { mode: 'independent_workspace_v2', workspace_id: INTERNAL } } },
          { field: 'required_dependencies', before: { value: null, reason: 'permission_restricted' }, after: { value: [INTERNAL] } },
        ],
      }] } })], total: 1, next_cursor: null, as_of: { at_unix_ms: AT },
    }));
    const panel = within(screen.getByTestId('plan-changes'));
    await panel.findByText(language === 'zh-CN' ? '上游成果的复核要求' : 'Upstream result review requirements');
    expect(panel.getByText(language === 'zh-CN' ? '工作结束时的确认要求' : 'End-of-work confirmation requirements')).toBeTruthy();
    expect(panel.getByText(language === 'zh-CN' ? '调整为：授权到期后，仍可核对原次执行已停止的结果' : 'Proposed: The original execution can be confirmed as stopped after its access expires')).toBeTruthy();
    expect(panel.getByText(language === 'zh-CN' ? '原先：需要访问权限' : 'Previously: Access restricted')).toBeTruthy();
    expect(document.body.textContent).not.toMatch(/dependency_acceptance|execution_settlement|workspace_id|independent_workspace|permission_restricted|agent_reviewed_caller_asserted_reconciled/);
    expect(document.body.textContent).not.toContain(INTERNAL);
  });

  it('does not call an empty required set ready', async () => {
    mount((q) => response(q.op === 'sources.inspect' ? documents({ items: [], total: 0,
      collection: { complete: true, required_total: 0, checked_at_unix_ms: AT } })
      : { items: [], total: 0, next_cursor: null, as_of: { at_unix_ms: AT } }));
    await screen.findByText('尚未登记所需资料');
    expect(screen.queryByText('所需资料已齐备')).toBeNull();
  });

  it.each(['zh-CN', 'en-US'])('preserves review-assurance and workspace-only changes without exposing identifiers in %s', async (language) => {
    await i18n.changeLanguage(language);
    mount((q) => response(q.op === 'sources.inspect' ? documents() : {
      items: [candidate({ current_diff: { counts: { added: 0, removed: 0, changed: 1, unchanged: 0 }, items: [{
        work_id: INTERNAL, title: 'Update collaboration requirements', kind: 'changed', fields: [
          { field: 'dependency_acceptance',
            before: { value: { [INTERNAL]: { cross_workstream: { review_assurance: 'team_independent', version_policy: 'current_contract' } } } },
            after: { value: { [INTERNAL]: { cross_workstream: { review_assurance: 'simulated_member_independent', version_policy: 'current_contract' } } } } },
          { field: 'execution_settlement',
            before: { value: { mode: 'independent_workspace_v1', workspace_id: INTERNAL } },
            after: { value: { mode: 'independent_workspace_v1', workspace_id: 'opaque-replacement' } } },
        ],
      }] } })], total: 1, next_cursor: null, as_of: { at_unix_ms: AT },
    }));
    await screen.findByText(language === 'zh-CN' ? '本次工作改用另一个工作区' : 'A different workspace is assigned to this work');
    const panel = screen.getByTestId('plan-changes');
    expect(panel.textContent).toContain(language === 'zh-CN' ? '原先：上游成果须由另一位成员独立复核' : 'Previously: Upstream results require independent review by another member');
    expect(panel.textContent).toContain(language === 'zh-CN' ? '调整为：上游成果须由另一名协作 Agent 复核' : 'Proposed: Use upstream results reviewed by another collaborating Agent');
    expect(panel.textContent).not.toMatch(/opaque-replacement|workspace_id|cross_workstream|review_assurance|version_policy|simulated_member_independent|independent_workspace_v1/);
    expect(panel.textContent).not.toContain(INTERNAL);
    expect(panel.querySelector('details, pre, code')).toBeNull();
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
    expect(screen.queryByText('所需资料已齐备')).toBeNull();
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
