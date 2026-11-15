import { QueryClientProvider } from '@tanstack/react-query';
import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { MemoryRouter } from 'react-router';
import type { ReactNode } from 'react';
import i18n from '../i18n';
import { createAppQueryClient, revokeObservationReads } from '../lib/queries';
import { RevocationProvider } from '../lib/revocation';
import type { DependencyObservation } from '../lib/types';
import { DependencyDetails, DependencyObservations } from './DependencyObservations';
import { WorkResponsibility } from './WorkResponsibility';

const AT = 1793534400000;
const INTERNAL = '01J000000000000000000000001';
const DIGEST = 'a'.repeat(64);
const VERSION = { receipt_id: INTERNAL, contract_hash: DIGEST, source_snapshot_id: INTERNAL };

function edge(overrides: Partial<DependencyObservation> = {}): DependencyObservation {
  return {
    work_id: 'customer-portal', work_title: 'Build the customer portal', provider_work_id: 'customer-api',
    provider_title: 'Deliver the customer API', dependency_position: 1, version_policy: 'current_contract',
    review_assurance: { cross_workstream: { review_assurance: 'team_independent', version_policy: 'current_contract' } },
    required_version: VERSION, available_version: VERSION, adopted_version: VERSION,
    consumed_version: VERSION, adoption_valid: true, validity: 'valid', reason: null, missing: {}, ...overrides,
  };
}

function response(data: unknown) {
  return new Response(JSON.stringify({ ok: true, result: { data }, freshness: {
    authoritative: { status: 'measured', at_unix_ms: AT },
    page_read: { status: 'measured', at_unix_ms: AT }, report_received: { status: 'not_measured' },
  } }), { headers: { 'content-type': 'application/json' } });
}

function error(code: string) {
  return new Response(JSON.stringify({ ok: false, error: { code, message: `Internal details ${DIGEST}` } }), {
    headers: { 'content-type': 'application/json' }, status: code === 'Forbidden' ? 403 : 409,
  });
}

function mount(content: ReactNode, read: (query: Record<string, unknown>) => Response | Promise<Response>) {
  const calls: Array<Record<string, unknown>> = [];
  vi.stubGlobal('fetch', vi.fn(async (_input, init) => {
    const body = JSON.parse(init.body);
    calls.push(body.query);
    return read(body.query);
  }));
  const client = createAppQueryClient({ retry: false });
  const view = render(<QueryClientProvider client={client}><RevocationProvider><MemoryRouter>{content}</MemoryRouter></RevocationProvider></QueryClientProvider>);
  return { ...view, client, calls };
}

function expectProductOnly(container: HTMLElement) {
  const visible = [container.textContent, ...[...container.querySelectorAll('[title],[aria-label]')]
    .flatMap((node) => [node.getAttribute('title'), node.getAttribute('aria-label')])].join(' ');
  expect(visible).not.toMatch(new RegExp(`${INTERNAL}|${DIGEST}|contract_hash|receipt_id|source_snapshot_id|current_contract|fixed_delivery|person-internal|agent-internal|技术细节|Technical details`));
  expect(container.querySelector('details, pre, code')).toBeNull();
}

afterEach(async () => { vi.unstubAllGlobals(); await i18n.changeLanguage('zh-CN'); });

