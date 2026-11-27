import assert from 'node:assert/strict';
import { createEqual, settle } from './shared.mjs';

/* Prototype projects(): .head 24/16 desktop and 16/12 mobile; .lead 20/28 with a
   20px bottom margin (18/26 mobile); .block radius 16; .block-head 48px and 0 20;
   .t th 40px/12px; first/last cell inset 20px desktop and 16px mobile. */
export async function projectsGeometry(page) {
  await page.locator('[data-testid="page-projects"], [data-testid="state-forbidden"]').first().waitFor();
  return page.evaluate(() => {
    const read = (selector) => {
      const element = document.querySelector(selector);
      if (!element) return null;
      const style = getComputedStyle(element);
      const rect = element.getBoundingClientRect();
      return {
        height: rect.height,
        paddingTop: style.paddingTop,
        paddingBottom: style.paddingBottom,
        paddingLeft: style.paddingLeft,
        marginBottom: style.marginBottom,
        borderRadius: style.borderRadius,
        fontSize: style.fontSize,
        lineHeight: style.lineHeight,
        fontWeight: style.fontWeight,
        fontVariantNumeric: style.fontVariantNumeric,
      };
    };
    return {
      forbidden: Boolean(document.querySelector('[data-testid="state-forbidden"]')),
      header: read('.obs-page-header'),
      lead: read('.projects-lead'),
      section: read('.projects-section'),
      sectionHead: read('.projects-section .ant-card-head'),
      sectionTitle: read('.projects-section .obs-section-title'),
      tableHead: read('.projects-table thead th'),
      firstTableHead: read('.projects-table thead th:first-child'),
      completionUnknown: read('.projects-completion-unknown'),
      count: document.querySelector('.projects-section .obs-section-count')?.textContent ?? null,
      rows: document.querySelectorAll('.projects-table tbody tr.ant-table-row').length,
      headers: [...document.querySelectorAll('.projects-table thead th')].map((element) => element.textContent).filter(Boolean),
      freshness: Boolean(document.querySelector('.projects-footer [data-testid="freshness-badge"]')),
    };
  });
}

export function checkProjectsGeometry(metric, compact) {
  if (metric.forbidden) return [];
  const failures = [];
  const equal = createEqual(failures);
  equal(metric.header?.paddingTop, compact ? 16 : 24, 'project header top padding');
  equal(metric.header?.paddingBottom, compact ? 12 : 16, 'project header bottom padding');
  equal(metric.lead?.marginBottom, 20, 'project summary bottom margin');
  equal(metric.lead?.fontSize, compact ? 18 : 20, 'project summary font size');
  equal(metric.lead?.lineHeight, compact ? 26 : 28, 'project summary line height');
  equal(metric.lead?.fontWeight, 500, 'project summary font weight');
  equal(metric.section?.borderRadius, 16, 'project section radius');
  equal(metric.sectionHead?.height, 48, 'project section heading height');
  equal(metric.sectionHead?.paddingLeft, 20, 'project section heading horizontal padding');
  equal(metric.sectionTitle?.fontSize, 14, 'project section title font size');
  equal(metric.sectionTitle?.lineHeight, 20, 'project section title line height');
  equal(metric.tableHead?.height, 40, 'project table heading height');
  equal(metric.tableHead?.fontSize, 12, 'project table heading font size');
  equal(metric.firstTableHead?.paddingLeft, compact ? 16 : 20, 'project first table heading padding');
  if (metric.completionUnknown) {
    equal(metric.completionUnknown.fontSize, 12, 'unknown completion font size');
    equal(metric.completionUnknown.lineHeight, 16, 'unknown completion line height');
  }
  if (metric.count !== null && !Number.isNaN(Number(metric.count))) equal(metric.count, metric.rows, 'project section count versus detail rows');
  const expectedHeaders = metric.rows === 0 && metric.count === '0' ? [] : compact
    ? ['项目', '状态', '一句话', '完成得怎么样']
    : ['项目', '状态', '一句话', '完成得怎么样', '更新'];
  if (JSON.stringify(metric.headers) !== JSON.stringify(expectedHeaders)) {
    failures.push(`project table headings differ: ${JSON.stringify(metric.headers)}`);
  }
  if (!metric.freshness) failures.push('project freshness badge is missing');
  return failures;
}

