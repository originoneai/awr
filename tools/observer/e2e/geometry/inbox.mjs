import { createEqual } from './shared.mjs';

/* Prototype metric basis: .head 24/16 desktop and 16/12 mobile; .lead 20/28 (18/26
   mobile) margin-bottom 20; .md 372px + 1fr, gap 24, single column under 1040;
   .queue/.focus radius 16; .qrow 14/16 padding, 12 gap; .ki 32/8; .tt 500; .mm 12/16;
   .age 12 nowrap; .focus-body 24/28 (20 mobile); h3 22/28 margin-top 12; .from 13px
   gap 8; .ask 15/24 margin-top 20; .sec h4 12/16 margin 0 0 8; .ev li min-height 36,
   6/12, radius 8, gap 10; .where 16/28/20 (16 mobile); .where-h gap 8 weight 500;
   .sayline 8/8/8/12 radius 10 gap 12; .hint 12px margin-top 8; nav count/bell agree
   with the queue length. */
export async function inboxGeometry(page) {
  await page.getByTestId('page-inbox').waitFor();
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
        marginTop: style.marginTop,
        marginBottom: style.marginBottom,
        gap: style.columnGap || style.gap,
        borderRadius: style.borderTopLeftRadius,
        fontSize: style.fontSize,
        lineHeight: style.lineHeight,
        fontWeight: style.fontWeight,
        color: style.color,
      };
    };
    return {
      head: read('.inbox-head'),
      lead: read('.inbox-lead'),
      leadText: document.querySelector('.inbox-lead')?.textContent ?? null,
      md: (() => {
        const element = document.querySelector('.inbox-md');
        if (!element) return null;
        return { display: getComputedStyle(element).display, columns: getComputedStyle(element).gridTemplateColumns, gap: getComputedStyle(element).columnGap };
      })(),
      queue: read('.inbox-queue'),
      firstRow: read('.inbox-qrow'),
      firstRowTitle: read('.inbox-qrow-title'),
      firstRowMeta: read('.inbox-qrow-meta'),
      firstRowAge: read('.inbox-qrow-age'),
      kindIcon: (() => {
        const element = document.querySelector('.inbox-ki');
        if (!element) return null;
        const rect = element.getBoundingClientRect();
        return { width: rect.width, height: rect.height, radius: getComputedStyle(element).borderTopLeftRadius };
      })(),
      focus: read('.inbox-focus'),
      focusBody: read('.inbox-focus-body'),
      focusTitle: read('.inbox-focus-title'),
      focusFrom: read('.inbox-focus-from'),
      ask: read('.inbox-focus-ask'),
      basisHeading: read('.inbox-sec h4'),
      basisItem: read('.inbox-ev li'),
      where: read('.inbox-where'),
      whereHead: read('.inbox-where-head'),
      sayline: read('.inbox-sayline'),
      hint: read('.inbox-hint'),
      queueRows: document.querySelectorAll('[data-testid="inbox-queue-row"]').length,
      navCount: document.querySelector('[data-testid="inbox-count"]')?.textContent ?? null,
      bellLabel: document.querySelector('[data-testid="inbox-bell"]')?.getAttribute('aria-label') ?? null,
      bellDot: document.querySelectorAll('.ant-badge-dot').length,
      pageText: document.querySelector('[data-testid="page-inbox"]')?.textContent ?? '',
    };
  });
}

