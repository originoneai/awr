import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import i18n from '../i18n';
import type { OverviewMember, OverviewTask } from '../core/overview/types';
import { buildSearchIndex, searchIndex, type SearchSource } from './search-index';
import { searchSourcesFromReads } from './search-sources';
import type { TeamOverviewModelResponse, TeamProjectsResponse } from './types';

const empty = { coverage: 'complete', items: [] } as const;

function model(tasks: readonly OverviewTask[], members: readonly OverviewMember[], coverage: 'complete' | 'partial' | 'unread' = 'complete'): TeamOverviewModelResponse {
  return {
    ok: true,
    project: 'demo',
    read_at_unix_ms: 1761840000000,
    input: {
      tasks: coverage === 'complete'
        ? { coverage, items: tasks }
        : coverage === 'partial'
          ? { coverage, items: tasks, reason: 'truncated' }
          : { coverage, items: tasks, reason: 'not-read' },
      dependencies: empty,
      decisions: empty,
      reviewRounds: empty,
      handoffs: empty,
      approvals: { coverage: 'unread', reason: 'not-read' },
      members: { coverage: 'complete', items: members },
      activity: empty,
      options: { nowUnixMs: 1761840000000, stalledAfterDays: 3 },
    },
    coverage: {} as TeamOverviewModelResponse['coverage'],
    freshness: {
      report_received: { status: 'not_measured' },
      authoritative: { status: 'measured' },
      page_read: { status: 'measured', at_unix_ms: 1761840000000 },
    },
  };
}

const projects: TeamProjectsResponse = {
  ok: true,
  projects: [
    { key: 'demo', title: '交付项目', role: 'project_admin' },
    { key: 'second', title: '交付项目', role: 'project_admin' },
  ],
};

beforeEach(async () => i18n.changeLanguage('zh-CN'));
afterEach(async () => i18n.changeLanguage('zh-CN'));

describe('search source adaptation', () => {
  it('uses only records present in authorized replies and adds no frontend permission guesses', () => {
    const sources = searchSourcesFromReads({
      projects,
      projectKey: 'demo',
      overview: model([{ id: 'authorized', title: '名称含受限字样的授权任务' }], []),
    });
    const index = buildSearchIndex(sources);
    expect(searchIndex(index, '未返回的无权任务')).toEqual([]);
    expect(searchIndex(index, '授权任务')).toHaveLength(1);
    expect(index.items.some((entry) => entry.title === '未返回的无权任务')).toBe(false);
    expect(index.items.filter((entry) => entry.kind === 'project')).toHaveLength(2);
  });

  it('distinguishes duplicate titles by owner and adds a neutral ordinal when all other meta is equal', () => {
    const sources = searchSourcesFromReads({
      projects,
      projectKey: 'demo',
      overview: model([
        { id: 'one', title: '同名任务', ownerPersonId: 'person-one' },
        { id: 'two', title: '同名任务', ownerPersonId: 'person-two' },
        { id: 'three', title: '同名任务', ownerPersonId: 'person-one' },
      ], [
        { personId: 'person-one', displayName: '甲成员' },
        { personId: 'person-two', displayName: '乙成员' },
      ]),
    });
    const tasks = sources.find((source) => source.kind === 'task')?.items ?? [];
    expect(new Set(tasks.map((entry) => entry.meta)).size).toBe(3);
    expect(tasks[0]?.meta).toContain('甲成员');
    expect(tasks[1]?.meta).toContain('乙成员');
    expect(tasks[0]?.meta).toContain('第 1 项');
    expect(tasks[2]?.meta).toContain('第 2 项');
    const duplicateProjects = sources.find((source) => source.kind === 'project')?.items ?? [];
    expect(new Set(duplicateProjects.map((entry) => entry.meta)).size).toBe(2);
  });

  it('keeps partial list coverage and does not expose raw identifiers as fallback names', () => {
    const sources = searchSourcesFromReads({
      projects: { ok: true, projects: [{ key: 'demo', title: 'demo', role: 'unlisted-role' }] },
      projectKey: 'demo',
      overview: model([{ id: 'raw-work-id', title: 'raw-work-id', ownerPersonId: 'raw-person-id' }], [
        { personId: 'raw-person-id', displayName: 'raw-person-id' },
      ], 'partial'),
    });
    expect(sources.find((source) => source.kind === 'task')?.coverage).toBe('partial');
    const visibleText = sources.flatMap((source) => source.items).map((entry) => `${entry.title} ${entry.meta}`).join(' ');
    expect(visibleText).not.toContain('raw-work-id');
    expect(visibleText).not.toContain('raw-person-id');
    expect(visibleText).not.toContain('demo');
    expect(visibleText).not.toContain('unlisted-role');
    expect(visibleText).not.toMatch(/\bundefined\b|\bnull\b|\bNaN\b/);
  });

  it('does not index unread slices even if a malformed caller attaches records to them', () => {
    const sources = searchSourcesFromReads({
      projects,
      projectKey: 'demo',
      overview: model([{ id: 'not-read', title: '未读取的任务' }], [], 'unread'),
    });
    const index = buildSearchIndex(sources);
    expect(searchIndex(index, '未读取')).toEqual([]);
    expect(sources.find((source) => source.kind === 'task')?.coverage).toBe('unread');
  });

  it('treats a reply without composition input as unread instead of throwing', () => {
    const legacyReply = { ok: true, project: 'demo' } as TeamOverviewModelResponse;
    const sources = searchSourcesFromReads({ projects, projectKey: 'demo', overview: legacyReply });
    expect(sources.find((source) => source.kind === 'task')?.coverage).toBe('unread');
    expect(sources.find((source) => source.kind === 'person')?.coverage).toBe('unread');
    expect(() => buildSearchIndex(sources)).not.toThrow();
  });

  it('accepts future goal, document and lesson sources without changing the adapter shape', () => {
    const supplemental = (kind: 'goal' | 'document' | 'lesson'): SearchSource => ({
      kind,
      coverage: 'complete',
      items: [{ key: kind, kind, title: `${kind} 资料`, meta: '合成范围', path: `/${kind}` }],
    });
    const sources = searchSourcesFromReads({
      projects,
      projectKey: 'demo',
      overview: model([], []),
      supplementalSources: { goal: supplemental('goal'), document: supplemental('document'), lesson: supplemental('lesson') },
    });
    expect(sources.map((source) => source.kind)).toEqual(['project', 'task', 'person', 'goal', 'document', 'lesson']);
    expect(sources.slice(3).every((source) => source.coverage === 'complete')).toBe(true);
  });
});
