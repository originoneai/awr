import assert from 'node:assert/strict';
import { createEqual, settle } from './shared.mjs';

/* Prototype metric basis: members() .head 24/16 desktop and 16/12 mobile;
   .lead 20/28 (18/26 mobile) with 20px bottom margin; .block radius 16 and
   .block-head 48px with 0 20 padding; .t th 40px/12px; .av 28px; .who gap 10;
   .chip 22px, 12px text, 6px gap/radius and 0 8 padding; .chevc width 36;
   .pnote 12px 20px and 12px/16px. */
export async function membersGeometry(page) {
  await page.getByTestId('member-table').waitFor();
  return page.evaluate(() => {
    const read = (selector) => {
      const element = document.querySelector(selector);
      if (!element) return null;
      const style = getComputedStyle(element);
      const rect = element.getBoundingClientRect();
      return {
        width: rect.width,
        height: rect.height,
        paddingTop: style.paddingTop,
        paddingBottom: style.paddingBottom,
        paddingLeft: style.paddingLeft,
        paddingRight: style.paddingRight,
        marginBottom: style.marginBottom,
        gap: style.gap,
        borderRadius: style.borderRadius,
        fontSize: style.fontSize,
        lineHeight: style.lineHeight,
        fontWeight: style.fontWeight,
      };
    };
    return {
      header: read('.obs-page-header'),
      lead: read('.members-lead'),
      section: read('.members-section'),
      sectionHead: read('.members-section .ant-card-head'),
      sectionTitle: read('.members-section .obs-section-title'),
      tableHead: read('.members-table thead th'),
      firstTableHead: read('.members-table thead th:first-child'),
      avatar: read('.members-table tbody .members-avatar'),
      who: read('.members-table tbody .members-who'),
      chip: read('.members-table tbody .members-chip'),
      navigation: read('.members-table tbody .obs-table-arrow-column'),
      note: read('.members-pnote'),
      borderCollapse: getComputedStyle(document.querySelector('.members-table table')).borderCollapse,
      fixedArrows: document.querySelectorAll('.members-table .ant-table-cell-fix-end').length,
      count: document.querySelector('.members-section .obs-section-count')?.textContent ?? null,
      rows: document.querySelectorAll('.members-table tbody tr.ant-table-row').length,
      headers: [...document.querySelectorAll('.members-table thead th')].map((element) => element.textContent).filter(Boolean),
    };
  });
}

export function checkMembersGeometry(metric, compact) {
  const failures = [];
  const equal = createEqual(failures);
  equal(metric.header?.paddingTop, compact ? 16 : 24, 'members header top padding');
  equal(metric.header?.paddingBottom, compact ? 12 : 16, 'members header bottom padding');
  equal(metric.lead?.fontSize, compact ? 18 : 20, 'members lead font size');
  equal(metric.lead?.lineHeight, compact ? 26 : 28, 'members lead line height');
  equal(metric.lead?.fontWeight, 500, 'members lead weight');
  equal(metric.lead?.marginBottom, 20, 'members lead bottom margin');
  equal(metric.section?.borderRadius, 16, 'members section radius');
  equal(metric.sectionHead?.height, 48, 'members section heading height');
  equal(metric.sectionHead?.paddingLeft, 20, 'members section heading padding');
  equal(metric.sectionTitle?.fontSize, 14, 'members section title size');
  equal(metric.sectionTitle?.lineHeight, 20, 'members section title line height');
  equal(metric.tableHead?.height, 40, 'members table heading height');
  equal(metric.tableHead?.fontSize, 12, 'members table heading font size');
  equal(metric.firstTableHead?.paddingLeft, compact ? 16 : 20, 'members first table heading padding');
  equal(metric.avatar?.width, 28, 'members avatar width');
  equal(metric.avatar?.height, 28, 'members avatar height');
  equal(metric.who?.gap, 10, 'members avatar/name gap');
  if (!compact) {
    equal(metric.chip?.height, 22, 'members Agent chip height');
    equal(metric.chip?.fontSize, 12, 'members Agent chip font size');
    equal(metric.chip?.paddingLeft, 8, 'members Agent chip padding');
    equal(metric.chip?.gap, 6, 'members Agent chip gap');
    equal(metric.chip?.borderRadius, 6, 'members Agent chip radius');
  }
  equal(metric.navigation?.width, 36, 'members navigation column width');
  equal(metric.note?.paddingTop, 12, 'members note vertical padding');
  equal(metric.note?.paddingLeft, 20, 'members note horizontal padding');
  equal(metric.note?.fontSize, 12, 'members note font size');
  equal(metric.note?.lineHeight, 16, 'members note line height');
  equal(metric.count, metric.rows, 'members section count versus visible rows');
  const expectedHeaders = compact
    ? ['成员', '小组 · 职责', '现在在做', '等本人处理的']
    : ['成员', '小组 · 职责', '在哪些项目', '现在在做', 'Agent 连接', '等本人处理的'];
  if (JSON.stringify(metric.headers) !== JSON.stringify(expectedHeaders)) failures.push(`members table headings differ: ${JSON.stringify(metric.headers)}`);
  if (metric.borderCollapse !== 'collapse') failures.push(`members table border model: ${metric.borderCollapse}`);
  if (metric.fixedArrows !== 0) failures.push(`members navigation must scroll with the prototype table, fixed cells: ${metric.fixedArrows}`);
  return failures;
}