/* The canonical variant also serves the project-list facts the page needs for its
   prototype comparison: recorded leads and groups, period updates and the takeover
   row. These assertions are the page-level oracle — they fail when the demo stops
   reproducing the prototype's projects table. */
export async function recordCanonicalProjects(page) {
  await settle(page);
  await page.getByTestId('projects-table').waitFor();
  // innerText flattens the DOM (links, fragments) into one string with incidental
  // whitespace; both sides are normalised the same way so the glyphs are compared.
  const flat = (text) => text.replace(/\s+/g, '');
  const lead = flat(await page.getByTestId('projects-lead').innerText());
  assert.equal(lead, flat('4 个项目，1 个有风险，1 个没有更新，1 个刚接管'),
    'the canonical projects lead sentence must equal the prototype');
  const rows = page.locator('.projects-table tbody tr.ant-table-row');
  assert.equal(await rows.count(), 4, 'the canonical projects table has the prototype four rows');
  const statusTexts = [];
  for (let index = 0; index < 4; index += 1) {
    statusTexts.push((await rows.nth(index).locator('.obs-tag').innerText()).trim());
  }
  assert.deepEqual(statusTexts, ['有风险', '正常', '没有更新', '刚接管'],
    'the four status labels must equal the prototype');
  const sentences = [];
  const updatedTexts = [];
  for (let index = 0; index < 4; index += 1) {
    sentences.push((await rows.nth(index).locator('.projects-summary-text').innerText()).trim());
  }
  assert.deepEqual(sentences, [
    '接口第 3 版的复核拖了两天，页面联调在等',
    '试运行第二周，没有新问题',
    '上一次更新是 12 天前',
    '读到 4 条工作线，按验收核验过的是 0 项，有 4 个问题要回答',
  ], 'the four one-sentence cells must equal the prototype');
  const wide = (page.viewportSize()?.width ?? 0) >= 1240;
  if (wide) {
    for (let index = 0; index < 4; index += 1) {
      updatedTexts.push((await rows.nth(index).locator('.projects-update-time').innerText()).trim());
    }
    assert.deepEqual(updatedTexts, ['今天', '3 天前', '12 天前', '刚刚'],
      'the four update-column ages must equal the prototype');
    // 该更新了 belongs to the third row alone.
    assert.equal(await page.locator('.projects-table .projects-update-note').count(), 1,
      'exactly one row is flagged 该更新了');
    assert.equal((await rows.nth(2).locator('.projects-update-note').innerText()).trim(), '该更新了');
  }
  // The takeover row links its zero-verified-tasks fact (its completion column stays
  // visible on small viewports).
  assert.equal((await rows.nth(3).locator('.projects-no-verified').innerText()).trim(),
    '接管后还没有按验收核验过的任务');
  const ownerMeta = [];
  for (let index = 0; index < 4; index += 1) {
    ownerMeta.push((await rows.nth(index).locator('.projects-project-meta').innerText()).trim());
  }
  assert.deepEqual(ownerMeta, ['周宁 · 应用小组', '吴桐 · 平台小组', '郑岚 · 交付小组', '刚接管'],
    'the four owner-and-group cells must equal the prototype');
  return { lead, statusTexts, sentences, updatedTexts, ownerMeta };
}

export default {
  name: 'projects',
  reportKey: 'projectsGeometry',
  matches: (route) => route.name === 'projects',
  measure: projectsGeometry,
  check: checkProjectsGeometry,
  canonical: {
    reportKey: 'canonicalProjectsRecords',
    route: {
      name: 'projects-canonical',
      path: '/projects',
      run: async (page, recordCanonical) => {
        await settle(page);
        recordCanonical(await recordCanonicalProjects(page));
      },
    },
  },
};
