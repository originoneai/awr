import { describe, expect, it } from 'vitest';
import type { PortfolioProjectItem, TeamPortfolioResponse } from '../../lib/types';
import {
  buildDepartmentWorkbenchModel,
  portfolioClockOffsetUnixMs,
  portfolioReadMomentUnixMs,
  worstProjectHealth,
} from './department-model';

const NOW = Date.UTC(2026, 10, 19, 12, 0, 0); // 2026-11-19 12:00 UTC
const DAY = 86_400_000;

function completeProject(
  key: string,
  overrides: Partial<Extract<PortfolioProjectItem, { read: { status: 'complete' } }>> = {},
): PortfolioProjectItem {
  return {
    project_id: `id-${key}`,
    project_key: key,
    project_status: 'active',
    coordinator_epoch: 'epoch-1',
    project_revision: '1',
    responsible_person: null,
    directory: {
      registered: true,
      group: { id: `grp-${key}`, name: `Group ${key}` },
      departments: [{ id: 'dept-1', name: 'Engineering' }],
    },
    works: {
      total: 4,
      not_started: 1,
      in_progress: 1,
      delivered_unverified: 1,
      verified: 1,
      completed_without_independent_review: 0,
      retired: 0,
      hidden_work_count: 0,
    },
    waits: {
      decision: 0,
      review: 0,
      dependency: 0,
      oldest_open_unix_ms: null,
    },
    latest_update: {
      update_id: `upd-${key}`,
      period_kind: 'week',
      period_key: '2026-W47',
      starts_on: '2026-11-16',
      ends_on: '2026-11-22',
      status: 'ok',
      summary: `Summary for ${key}.`,
      author: { person_id: 'lead-1', display_name: 'Lead One' },
      recorded_at_unix_ms: NOW,
    },
    read: { status: 'complete' },
    ...overrides,
  };
}

function portfolioResponse(
  projects: PortfolioProjectItem[],
  overrides: Partial<TeamPortfolioResponse> = {},
): TeamPortfolioResponse {
  const notRead = projects.filter((p) => p.read.status === 'not_read').length;
  return {
    ok: true,
    result: {
      protocol_version: 1,
      op: 'portfolio.overview',
      selection_basis: 'member',
      data: {
        as_of: { at_unix_ms: NOW },
        execution_authorized: false,
        projects,
        total_projects: projects.length,
        truncated: false,
        next_cursor: null,
      },
    },
    read_at_unix_ms: NOW,
    coverage: {
      status: notRead > 0 ? 'partial' : 'complete',
      returned_projects: projects.length,
      total_projects: projects.length,
      not_read_projects: notRead,
      truncated: false,
      next_cursor: null,
    },
    freshness: {
      report_received: { status: 'not_measured' },
      authoritative: { status: 'measured', at_unix_ms: NOW },
      page_read: { status: 'measured', at_unix_ms: NOW },
    },
    ...overrides,
  };
}

describe('worstProjectHealth rollup', () => {
  it('always picks the worst status across projects and never averages', () => {
    expect(worstProjectHealth([{ health: 'ok' }, { health: 'ok' }])).toBe('ok');
    expect(worstProjectHealth([{ health: 'ok' }, { health: 'none' }])).toBe('none');
    expect(worstProjectHealth([{ health: 'ok' }, { health: 'none' }, { health: 'risk' }])).toBe('risk');
    expect(worstProjectHealth([{ health: 'ok' }, { health: 'risk' }, { health: 'off' }])).toBe('off');
    expect(worstProjectHealth([{ health: 'ok' }, { health: 'unknown' }])).toBe('none');
    expect(worstProjectHealth([])).toBe('none');
  });

  it('prevents a rosier recorded update from hiding a worse project status', () => {
    expect(worstProjectHealth([{ health: 'off' }, { health: 'ok' }], 'ok')).toBe('off');
    expect(worstProjectHealth([{ health: 'risk' }, { health: 'ok' }], 'ok')).toBe('risk');
    expect(worstProjectHealth([{ health: 'ok' }], 'risk')).toBe('risk');
  });
});