describe('dependency observation', () => {
  it.each(['zh-CN', 'en-US'])('presents version relationships and only sends read operations in %s', async (language) => {
    await i18n.changeLanguage(language);
    const { container, calls } = mount(<DependencyObservations project="portal" />, () => response({
      items: [edge()], total: 1, next_cursor: null, as_of: { at_unix_ms: AT },
    }));
    const detail = await screen.findByTestId('dependency-detail');
    expect(detail.textContent).toContain(language === 'zh-CN' ? '与已采用成果一致' : 'Matches the adopted result');
    expect(detail.textContent).toContain(language === 'zh-CN' ? '团队独立复核' : 'Independent team review');
    expect(detail.textContent).toContain(language === 'zh-CN' ? '其余执行条件' : 'remaining execution conditions');
    const upstream = within(detail).getByRole('link', { name: 'Deliver the customer API' });
    expect(upstream.getAttribute('href')).toBe('/projects/portal?tab=plan&work=customer-api');
    expect(calls.every((query) => query.op === 'dependencies.list')).toBe(true);
    expectProductOnly(container);
  });

  it.each(['zh-CN', 'en-US'])('keeps a pinned historical delivery valid when other work changes in %s', async (language) => {
    await i18n.changeLanguage(language);
    const { container } = mount(<DependencyDetails project="portal" item={edge({ version_policy: 'fixed_delivery',
      review_assurance: { cross_workstream: { review_assurance: 'team_independent', version_policy: 'fixed_delivery' } } })} />, () => response({}));
    expect(container.textContent).toContain(language === 'zh-CN' ? '保持约定的历史交付' : 'Keep the agreed historical delivery');
    expect(container.textContent).not.toContain(language === 'zh-CN' ? '需要核对' : 'Needs attention');
    expectProductOnly(container);
  });

  it('keeps adopted and execution-consumed results distinct after a new adoption', () => {
    mount(<DependencyDetails project="portal" item={edge({ consumed_version: { ...VERSION, receipt_id: 'previous-delivery' },
      validity: 'upstream_changed', reason: 'execution_inputs_changed' })} />, () => response({}));
    const detail = screen.getByTestId('dependency-detail');
    expect(detail.textContent).toContain('与可用成果一致');
    expect(detail.textContent).toContain('与当前采用的成果不同');
    expect(detail.textContent).toContain('当前采用的成果与上次执行使用的成果不同');
    expect(detail.textContent).not.toContain('previous-delivery');
  });

  it('keeps a first-use available delivery distinct from adoption and execution', () => {
    mount(<DependencyDetails project="portal" item={edge({ adopted_version: null, consumed_version: null,
      adoption_valid: null, review_assurance: null })} />, () => response({}));
    const detail = screen.getByTestId('dependency-detail');
    expect(detail.textContent).toContain('已有通过验收的成果');
    expect(detail.textContent).toContain('尚无采用记录');
    expect(detail.textContent).toContain('尚无执行使用记录');
  });

  it.each(['zh-CN', 'en-US'])('keeps restricted upstream metadata undisclosed in %s', async (language) => {
    await i18n.changeLanguage(language);
    const { container } = mount(<DependencyDetails project="portal" item={edge({ provider_title: null,
      required_version: null, available_version: null, adopted_version: null, consumed_version: null,
      validity: 'unavailable', reason: 'permission_restricted', missing: { required_version: 'permission_restricted' } })} />, () => response({}));
    expect(container.textContent).toContain(language === 'zh-CN' ? '受限工作' : 'Restricted work');
    expect(container.textContent).toContain(language === 'zh-CN' ? '当前无权查看' : 'Access is restricted');
    expect(screen.queryByRole('link', { name: 'Deliver the customer API' })).toBeNull();
    expectProductOnly(container);
  });

  it('restarts an expired page from the latest records and hides stale details on read failure', async () => {
    const user = userEvent.setup();
    const { calls } = mount(<DependencyObservations project="portal" />, (query) => query.cursor ? error('PreconditionsChanged')
      : response({ items: [edge()], total: 2, next_cursor: 'page-two', as_of: { at_unix_ms: AT } }));
    await screen.findByTestId('dependency-detail');
    await user.click(screen.getByRole('button', { name: '下一页' }));
    await screen.findByText('依赖信息暂时无法读取');
    expect(screen.queryByTestId('dependency-detail')).toBeNull();
    expect(document.body.textContent).not.toContain(DIGEST);
    await user.click(screen.getByRole('button', { name: '刷新记录' }));
    await screen.findByTestId('dependency-detail');
    await waitFor(() => expect(calls.at(-1)?.cursor).toBeUndefined());
  });

  it('distinguishes no dependency records from an unsupported read', async () => {
    const { client } = mount(<DependencyObservations project="portal" />, () => response({ items: [], total: 0, next_cursor: null, as_of: { at_unix_ms: AT } }));
    await screen.findByText('当前范围没有依赖记录');
    vi.stubGlobal('fetch', vi.fn(async () => error('Unsupported')));
    await client.refetchQueries();
    await screen.findByText('依赖信息暂时无法读取');
    expect(screen.queryByText('当前范围没有依赖记录')).toBeNull();
  });

  it('clears dependencies after permission revocation', async () => {
    const { client } = mount(<DependencyObservations project="portal" />, () => response({ items: [edge()], total: 1, next_cursor: null, as_of: { at_unix_ms: AT } }));
    await screen.findByTestId('dependency-detail');
    revokeObservationReads(client);
    await screen.findByTestId('state-forbidden');
    expect(screen.queryByText('Build the customer portal')).toBeNull();
  });

  it('rechecks a later dependency page and clears facts after server-side revocation', async () => {
    const user = userEvent.setup();
    let revoked = false;
    const { calls } = mount(<DependencyObservations project="portal" />, (query) => {
      if (query.cursor && revoked) return error('Forbidden');
      return response({ items: [edge(query.cursor ? { work_title: 'Connect account management' } : {})], total: 2,
        next_cursor: query.cursor ? null : 'page-two', as_of: { at_unix_ms: AT } });
    });
    await screen.findByTestId('dependency-detail');
    await user.click(screen.getByRole('button', { name: '下一页' }));
    await waitFor(() => expect(screen.getByTestId('dependency-detail').textContent).toContain('Connect account management'));
    revoked = true;
    await waitFor(() => expect(screen.getByTestId('state-forbidden')).toBeTruthy(), { timeout: 9000 });
    expect(screen.queryByText('Connect account management')).toBeNull();
    expect(screen.queryByTestId('dependency-detail')).toBeNull();
    expect(calls.filter(query => query.cursor === 'page-two').length).toBeGreaterThanOrEqual(2);
  }, 15000);
});

