import { writeFileSync } from 'node:fs';
import { act } from '@testing-library/react';
import { afterAll, afterEach, describe, expect, it, vi } from 'vitest';
import i18n from './i18n';
import BASELINE from './test-fixtures/human-language-baseline.json';
import { defaultVisibleText, findForbiddenVisibleText, withinHumanLanguageBaseline } from './lib/human-language-guard';
import {
  DEMO_ACTIVITY,
  DEMO_OVERVIEW,
  DEMO_PROJECT_OVERVIEW,
  DEMO_PROJECTS,
  DEMO_REVIEW,
  DEMO_WORK_INBOX,
  DEMO_WORK_NEXT,
  demoWorkSnapshot,
  renderApp,
} from './test-utils';

const PAGES = {
  account: '/account?scope=demo',
  projects: '/projects',
  workbench: '/workbench?scope=demo',
  overview: '/projects/demo?tab=overview',
  plan: '/projects/demo?tab=plan&work=W1',
  acceptance: '/projects/demo?tab=acceptance&work=W1',
  knowledge: '/projects/demo?tab=knowledge',
  activity: '/activity?project=demo',
  agentConnect: '/agent-connect?scope=demo',
  placeholders: '/members',
} as const;
const STATES = ['data', 'empty', 'error', 'partial', 'revoked', 'unknown'] as const;
const LANGUAGES = ['zh-CN', 'en-US'] as const;
type State = (typeof STATES)[number];

function json(payload: unknown) {
  return new Response(JSON.stringify(payload), { status: 200, headers: { 'content-type': 'application/json' } });
}

function installRealShapeMock(state: State) {
  const failure = { ok: false, error: { code: 'ReadTimeout', message: 'synthetic read failure' } };
  const revoked = { ok: false, error: { code: 'Unauthenticated', message: 'revoked' } };
  vi.stubGlobal('fetch', vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
    const url = String(input);
    if (state === 'revoked') return json(revoked);
    if (state === 'error') return json(failure);
    if (url.includes('/api/team/projects')) {
      return json(state === 'empty' ? { ...DEMO_PROJECTS, projects: [] } : DEMO_PROJECTS);
    }
    if (url.includes('/api/team/overview')) {
      if (state === 'empty') return json({ ...DEMO_OVERVIEW, works: [], workstreams: [] });
      return json({
        ...DEMO_PROJECT_OVERVIEW,
        identity: {
          actor_id: 'observer-x', client_id: 'observer-client-x', role: 'reader', business_roles: ['observer'],
          can_manage_members: false, can_read_project_audit: true, membership_action_ceiling: ['work.read'],
        },
        mcp_url: 'http://127.0.0.1:9000/v1/projects/demo/mcp',
      });
    }
    if (state === 'partial') return json(failure);
    if (url.includes('/api/team/activity')) {
      if (state === 'empty') return json({ ...DEMO_ACTIVITY, data: { ...DEMO_ACTIVITY.data, items: [] } });
      if (state === 'unknown') {
        return json({
          ...DEMO_ACTIVITY,
          data: {
            ...DEMO_ACTIVITY.data,
            items: DEMO_ACTIVITY.data.items.map((item) => ({ ...item, result: 'unknown' })),
          },
        });
      }
      return json(DEMO_ACTIVITY);
    }
    if (url.includes('/api/team/read')) {
      const query = JSON.parse(String(init?.body)).query;
      if (query.op === 'capabilities') {
        return json({
          ok: true,
          result: {
            protocol_version: 1,
            data: {
              identity: {
                actor_id: 'observer-x', client_id: 'observer-client-x', role: 'reader', business_roles: ['observer'],
                can_manage_members: false, can_read_project_audit: true, membership_action_ceiling: ['work.read'],
              },
            },
          },
        });
      }
      if (query.op === 'work.next') {
        return json(state === 'empty'
          ? { ...DEMO_WORK_NEXT, result: { ...DEMO_WORK_NEXT.result, data: { ...DEMO_WORK_NEXT.result.data, items: [], resume: [] } } }
          : DEMO_WORK_NEXT);
      }
      if (query.op === 'work.inbox') {
        return json(state === 'empty'
          ? { ...DEMO_WORK_INBOX, result: { ...DEMO_WORK_INBOX.result, data: { ...DEMO_WORK_INBOX.result.data, items: [] } } }
          : DEMO_WORK_INBOX);
      }
      if (query.op === 'work.snapshot') {
        const snapshot = demoWorkSnapshot(query.work_id ?? 'W1') as unknown as { result: { data: Record<string, unknown> } };
        if (state === 'empty') {
          snapshot.result.data = {
            ...snapshot.result.data,
            observation: undefined,
            guidance: undefined,
            required_specs: [],
            authorized_readable_refs: [],
          };
        }
        return json(snapshot);
      }
      if (query.op === 'review.inspect') {
        return json(state === 'empty'
          ? { ...DEMO_REVIEW, result: { ...DEMO_REVIEW.result, data: { review: null } } }
          : DEMO_REVIEW);
      }
      return json({ ok: false, error: { code: 'Unavailable', message: 'synthetic partial evidence' } });
    }
    return json({ ok: false, error: { code: 'NoRoute', message: url } });
  }));
}

const generatedBaseline: Record<string, number> = {};

afterAll(() => {
  if (process.env.UPDATE_HUMAN_BASELINE === '1') {
    writeFileSync('src/test-fixtures/human-language-baseline.json', `${JSON.stringify(generatedBaseline, null, 2)}\n`);
  }
});

afterEach(async () => {
  await act(async () => { await i18n.changeLanguage('zh-CN'); });
});

describe('human-language violation ratchet', () => {
  it('requires the exact recorded baseline in both directions', () => {
    expect(withinHumanLanguageBaseline(1, 0)).toBe(false);
    expect(withinHumanLanguageBaseline(0, 1)).toBe(false);
    expect(withinHumanLanguageBaseline(0, 0)).toBe(true);
  });

  it('records one entry for every page, state and language, and nothing else', () => {
    const expected = Object.keys(PAGES).flatMap((page) => LANGUAGES.flatMap((language) => STATES.map((state) => (language === 'zh-CN' ? `${page}/${state}` : `${page}/${state}@${language}`))));
    expect(Object.keys(BASELINE).sort()).toEqual(expected.sort());
  });

  it.each(Object.entries(PAGES).flatMap(([page, path]) => LANGUAGES.flatMap((language) => STATES.map((state) => [page, path, state, language] as const))))(
    '%s / %s / %s / %s matches its recorded debt exactly',
    async (page, path, state, language) => {
      await act(async () => { await i18n.changeLanguage(language); });
      installRealShapeMock(state);
      renderApp(path);
      await act(async () => { await new Promise((resolve) => setTimeout(resolve, 40)); });
      const count = findForbiddenVisibleText(defaultVisibleText(document.body)).length;
      const key = language === 'zh-CN' ? `${page}/${state}` : `${page}/${state}@${language}`;
      generatedBaseline[key] = count;
      const baseline = BASELINE[key as keyof typeof BASELINE] ?? 0;
      expect(
        withinHumanLanguageBaseline(count, baseline),
        `${key}: actual violations (${count}) differ from baseline (${baseline}); regenerate the baseline after an intentional change`,
      ).toBe(true);
    },
  );
});
