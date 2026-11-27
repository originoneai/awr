import { createEqual } from './shared.mjs';

/* Prototype metric basis: .head 24/16 desktop and 16/12 mobile; .filters gap 12,
   margin-bottom 16; .seg gap/padding 2, radius 8; .seg button 28px, 13px, 0 10,
   radius 6; .block radius 16; .block-head 48px and 0 20; .t th 40px/12px;
   .pnote 12px 20px and 12px/16px. */
export async function activityGeometry(page) {
  await page.getByTestId('page-activity').waitFor();
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
        paddingRight: style.paddingRight,
        marginBottom: style.marginBottom,
        gap: style.gap,
        borderRadius: style.borderRadius,
        fontSize: style.fontSize,
        lineHeight: style.lineHeight,
        color: style.color,
        textDecorationLine: style.textDecorationLine,
      };
    };
    return {
      header: read('.obs-page-header'),
      filters: read('.activity-filters'),
      segments: [...document.querySelectorAll('.activity-segment')].map((element) => {
        const style = getComputedStyle(element);
        return { gap: style.gap, paddingTop: style.paddingTop, borderRadius: style.borderRadius };
      }),
      options: [...document.querySelectorAll('.activity-segment button')].map((element) => {
        const style = getComputedStyle(element);
        return {
          height: element.getBoundingClientRect().height,
          fontSize: style.fontSize,
          paddingLeft: style.paddingLeft,
          paddingRight: style.paddingRight,
          borderRadius: style.borderRadius,
        };
      }),
      section: read('.activity-section'),
      sectionHead: read('.activity-section .ant-card-head'),
      sectionTitle: read('.activity-section .obs-section-title'),
      tableHead: read('.activity-table thead th'),
      firstTableHead: read('.activity-table thead th:first-child'),
      taskLink: read('.activity-task-link'),
      note: read('.activity-pnote'),
      count: document.querySelector('.activity-section .obs-section-count')?.textContent ?? null,
      rows: document.querySelectorAll('.activity-table tbody tr.ant-table-row').length,
      rowHeights: [...document.querySelectorAll('.activity-table tbody tr.ant-table-row')]
        .map((row) => row.getBoundingClientRect().height),
      times: [...document.querySelectorAll('.activity-time')].map((cell) => cell.textContent),
      headers: [...document.querySelectorAll('.activity-table thead th')].map((element) => element.textContent).filter(Boolean),
    };
  });
}

export function checkActivityGeometry(metric, compact) {
  const failures = [];
  const equal = createEqual(failures);
  equal(metric.header?.paddingTop, compact ? 16 : 24, 'page header top padding');
  equal(metric.header?.paddingBottom, compact ? 12 : 16, 'page header bottom padding');
  equal(metric.filters?.gap, 12, 'filter gap');
  equal(metric.filters?.marginBottom, 16, 'filter bottom margin');
  for (const segment of metric.segments) {
    equal(segment.gap, 2, 'segment gap');
    equal(segment.paddingTop, 2, 'segment padding');
    equal(segment.borderRadius, 8, 'segment radius');
  }
  for (const option of metric.options) {
    equal(option.height, 28, 'segment option height');
    equal(option.fontSize, 13, 'segment option font size');
    equal(option.paddingLeft, 10, 'segment option left padding');
    equal(option.paddingRight, 10, 'segment option right padding');
    equal(option.borderRadius, 6, 'segment option radius');
  }
  equal(metric.section?.borderRadius, 16, 'section radius');
  equal(metric.sectionHead?.height, 48, 'section heading height');
  equal(metric.sectionHead?.paddingLeft, 20, 'section heading horizontal padding');
  equal(metric.sectionTitle?.fontSize, 14, 'section title font size');
  equal(metric.sectionTitle?.lineHeight, 20, 'section title line height');
  if (metric.taskLink) {
    if (metric.taskLink.color !== 'rgb(17, 24, 39)') failures.push(`task link colour: ${metric.taskLink.color}`);
    if (metric.taskLink.textDecorationLine !== 'none') failures.push(`task link default decoration: ${metric.taskLink.textDecorationLine}`);
  }
  if (metric.rowHeights.some((height) => height > 46.01)) {
    failures.push(`activity rows exceed 46px: ${metric.rowHeights.join(', ')}`);
  }
  if (metric.times.some((text) => /\d{2}:\d{2}:\d{2}/.test(text ?? ''))) {
    failures.push(`seconds remain in compact activity times: ${metric.times.join(', ')}`);
  }
  if (metric.tableHead) {
    equal(metric.tableHead.height, 40, 'table heading height');
    equal(metric.tableHead.fontSize, 12, 'table heading font size');
  }
  if (!compact && metric.firstTableHead) equal(metric.firstTableHead.paddingLeft, 20, 'first table heading padding');
  equal(metric.note?.paddingTop, 12, 'note vertical padding');
  equal(metric.note?.paddingLeft, 20, 'note horizontal padding');
  equal(metric.note?.fontSize, 12, 'note font size');
  equal(metric.note?.lineHeight, 16, 'note line height');
  if (metric.count !== null) equal(metric.count, metric.rows, 'section count versus visible rows');
  const expectedHeaders = compact ? ['什么时候', '谁', '做了什么'] : ['什么时候', '谁', '做了什么', '哪件任务'];
  if (metric.headers.length && JSON.stringify(metric.headers) !== JSON.stringify(expectedHeaders)) {
    failures.push(`table headings differ: ${JSON.stringify(metric.headers)}`);
  }
  return failures;
}

export default {
  name: 'activity',
  reportKey: 'activityGeometry',
  matches: (route) => route.name.startsWith('activity-'),
  measure: activityGeometry,
  check: checkActivityGeometry,
};