describe('responsibility and handoffs', () => {
  const snapshot = { work_id: 'customer-portal', observation: {
    responsibility: { owner: 'person-internal', owner_name: 'Lin Chen', pending: { person_id: INTERNAL }, pending_name: 'Shen Lan',
      current_executor: { agent_id: 'agent-internal' }, executor_person_name: 'Zhou Ning', executor_agent_name: 'Delivery assistant' },
    claim: { lease_live: true }, progress: { origin: { actor_name: 'Review assistant' } },
  } };
  const handoff = { handoff_id: INTERNAL, kind: 'execution', effective_status: 'expired', pending: false, expired: true,
    from_display_name: 'Lin Chen', to_display_name: 'Shen Lan', updated_at_unix_ms: AT, expires_at_unix_ms: AT };

  it.each(['zh-CN', 'en-US'])('separates formal owner, pending assignment, executor, reservation and reporter in %s', async (language) => {
    await i18n.changeLanguage(language);
    const { container, calls } = mount(<WorkResponsibility project="portal" workId="customer-portal" snapshot={snapshot} />, () => response({
      items: [handoff], total: 1, next_cursor: null, as_of: { at_unix_ms: AT },
    }));
    await screen.findByText(language === 'zh-CN' ? /执行交接/ : /Execution handoff/);
    const panel = screen.getByTestId('work-responsibility');
    expect(panel.textContent).toContain('Lin Chen');
    expect(panel.textContent).toContain('Shen Lan');
    expect(panel.textContent).toContain('Zhou Ning');
    expect(panel.textContent).toContain('Delivery assistant');
    expect(panel.textContent).toContain('Review assistant');
    expect(panel.textContent).toContain(language === 'zh-CN' ? '已过期' : 'Expired');
    expect(panel.textContent).not.toContain(language === 'zh-CN' ? '待接手有效至' : 'Acceptance available until');
    expect(calls).toEqual([{ protocol_version: 1, op: 'handoff.list', work_id: 'customer-portal', handoff_state: 'all', limit: 10, max_context_bytes: 65536 }]);
    expectProductOnly(container);
  });

  it('keeps readable responsibility when handoff history is forbidden', async () => {
    const { client } = mount(<WorkResponsibility project="portal" workId="customer-portal" snapshot={snapshot} />, () => error('Forbidden'));
    await screen.findByTestId('state-forbidden');
    expect(screen.getByTestId('work-responsibility').textContent).toContain('Lin Chen');
    expect(client.getQueryCache().getAll().length).toBe(1);
  });

  it('shows missing names and an expired reservation without exposing internal identities', async () => {
    const { container } = mount(<WorkResponsibility project="portal" workId="customer-portal" snapshot={{ observation: {
      responsibility: { owner: 'person-internal', pending: null, current_executor: { agent_id: 'agent-internal' } }, claim: { lease_live: false },
    } }} />, () => response({ items: [], total: 0, next_cursor: null, as_of: { at_unix_ms: AT } }));
    await screen.findByText('还没有交接记录');
    expect(container.textContent).toContain('未提供姓名');
    expect(container.textContent).toContain('没有待接手指派');
    expect(container.textContent).toContain('占用已结束');
    expectProductOnly(container);
  });

  it('rechecks a later handoff page without clearing the readable responsibility', async () => {
    const user = userEvent.setup();
    let revoked = false;
    const { calls } = mount(<WorkResponsibility project="portal" workId="customer-portal" snapshot={snapshot} />, query => {
      if (query.cursor && revoked) return error('Forbidden');
      return response({ items: [{ ...handoff, to_display_name: query.cursor ? 'Restricted colleague' : 'Shen Lan' }],
        total: 2, next_cursor: query.cursor ? null : 'handoff-page-two', as_of: { at_unix_ms: AT } });
    });
    await screen.findByText('Lin Chen → Shen Lan');
    await user.click(screen.getByRole('button', { name: '下一页' }));
    await screen.findByText('Lin Chen → Restricted colleague');
    revoked = true;
    await waitFor(() => expect(screen.getByTestId('state-forbidden')).toBeTruthy(), { timeout: 9000 });
    expect(screen.queryByText('Lin Chen → Restricted colleague')).toBeNull();
    expect(screen.getByTestId('work-responsibility').textContent).toContain('Lin Chen');
    expect(calls.filter(query => query.cursor === 'handoff-page-two').length).toBeGreaterThanOrEqual(2);
  }, 15000);
});