/* The canonical variant also serves the organization-wide directory the prototype's members
   page reads: eight members, three groups, recorded duties, and each person's queue count
   equal to the prototype (周宁 1、林悦 1、许宁 3, everyone else none). These assertions are
   the page-level oracle; a red count means the demo facts or the queue rule drifted. */
export async function recordCanonicalMembers(page) {
  await settle(page);
  await page.getByTestId('member-table').waitFor();
  const rows = page.locator('.members-table tbody tr.ant-table-row');
  assert.equal(await rows.count(), 8, 'the canonical members table has the prototype eight rows');
  const names = [];
  const duties = [];
  for (let index = 0; index < 8; index += 1) {
    names.push((await rows.nth(index).locator('.members-name').innerText()).trim());
    duties.push((await rows.nth(index).locator('.members-group-cell .members-small').innerText()).trim());
  }
  assert.deepEqual(names, ['周宁', '林悦', '陈宇', '许宁', '吴桐', '赵衡', '郑岚', '何川'],
    'the eight member names must equal the prototype order');
  assert.deepEqual(duties, ['小组负责人', '开发', '开发', '复核', '小组负责人', '开发', '小组负责人', '开发'],
    'the recorded duties must equal the prototype');
  const pending = [];
  for (let index = 0; index < 8; index += 1) {
    pending.push((await rows.nth(index).locator('[data-testid^="member-pending-"]').innerText()).trim());
  }
  assert.deepEqual(pending, ['1 件', '1 件', '没有', '3 件', '没有', '没有', '没有', '没有'],
    'the waiting-on-this-person counts must equal the prototype');
  const summary = (await page.getByTestId('members-summary').innerText()).replace(/\s+/g, '');
  assert.ok(summary.includes('8位成员，3个小组'),
    `the members lead must count eight members and three groups (read ${summary})`);
  assert.equal((await rows.nth(3).locator('[data-testid^="member-doing-"]').innerText()).trim(),
    '复核“接口第 3 版”等 3 件',
    'the recorded reviewer reads their review queue like the prototype');
  assert.equal((await rows.nth(7).locator('[data-testid^="member-doing-"]').innerText()).trim(),
    '在甲方的测试环境走通 5 步接入',
    'a waiting task without an agent still reads as the doing work, like the prototype');
  const wide = (page.viewportSize()?.width ?? 0) >= 1240;
  if (wide) {
    assert.equal((await rows.nth(3).locator('[data-testid^="member-projects-"]').innerText()).trim(),
      '订单协作平台、统一通知服务',
      'a member of two projects reads both in one row like the prototype');
  }
  return { names, duties, pending, summary };
}

export default {
  name: 'members',
  reportKey: 'membersGeometry',
  matches: (route) => route.name === 'members',
  measure: membersGeometry,
  check: checkMembersGeometry,
  canonical: {
    reportKey: 'canonicalMembersRecords',
    route: {
      name: 'members-canonical',
      path: '/members',
      // The prototype's members page is organization-wide: without a selected project the
      // canonical bridge serves that directory and the page reads it in one request.
      run: async (page, recordCanonical) => {
        await settle(page);
        recordCanonical(await recordCanonicalMembers(page));
      },
    },
  },
};
