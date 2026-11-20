import { describe, expect, it } from 'vitest';
import { createRequire } from 'node:module';
// Cross-boundary by design, the same way test/demo-organization.test.js loads the
// derivation: the page model must consume what the real bridge routes answer, so the
// canonical demo's project-list facts stay pinned to the shapes the page reads. The
// bridge is a CommonJS server module, so it loads through createRequire.
const nodeRequire = createRequire(import.meta.url);
const { createTeamBridge } = nodeRequire('../../../server/team-bridge.js') as {
  createTeamBridge: (options: Record<string, unknown>) => {
    routes: Record<string, (url: URL, body: unknown, req: unknown, res: unknown)
      => Promise<Record<string, unknown>>>;
  };
};
import fixture from '../../../fixtures/demo-organization.json';
import {
  deriveProjectListItem,
  projectListFreshness,
  summarizeProjects,
  type ProjectListEntry,
  type ProjectListModel,
} from './project-list-model';

const DAY_MS = 86_400_000;

/** The canonical demo's real route replies: the project directory plus one overview
 * model per ordinary project, exactly what the page queries. */
async function canonicalDemoReplies() {
  const bridge = createTeamBridge({
    demo: true, demoVariant: 'canonical', demoTimestampShiftMs: 0, now: () => fixture.now_unix_ms,
  });
  const req = { headers: {}, method: 'GET' };
  const res = { setHeader() {} };
  const call = (route: string, target: string) => bridge.routes[route](
    new URL(target, 'http://127.0.0.1'), undefined, req, res);
  const directory = await call('GET /api/team/projects', '/api/team/projects');
  expect(directory.ok).toBe(true);
  const entries = directory.projects as ProjectListEntry[];
  const models = new Map<string, ProjectListModel>();
  for (const entry of entries) {
    if (entry.takeover) continue;
    const model = await call('GET /api/team/overview-model', `/api/team/overview-model?project=${entry.key}`);
    expect(model.ok).toBe(true);
    models.set(entry.key, model as unknown as ProjectListModel);
  }
  return { entries, models };
}

describe('the canonical demo drives the project list with the prototype facts', () => {
  it('derives the four prototype rows: risk, ok, no update and takeover', async () => {
    const { entries, models } = await canonicalDemoReplies();
    expect(entries.map((entry) => entry.key)).toEqual(['P-001', 'P-002', 'P-003', 'P-004']);
    const items = entries.map((entry) => deriveProjectListItem(entry, models.get(entry.key), 'UTC'));
    expect(items.map((item) => item.health)).toEqual(['risk', 'ok', 'none', 'info']);
    expect(items.map((item) => item.leadName)).toEqual(['周宁', '吴桐', '郑岚', null]);
    expect(items.map((item) => item.groupName)).toEqual(['应用小组', '平台小组', '交付小组', null]);
    // The recorded sentences are the prototype's, the third row's age is exactly the
    // prototype's 12 days, and the takeover row carries its deterministic facts.
    expect(items[0].summary).toEqual({ kind: 'update', text: '接口第 3 版的复核拖了两天，页面联调在等。' });
    expect(items[1].summary).toEqual({ kind: 'update', text: '试运行第二周，没有新问题。' });
    expect(items[2].summary).toEqual({ kind: 'last-update', days: 12 });
    expect(items[3].summary).toEqual({ kind: 'takeover', workstreams: 4, verified: 0, questions: 4 });
    expect(items[3].showTakeoverCompletion).toBe(true);
    // The update column's whole-day ages: 今天, 3 天前, 12 天前, 刚刚.
    expect(items.map((item) => Math.floor((fixture.now_unix_ms - (item.updatedAtUnixMs ?? 0)) / DAY_MS)))
      .toEqual([0, 3, 12, 0]);
    // 该更新了 belongs to the third row alone: only it is late.
    expect(items.filter((item) => item.late).map((item) => item.entry.key)).toEqual(['P-003']);
    const summary = summarizeProjects(items);
    expect(summary.all.count).toBe(4);
    expect(summary.risk.count).toBe(1);
    expect(summary.noUpdate.count).toBe(1);
    expect(summary.takeover.count).toBe(1);
    for (const group of [summary.all, summary.risk, summary.noUpdate, summary.takeover]) {
      expect(group.unknownItems).toEqual([]);
    }
    // The aggregated freshness is measured on every plane after the clock decoupling.
    const freshness = projectListFreshness([...models.values()]);
    expect(freshness.page_read.status).toBe('measured');
    expect(freshness.authoritative.status).toBe('measured');
  });
});