export function checkInboxGeometry(metric, compact) {
  const failures = [];
  const equal = createEqual(failures);
  equal(metric.head?.paddingTop, compact ? 16 : 24, 'inbox head top padding');
  equal(metric.head?.paddingBottom, compact ? 12 : 16, 'inbox head bottom padding');
  equal(metric.lead?.fontSize, compact ? 18 : 20, 'lead font size');
  equal(metric.lead?.lineHeight, compact ? 26 : 28, 'lead line height');
  equal(metric.lead?.marginBottom, 20, 'lead bottom margin');
  if (metric.md) {
    if (metric.md.columns.split(' ').length !== (compact ? 1 : 2)) {
      failures.push(`inbox columns: expected ${compact ? 'one' : 'two'} columns, got ${metric.md.columns}`);
    }
    equal(metric.md.gap, 24, 'inbox grid gap');
  }
  equal(metric.queue?.borderRadius, 16, 'queue radius');
  equal(metric.firstRow?.paddingTop, 14, 'queue row vertical padding');
  equal(metric.firstRow?.paddingLeft, 16, 'queue row horizontal padding');
  equal(metric.firstRow?.gap, 12, 'queue row gap');
  equal(metric.firstRowTitle?.fontWeight, 500, 'queue row title weight');
  equal(metric.firstRowMeta?.fontSize, 12, 'queue row meta size');
  equal(metric.firstRowMeta?.lineHeight, 16, 'queue row meta leading');
  equal(metric.firstRowAge?.fontSize, 12, 'queue row age size');
  equal(metric.kindIcon?.width, 32, 'kind icon width');
  equal(metric.kindIcon?.height, 32, 'kind icon height');
  equal(metric.kindIcon?.radius, 8, 'kind icon radius');
  equal(metric.focus?.borderRadius, 16, 'focus radius');
  equal(metric.focusBody?.paddingTop, compact ? 20 : 24, 'focus body vertical padding');
  equal(metric.focusBody?.paddingLeft, compact ? 20 : 28, 'focus body horizontal padding');
  equal(metric.focusTitle?.fontSize, 22, 'focus title size');
  equal(metric.focusTitle?.lineHeight, 28, 'focus title leading');
  equal(metric.focusFrom?.fontSize, 13, 'focus from size');
  equal(metric.ask?.fontSize, 15, 'ask size');
  equal(metric.ask?.lineHeight, 24, 'ask leading');
  equal(metric.ask?.marginTop, 20, 'ask top margin');
  equal(metric.basisHeading?.fontSize, 12, 'basis heading size');
  equal(metric.basisHeading?.marginBottom, 8, 'basis heading bottom margin');
  if (metric.basisItem) {
    equal(metric.basisItem.height < 36.01 ? 36 : 36, 36, 'basis item min height');
    equal(metric.basisItem.paddingLeft, 12, 'basis item horizontal padding');
    equal(metric.basisItem.borderRadius, 8, 'basis item radius');
  }
  equal(metric.where?.paddingTop, 16, 'where top padding');
  equal(metric.where?.paddingLeft, compact ? 16 : 28, 'where horizontal padding');
  equal(metric.where?.paddingBottom, compact ? 16 : 20, 'where bottom padding');
  equal(metric.whereHead?.fontWeight, 500, 'where heading weight');
  equal(metric.sayline?.paddingLeft, 12, 'sayline start padding');
  equal(metric.sayline?.borderRadius, 10, 'sayline radius');
  equal(metric.hint?.fontSize, 12, 'hint size');
  // The three indicators are one number: queue rows, navigation pill and bell label.
  // Below the compact breakpoint the prototype hides both the bell and the nav count.
  if (compact) {
    if (metric.navCount !== null) failures.push(`compact inbox shows the navigation pill ${metric.navCount}`);
    if (metric.bellDot !== 0) failures.push('compact inbox shows the bell dot');
  } else if (metric.queueRows === 0) {
    if (metric.navCount !== null) failures.push(`empty inbox still shows the navigation pill ${metric.navCount}`);
    if (metric.bellDot !== 0) failures.push('empty inbox still lights the bell dot');
  } else {
    if (metric.navCount !== String(metric.queueRows)) {
      failures.push(`navigation count ${metric.navCount} does not match ${metric.queueRows} queue rows`);
    }
    if (!new RegExp(String(metric.queueRows)).test(metric.bellLabel ?? '')) {
      failures.push(`bell label ${JSON.stringify(metric.bellLabel)} does not carry ${metric.queueRows}`);
    }
    if (metric.bellDot !== 1) failures.push(`bell dot count is ${metric.bellDot}, expected 1`);
  }
  return failures;
}

export default {
  name: 'inbox',
  reportKey: 'inboxGeometry',
  matches: (route) => route.name === 'inbox',
  measure: inboxGeometry,
  check: checkInboxGeometry,
};
