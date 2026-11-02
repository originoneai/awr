import { readdirSync, readFileSync, writeFileSync } from 'node:fs';
import { act } from '@testing-library/react';
import { afterAll, afterEach, describe, expect, it, vi } from 'vitest';
import i18n from './i18n';
import BASELINE from './test-fixtures/human-language-baseline.json';
import { findHumanLanguageViolations, visibleTextParts, withinHumanLanguageBaseline } from './lib/human-language-guard';
import { REAL_API_WORK, realShapedActivity, realShapedOverview, realShapedSnapshot, realShapedWorkInbox, realShapedWorkNext, type RealMoment } from './test-fixtures/real-shaped';
import {
  DEMO_OVERVIEW,
  DEMO_PROJECTS,
  DEMO_REVIEW,
  renderApp,
} from './test-utils';

// The pages read the demo data with a rehearsal run's real values laid over it (src/test-fixtures/real-shaped.ts): the works
// are named as the service names them and carry the checkpoints their reporters wrote. A second pair of pages reads the same
// work after its delivery, where the reporter's sentences carry receipt ids and commit hashes.
const PAGES = {
  account: '/account?scope=demo',
  projects: '/projects',
  workbench: `/workbench?scope=demo&work=${REAL_API_WORK}`,
  workbenchDelivered: `/workbench?scope=demo&work=${REAL_API_WORK}`,
  overview: '/projects/demo?tab=overview',
  plan: `/projects/demo?tab=plan&work=${REAL_API_WORK}`,
  planDelivered: `/projects/demo?tab=plan&work=${REAL_API_WORK}`,
  acceptance: `/projects/demo?tab=acceptance&work=${REAL_API_WORK}`,
  knowledge: '/projects/demo?tab=knowledge',
  activity: '/activity?project=demo',
  agentConnect: '/agent-connect?scope=demo',
  placeholders: '/members',
} as const;
const MOMENTS: Partial<Record<keyof typeof PAGES, RealMoment>> = { workbenchDelivered: 'delivered', planDelivered: 'delivered' };
const STATES = ['data', 'empty', 'error', 'partial', 'revoked', 'unknown'] as const;
const LANGUAGES = ['zh-CN', 'en-US'] as const;
type State = (typeof STATES)[number];

function json(payload: unknown) {
  return new Response(JSON.stringify(payload), { status: 200, headers: { 'content-type': 'application/json' } });
}

function installRealShapeMock(state: State, moment: RealMoment) {
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
        ...realShapedOverview(),
        identity: {
          actor_id: 'observer-x', client_id: 'observer-client-x', role: 'reader', business_roles: ['observer'],
          can_manage_members: false, can_read_project_audit: true, membership_action_ceiling: ['work.read'],
        },
        mcp_url: 'http://127.0.0.1:9000/v1/projects/demo/mcp',
      });
    }
    if (state === 'partial') return json(failure);
    if (url.includes('/api/team/activity')) {
      const activity = realShapedActivity();
      if (state === 'empty') return json({ ...activity, data: { ...activity.data, items: [] } });
      if (state === 'unknown') {
        return json({
          ...activity,
          data: {
            ...activity.data,
            items: activity.data.items.map((item) => ({ ...item, result: 'unknown' })),
          },
        });
      }
      return json(activity);
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
        const next = realShapedWorkNext();
        return json(state === 'empty'
          ? { ...next, result: { ...next.result, data: { ...next.result.data, items: [], resume: [] } } }
          : next);
      }
      if (query.op === 'work.inbox') {
        const inbox = realShapedWorkInbox();
        return json(state === 'empty'
          ? { ...inbox, result: { ...inbox.result, data: { ...inbox.result.data, items: [] } } }
          : inbox);
      }
      if (query.op === 'work.snapshot') {
        const snapshot = realShapedSnapshot(query.work_id ?? REAL_API_WORK, moment) as unknown as { result: { data: Record<string, unknown> } };
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
      installRealShapeMock(state, MOMENTS[page as keyof typeof PAGES] ?? 'in-review');
      renderApp(path);
      await act(async () => { await new Promise((resolve) => setTimeout(resolve, 40)); });
      const violations = findHumanLanguageViolations(document.body);
      const count = violations.length;
      const key = language === 'zh-CN' ? `${page}/${state}` : `${page}/${state}@${language}`;
      generatedBaseline[key] = count;
      const baseline = BASELINE[key as keyof typeof BASELINE] ?? 0;
      expect(
        withinHumanLanguageBaseline(count, baseline),
        `${key}: actual violations (${count}) differ from baseline (${baseline}) [${violations.map((violation) => `${violation.id}: ${violation.match}`).join('; ')}]; regenerate the baseline after an intentional change`,
      ).toBe(true);
    },
  );
});

