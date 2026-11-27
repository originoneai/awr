import { createEqual, settle } from './shared.mjs';

export async function taskDetail(page) {
  await page.getByTestId('page-task').waitFor();
  await page.getByTestId('task-title').waitFor();
  await page.getByTestId('task-section-lessons').waitFor();
  await settle(page);
}

export async function taskGeometry(page) {
  await page.getByTestId('page-task').waitFor();
  return page.evaluate(() => {
    const title = document.querySelector('[data-testid="task-title"]');
    const cols = document.querySelector('.task-cols');
    const section = document.querySelector('.task-section');
    const titleStyle = title ? getComputedStyle(title) : null;
    const colsStyle = cols ? getComputedStyle(cols) : null;
    const sectionStyle = section ? getComputedStyle(section) : null;
    return {
      titleText: title?.textContent?.trim() ?? '',
      titleSize: titleStyle?.fontSize ?? null,
      titleLeading: titleStyle?.lineHeight ?? null,
      columns: colsStyle ? colsStyle.gridTemplateColumns.split(' ').filter(Boolean).length : 0,
      sectionRadius: sectionStyle?.borderTopLeftRadius ?? null,
      leftHeadings: [...document.querySelectorAll('[data-testid="task-col-left"] .obs-section-title')].map((el) => el.textContent?.trim()),
      rightHeadings: [...document.querySelectorAll('[data-testid="task-col-right"] .obs-section-title')].map((el) => el.textContent?.trim()),
      detailsCount: document.querySelectorAll('details').length,
      freshness: Boolean(document.querySelector('[data-testid="task-coverage"] [data-testid="freshness-badge"]')),
      hasUnknownBanner: Boolean(document.querySelector('[data-testid="task-unknown"]')),
      hasPartialBanner: Boolean(document.querySelector('[data-testid="task-partial"]')),
      statusTagText: document.querySelector('[data-testid="task-meta"] .obs-tag')?.textContent?.trim() ?? '',
      coverageSummaryText: document.querySelector('[data-testid="task-coverage-summary"]')?.textContent?.trim() ?? '',
    };
  });
}

export function checkTaskGeometry(metric, compact) {
  const failures = [];
  const equal = createEqual(failures);
  if (!metric.titleText) failures.push('task title is empty');
  equal(metric.titleSize, 24, 'task title font size');
  equal(metric.titleLeading, 32, 'task title line height');
  equal(metric.columns, compact ? 1 : 2, 'task grid column count');
  equal(metric.sectionRadius, 16, 'task section border radius');
  const expectedLeft = ['完成标准', '最近一次报告', '交付与复核', '经过'];
  const expectedRight = ['现在卡在哪', '为什么做', '上下游', '要读的资料', '用到的经验'];
  if (JSON.stringify(metric.leftHeadings) !== JSON.stringify(expectedLeft)) {
    failures.push(`task left headings differ: ${JSON.stringify(metric.leftHeadings)}`);
  }
  if (JSON.stringify(metric.rightHeadings) !== JSON.stringify(expectedRight)) {
    failures.push(`task right headings differ: ${JSON.stringify(metric.rightHeadings)}`);
  }
  if (metric.detailsCount !== 0) failures.push(`task page rendered ${metric.detailsCount} <details> elements (technical details must be omitted)`);
  if (!metric.freshness) failures.push('task freshness badge is missing');
  const claimsComplete = /任务详情、报告、复核、依赖与动态已读取|task details, report, reviews, dependencies and activity read/i.test(metric.coverageSummaryText);
  if ((metric.hasUnknownBanner || metric.hasPartialBanner || metric.statusTagText === '未知' || metric.statusTagText === 'Unknown') && claimsComplete) {
    failures.push(`task page shows unknown/partial state (unknownBanner=${metric.hasUnknownBanner}, partialBanner=${metric.hasPartialBanner}, status=${metric.statusTagText}) but coverage footer claims complete read: ${metric.coverageSummaryText}`);
  }
  return failures;
}

export default {
  name: 'task',
  reportKey: 'taskGeometry',
  matches: (route) => route.name === 'task',
  measure: taskGeometry,
  check: checkTaskGeometry,
  routeActions: {
    task: taskDetail,
  },
};