describe('product-safe collaboration names and labels', () => {
  it.each(['zh-CN', 'en-US'])('guards dependency titles in visible and accessible text in %s', async (language) => {
    await i18n.changeLanguage(language);
    const { container } = mount(<DependencyObservations project="portal" />, () => response({
      items: [edge({ work_title: 'payload={"token":"synthetic-dependency"}',
        provider_title: 'client[secret]=synthetic-provider' })], total: 1, next_cursor: null, as_of: { at_unix_ms: AT },
    }));
    await screen.findByTestId('dependency-detail');
    const text = [container.textContent, ...[...container.querySelectorAll('[aria-label],[title]')]
      .flatMap((element) => [element.getAttribute('aria-label'), element.getAttribute('title')])].join(' ');
    expect(text).not.toMatch(/synthetic-dependency|synthetic-provider|payload|secret/);
    expectProductOnly(container);
    expect(container.textContent).toContain(language === 'zh-CN' ? '某项工作' : 'A work item');
  });

  it.each(['zh-CN', 'en-US'])('preserves responsibility and handoff state while withholding unsafe names in %s', async (language) => {
    await i18n.changeLanguage(language);
    const raw = 'client[secret]=synthetic-person';
    const snapshot = { observation: {
      responsibility: { owner: 'person', owner_name: raw, pending: { person_id: 'next-person' }, pending_name: raw,
        current_executor: { agent_id: 'agent' }, executor_person_name: raw, executor_agent_name: 'GLM4.6' },
      claim: { lease_live: true }, progress: { origin: { actor_name: raw } },
    } };
    const { container } = mount(<WorkResponsibility project="portal" workId="customer-portal" snapshot={snapshot} />, () => response({
      items: [{ handoff_id: INTERNAL, kind: 'execution', effective_status: 'accepted', pending: false, expired: false,
        from_display_name: raw, to_display_name: 'Grok4', updated_at_unix_ms: AT, expires_at_unix_ms: null }],
      total: 1, next_cursor: null, as_of: { at_unix_ms: AT },
    }));
    await screen.findByText(language === 'zh-CN' ? /已接手/ : /Accepted/);
    expect(container.textContent).toContain('GLM4.6');
    expect(container.textContent).toContain('Grok4');
    expect(container.textContent).toContain(language === 'zh-CN' ? '名称暂时无法显示' : 'Name cannot be displayed');
    expect(container.textContent).not.toContain('synthetic-person');
    expectProductOnly(container);
  });
});