describe('department workbench model derivation', () => {
  it('marks projects overdue strictly past 7 days without a current-period update', () => {
    const sevenDays = completeProject('p-7d', {
      latest_update: {
        update_id: 'u-7',
        period_kind: 'week',
        period_key: '2026-W45',
        starts_on: '2026-11-02',
        ends_on: '2026-11-08',
        status: 'ok',
        summary: 'Old',
        author: null,
        recorded_at_unix_ms: NOW - 7 * DAY,
      },
    });
    const eightDays = completeProject('p-8d', {
      latest_update: {
        update_id: 'u-8',
        period_kind: 'week',
        period_key: '2026-W45',
        starts_on: '2026-11-02',
        ends_on: '2026-11-08',
        status: 'ok',
        summary: 'Older',
        author: null,
        recorded_at_unix_ms: NOW - 8 * DAY,
      },
    });
    const model = buildDepartmentWorkbenchModel(portfolioResponse([sevenDays, eightDays]), 'zh-CN', {
      readAtUnixMs: NOW,
      referenceUnixMs: NOW,
      timeZone: 'UTC',
    });
    expect(model.projects[0].health).toBe('none');
    expect(model.projects[0].late).toBe(false);
    expect(model.projects[1].health).toBe('none');
    expect(model.projects[1].late).toBe(true);
    expect(model.projects[1].elapsedDays).toBe(8);
  });

  it('merges delivered_unverified and completed_without_independent_review into reported', () => {
    const proj = completeProject('p-rep', {
      works: {
        total: 10,
        verified: 3,
        delivered_unverified: 2,
        completed_without_independent_review: 1,
        in_progress: 2,
        not_started: 2,
        retired: 0,
        hidden_work_count: 0,
      },
    });
    const model = buildDepartmentWorkbenchModel(portfolioResponse([proj]), 'zh-CN', {
      readAtUnixMs: NOW,
      referenceUnixMs: NOW,
      timeZone: 'UTC',
    });
    expect(model.projects[0].completion).toEqual({
      verified: 3,
      reported: 3,
      inProgress: 2,
      notStarted: 2,
    });
  });

  it('never substitutes latest_update author as responsible_person', () => {
    const proj = completeProject('p-no-owner', {
      responsible_person: null,
      latest_update: {
        update_id: 'u-1',
        period_kind: 'week',
        period_key: '2026-W47',
        starts_on: '2026-11-16',
        ends_on: '2026-11-22',
        status: 'ok',
        summary: 'All good',
        author: { person_id: 'M-999', display_name: 'Update Author Only' },
        recorded_at_unix_ms: NOW,
      },
    });
    const model = buildDepartmentWorkbenchModel(portfolioResponse([proj]), 'zh-CN', {
      readAtUnixMs: NOW,
      referenceUnixMs: NOW,
      timeZone: 'UTC',
    });
    expect(model.projects[0].leadName).toBeNull();
  });

  it('uses recorded department update sentence and viewer-authored byline when present', () => {
    const res = portfolioResponse([
      completeProject('p-1', { latest_update: { ...completeProject('p-1').read.status === 'complete' ? (completeProject('p-1') as Extract<PortfolioProjectItem, { read: { status: 'complete' } }>).latest_update! : null!, status: 'risk' } }),
    ], {
      department_facts: {
        viewer_person_id: 'M-010',
        department_update: {
          id: null,
          health: 'risk',
          sentence: 'Recorded department verdict.',
          author_person_id: 'M-010',
          author_name: 'Department Lead',
          written_at_unix_ms: NOW - 2 * DAY,
        },
      },
    });
    const model = buildDepartmentWorkbenchModel(res, 'zh-CN', {
      readAtUnixMs: NOW,
      referenceUnixMs: NOW,
      timeZone: 'UTC',
    });
    expect(model.verdict.source).toBe('recorded-update');
    expect(model.verdict.sentence).toBe('Recorded department verdict.');
    expect(model.verdict.byline).toBe('你 11 月 17 日在自己的 Agent 里写的 · 汇总时取最差的那个，不取平均');
  });

  it('falls back to other-authored byline when viewer does not match update author', () => {
    const res = portfolioResponse([completeProject('p-1')], {
      department_facts: {
        viewer_person_id: 'M-099',
        department_update: {
          id: 'U-9',
          health: 'ok',
          sentence: 'Another lead wrote this.',
          author_person_id: 'M-010',
          author_name: 'Dept Lead B',
          written_at_unix_ms: NOW - DAY,
        },
      },
    });
    const model = buildDepartmentWorkbenchModel(res, 'en-US', {
      readAtUnixMs: NOW,
      referenceUnixMs: NOW,
      timeZone: 'UTC',
    });
    expect(model.verdict.source).toBe('recorded-update');
    expect(model.verdict.byline).toContain('Dept Lead B');
    expect(model.verdict.byline).not.toContain('You wrote');
  });

  it('selects all deterministic generated verdict rules when no department update exists', () => {
    const empty = buildDepartmentWorkbenchModel(portfolioResponse([]), 'zh-CN', { referenceUnixMs: NOW, timeZone: 'UTC' });
    expect(empty.verdict.source).toBe('generated');
    expect(empty.verdict.sentence).toBe('当前授权范围内还没有部门项目。');
    expect(empty.verdict.byline).toContain('程序按各项目事实生成');

    const partial = buildDepartmentWorkbenchModel(portfolioResponse([
      completeProject('p-1'),
      { project_id: 'id-2', project_key: 'p-2', read: { status: 'not_read', reason: 'read_failed' } },
    ]), 'zh-CN', { referenceUnixMs: NOW, timeZone: 'UTC' });
    expect(partial.verdict.sentence).toContain('已读到 1 个项目');
    expect(partial.verdict.health).toBe('none');

    const offProj = completeProject('p-off');
    if (offProj.read.status === 'complete' && 'latest_update' in offProj && offProj.latest_update) offProj.latest_update.status = 'off';
    const offModel = buildDepartmentWorkbenchModel(portfolioResponse([offProj, completeProject('p-ok')]), 'zh-CN', { referenceUnixMs: NOW, timeZone: 'UTC' });
    expect(offModel.verdict.health).toBe('off');
    expect(offModel.verdict.sentence).toBe('2 个项目里 1 个偏离。');

    const riskProj = completeProject('p-risk');
    if (riskProj.read.status === 'complete' && 'latest_update' in riskProj && riskProj.latest_update) riskProj.latest_update.status = 'risk';
    const noUpdProj = completeProject('p-none', { latest_update: null });
    const bothModel = buildDepartmentWorkbenchModel(portfolioResponse([riskProj, noUpdProj, completeProject('p-ok')]), 'zh-CN', { referenceUnixMs: NOW, timeZone: 'UTC' });
    expect(bothModel.verdict.health).toBe('risk');
    expect(bothModel.verdict.sentence).toBe('3 个项目里 1 个有风险，1 个没有更新。');
    expect(bothModel.metrics[0].note?.text).toBe('1 个有风险，1 个没有更新');
    expect(bothModel.metrics[0].note?.tone).toBe('warn');

    const okModel = buildDepartmentWorkbenchModel(portfolioResponse([completeProject('p-ok')]), 'zh-CN', { referenceUnixMs: NOW, timeZone: 'UTC' });
    expect(okModel.verdict.health).toBe('ok');
    expect(okModel.verdict.sentence).toBe('1 个项目目前均按计划推进。');
    expect(okModel.metrics[0].note?.text).toBe('全部正常');
    expect(okModel.metrics[0].note?.tone).toBe('good');
  });

  it('keeps monthly verified, cross-group waits and pending unknown on live portfolio replies without inventing zeros', () => {
    const res = portfolioResponse([completeProject('p-live')]);
    const model = buildDepartmentWorkbenchModel(res, 'zh-CN', { readAtUnixMs: NOW, referenceUnixMs: NOW, timeZone: 'UTC' });
    expect(model.metrics[0].count).toBe(1);
    expect(model.metrics[1].count).toBeNull();
    expect(model.metrics[2].count).toBeNull();
    expect(model.metrics[3].count).toBeNull();
    expect(model.pending.count).toBeNull();
    expect(model.waits).toBeNull();
    expect(model.lessons).toBeNull();
  });

  it('computes monthly trend, waits oldest party, and pending singular vs plural wording', () => {
    const res = portfolioResponse([completeProject('p-1')], {
      department_facts: {
        monthly_verified: {
          current: { count: 1, items: [{ id: 'T-1', title: 'Task 1', project_key: 'p-1', project_title: 'P1', owner_name: 'Dev', verified_at_unix_ms: NOW }] },
          previous: { count: 3 },
        },
        cross_group_waits: [
          { id: 'w1', who: 'G1', on: 'G2', what: 'Review', kind: 'review', days: 2, started_at_unix_ms: NOW - 2 * DAY, behind: 3, task_id: 'T-1', task_title: 'Task 1', project_key: 'p-1' },
          { id: 'w2', who: 'G3', on: 'Partner X', what: 'Account', kind: 'external', days: 9, started_at_unix_ms: NOW - 9 * DAY, behind: 2, task_id: 'T-2', task_title: 'Task 2', project_key: 'p-1' },
        ],
        pending: [
          { id: 'D-1', kind: 'decision', title: 'Decide A', description: null, from: 'Lead', since_unix_ms: NOW - DAY, days: 1, project_key: 'p-1', project_title: 'P1', task_id: null, person_id: null, copy_text: null, agent_guidance: null },
        ],
      },
    });
    const model = buildDepartmentWorkbenchModel(res, 'zh-CN', { readAtUnixMs: NOW, referenceUnixMs: NOW, timeZone: 'UTC' });
    expect(model.metrics[1].count).toBe(1);
    expect(model.metrics[1].note).toEqual({ text: '上月 3 件', tone: 'bad', trend: 'down' });
    expect(model.metrics[2].count).toBe(2);
    expect(model.metrics[2].note?.text).toBe('最久 9 天，等Partner X');
    expect(model.metrics[3].count).toBe(1);
    expect(model.metrics[3].note?.text).toBe('等了 1 天');
  });

  it('handles usage visibility and missing-group notes honestly', () => {
    // No usage on live portfolio: names the missing groups from visible projects, shows no usage column.
    const noUsage = buildDepartmentWorkbenchModel(portfolioResponse([
      completeProject('p-1', { directory: { registered: true, group: { id: 'g1', name: 'Group Alpha' }, departments: [] } }),
      completeProject('p-2', { directory: { registered: true, group: { id: 'g2', name: 'Group Beta' }, departments: [] } }),
    ]), 'zh-CN', { referenceUnixMs: NOW, timeZone: 'UTC' });
    expect(noUsage.hasAnyProjectUsage).toBe(false);
    expect(noUsage.usageNote).toContain('Group Alpha');
    expect(noUsage.usageNote).toContain('Group Beta');

    // Partial usage with explicit collected + missing groups and one project carrying usage display text.
    const partialUsage = buildDepartmentWorkbenchModel(portfolioResponse([
      completeProject('p-1'),
      completeProject('p-2'),
    ], {
      department_facts: {
        usage: {
          collected_groups: ['Group Alpha'],
          missing_groups: ['Group Beta'],
          by_project: { 'p-1': { display_text: '14 小时' }, 'p-2': null },
        },
      },
    }), 'zh-CN', { referenceUnixMs: NOW, timeZone: 'UTC' });
    expect(partialUsage.hasAnyProjectUsage).toBe(true);
    expect(partialUsage.projects[0].usageDisplayText).toBe('14 小时');
    expect(partialUsage.projects[1].usageDisplayText).toBeNull();
    expect(partialUsage.usageNote).toBe('投入只采集到Group Alpha的用量，另外一个小组的缺失，所以这里不显示费用。');
  });

  it('treats a blank usage text as no usage, never as a cost figure', () => {
    const model = buildDepartmentWorkbenchModel(portfolioResponse([completeProject('p-1'), completeProject('p-2')], {
      department_facts: {
        usage: {
          collected_groups: ['Group Alpha'],
          missing_groups: [],
          by_project: { 'p-1': { display_text: '   ' }, 'p-2': { display_text: '' } },
        },
      },
    }), 'zh-CN', { referenceUnixMs: NOW, timeZone: 'UTC' });
    expect(model.projects.map((project) => project.usageDisplayText)).toEqual([null, null]);
    expect(model.hasAnyProjectUsage).toBe(false);
  });

  it('derives live blocker chains from portfolio waits without inventing task names', () => {
    const withWaits = completeProject('p-wait', {
      waits: { decision: 1, review: 2, dependency: 0, oldest_open_unix_ms: NOW - 2 * DAY },
    });
    const model = buildDepartmentWorkbenchModel(portfolioResponse([withWaits]), 'zh-CN', { referenceUnixMs: NOW, timeZone: 'UTC' });
    expect(model.chains).toHaveLength(2);
    expect(model.chains[0].tab).toBe('复核人手');
    expect(model.chains[1].tab).toBe('等待协调');

    const calm = buildDepartmentWorkbenchModel(portfolioResponse([completeProject('p-calm')]), 'zh-CN', { referenceUnixMs: NOW, timeZone: 'UTC' });
    expect(calm.chains).toHaveLength(0);
  });

  it('computes display-clock offsets from as_of and read_at', () => {
    const shifted = portfolioResponse([completeProject('p-1')], { read_at_unix_ms: NOW + 3 * DAY });
    expect(portfolioClockOffsetUnixMs(shifted)).toBe(-3 * DAY);
    expect(portfolioReadMomentUnixMs(shifted)).toBe(NOW);
  });
});