// What the reporters of the run wrote, as the pages show it in English: the sentences are the reporter's own, long identifiers are
// cut to a short start, and the machine names a vocabulary knows are in words. Nothing else on these pages is exempt from the guard.
const IN_REVIEW = {
  next: 'Wait for independent review of round 01M6AVQ8…. Do not self-approve or merge to main.',
  report: [
    'Status and keyword filters are on branch issue-filter-api at 5f7eb2fe…. npm test passed 5 of 5. Local content and tests passed. Review is open.',
    'filterIssues combines status and keyword without writing issue records.',
    'Checks cover an empty list, no match, and combined filters.',
    'Published refs/heads/issue-filter-api and submitted the report plus both manifest files.',
  ],
};
const DELIVERED = {
  next: 'Leave issue-filter-page available for self-claim. Eligible assignees are lin and mei. The page has no implementation, review, or delivery.',
  report: [
    'Fast-forwarded refs/heads/main from a5163424… to 5f7eb2fe… and finalized issue-filter-api. Completion receipt 01M6B189…. Source publication 01M6B189… is confirmed.',
    'Prepare mainline integration 01M6B0NN… confirmed applied',
    'Finalize delivery receipt 01M6B189…',
    'origin/main is 5f7eb2fe…',
  ],
};
const REPORTER_SENTENCES = {
  plan: [IN_REVIEW.next, ...IN_REVIEW.report],
  planDelivered: [DELIVERED.next, ...DELIVERED.report],
  workbench: [IN_REVIEW.next, ...IN_REVIEW.report],
  workbenchDelivered: [DELIVERED.next, ...DELIVERED.report],
  overview: [IN_REVIEW.next],
} as const;

describe('the real-shaped pages show a reporter as written, with long identifiers cut and machine names in words', () => {
  it('has the mark only in the files that show a reporter\'s sentence, because a marked element is exempt from the word lists', () => {
    const marked = (readdirSync('src', { recursive: true }) as string[])
      .filter((file) => file.endsWith('.tsx') && !file.endsWith('.test.tsx'))
      .filter((file) => readFileSync(`src/${file}`, 'utf8').includes('data-reporter-text'));
    expect(marked.sort()).toEqual(['components/ProgressReport.tsx', 'components/WorkDetailPanel.tsx', 'pages/ProjectOverview.tsx', 'pages/WorkbenchPage.tsx']);
  });

  it.each(Object.keys(REPORTER_SENTENCES) as Array<keyof typeof REPORTER_SENTENCES>)('%s exempts exactly the reporter\'s sentences', async (page) => {
    await act(async () => { await i18n.changeLanguage('en-US'); });
    installRealShapeMock('data', MOMENTS[page] ?? 'in-review');
    renderApp(PAGES[page]);
    await act(async () => { await new Promise((resolve) => setTimeout(resolve, 40)); });
    expect(visibleTextParts(document.body).reporter.split('\n').sort()).toEqual([...REPORTER_SENTENCES[page]].sort());
  });
});
