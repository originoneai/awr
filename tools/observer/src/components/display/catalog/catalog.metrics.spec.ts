import { expect, test, type Page } from '@playwright/test';

// Computed-metric assertions for the shared display components. Every expected value is
// transcribed from the prototype's own stylesheet (panorama.html): the selectors are named
// in comments next to each number. The prototype applies its compact overrides inside
// @media (max-width: 860px); the phone-light project runs inside that query, so expectations
// that the prototype overrides below 860px are selected by project name.

function isPhone(projectName: string): boolean {
  return projectName === 'phone-light';
}

async function open(page: Page, component: string) {
  await page.goto(`/src/components/display/catalog/index.html?component=${component}`);
  await page.getByTestId('example-canvas').waitFor();
  await page.evaluate(async () => {
    await document.fonts.ready;
  });
}

function readStyles(page: Page, selector: string, properties: string[], pseudo?: string) {
  return page.evaluate(({ selector, properties, pseudo }) => {
    const element = document.querySelector(selector);
    if (!element) throw new Error(`Missing element for ${selector}`);
    const styles = getComputedStyle(element, pseudo ?? null);
    const result: Record<string, string> = {};
    for (const property of properties) result[property] = styles.getPropertyValue(property);
    return result;
  }, { selector, properties, pseudo });
}

function readBox(page: Page, selector: string) {
  return page.evaluate((selector) => {
    const element = document.querySelector(selector);
    if (!element) throw new Error(`Missing element for ${selector}`);
    const rect = element.getBoundingClientRect();
    return { width: rect.width, height: rect.height, left: rect.left, right: rect.right, top: rect.top };
  }, selector);
}

test.beforeEach(async ({ page }, testInfo) => {
  await page.emulateMedia({ reducedMotion: 'reduce' });
  const mode = testInfo.project.name === 'desktop-dark' ? 'dark' : 'light';
  await page.addInitScript((themeMode) => {
    window.localStorage.setItem('awr-observer-theme', themeMode);
    window.localStorage.setItem('i18nextLng', 'zh-CN');
  }, mode);
});

test('section matches the prototype block, block-head and block-body', async ({ page }, testInfo) => {
  await open(page, 'section');
  // .block { border: 1px solid; border-radius: 16px; box-shadow: var(--shadow-s) }
  // The prototype defines --shadow-s only for light; dark sets it to none.
  const block = await readStyles(page, '.obs-section', ['border-top-width', 'border-top-left-radius', 'box-shadow']);
  expect(block['border-top-width']).toBe('1px');
  expect(block['border-top-left-radius']).toBe('16px');
  if (testInfo.project.name === 'desktop-dark') {
    expect(block['box-shadow']).toBe('none');
  } else {
    expect(block['box-shadow']).not.toBe('none');
  }
  // .block-head { min-height: 48px; padding: 0 20px; border-bottom: 1px solid var(--line) }
  const head = await readStyles(page, '.obs-section .ant-card-head', ['min-height', 'padding-left', 'padding-right', 'border-bottom-width']);
  expect(head['min-height']).toBe('48px');
  expect(head['padding-left']).toBe('20px');
  expect(head['padding-right']).toBe('20px');
  expect(head['border-bottom-width']).toBe('1px');
  // .block-head h2 { font-size: 14px; line-height: 20px; font-weight: 500 }
  const title = await readStyles(page, '.obs-section-title', ['font-size', 'font-weight', 'line-height']);
  expect(title).toEqual({ 'font-size': '14px', 'font-weight': '500', 'line-height': '20px' });
  // .block-head .n { font-size: 12px; font-variant-numeric: tabular-nums }
  const count = await readStyles(page, '.obs-section-count', ['font-size', 'font-variant-numeric', 'font-weight']);
  expect(count['font-size']).toBe('12px');
  expect(count['font-weight']).toBe('400');
  expect(count['font-variant-numeric']).toContain('tabular-nums');
  // .block-head .sp { gap: 12px }
  const extra = await readStyles(page, '.obs-section .ant-card-extra', ['column-gap']);
  expect(extra['column-gap']).toBe('12px');
  // .block-body { padding: 16px 20px }
  const body = await readStyles(page, '.obs-section-content[data-padded]', ['padding-top', 'padding-right', 'padding-bottom', 'padding-left']);
  expect(`${body['padding-top']} ${body['padding-right']} ${body['padding-bottom']} ${body['padding-left']}`).toBe('16px 20px 16px 20px');
});

test('empty state matches the prototype empty', async ({ page }) => {
  await open(page, 'empty');
  // .empty { padding: 20px; color: var(--ink-3); font-size: 13px }
  const styles = await readStyles(page, '.obs-empty', ['padding-top', 'padding-left', 'font-size']);
  expect(`${styles['padding-top']} ${styles['padding-left']}`).toBe('20px 20px');
  expect(styles['font-size']).toBe('13px');
});

test('key-value table matches the prototype kv', async ({ page }) => {
  await open(page, 'keyValue');
  // .kv { display: grid; grid-template-columns: 96px minmax(0, 1fr); gap: 8px 16px; padding: 16px 20px }
  const grid = await readStyles(page, '.obs-key-value', ['display', 'grid-template-columns', 'row-gap', 'column-gap', 'padding-top', 'padding-left']);
  expect(grid['display']).toBe('grid');
  expect(grid['grid-template-columns'].split(' ')[0]).toBe('96px');
  expect(grid['row-gap']).toBe('8px');
  expect(grid['column-gap']).toBe('16px');
  expect(`${grid['padding-top']} ${grid['padding-left']}`).toBe('16px 20px');
  // .kv dt { font-size: 13px }
  const term = await readStyles(page, '.obs-key-value dt', ['font-size']);
  expect(term['font-size']).toBe('13px');
});

test('row list matches the prototype rows', async ({ page }) => {
  await open(page, 'rowList');
  // .rows > * { display: flex; gap: 12px; width: 100%; padding: 12px 20px; box-sizing border-box }
  const control = await readStyles(page, '.obs-row-control', ['padding-top', 'padding-left', 'column-gap', 'box-sizing']);
  expect(`${control['padding-top']} ${control['padding-left']}`).toBe('12px 20px');
  expect(control['column-gap']).toBe('12px');
  expect(control['box-sizing']).toBe('border-box');
  const widths = await page.evaluate(() => {
    const controlElement = document.querySelector('.obs-row-control')!;
    const itemElement = controlElement.parentElement!;
    return { control: controlElement.getBoundingClientRect().width, item: itemElement.getBoundingClientRect().width };
  });
  expect(widths.control, 'row control must not overflow its row').toBeLessThanOrEqual(widths.item + 0.5);
  // .rows > * + * { border-top: 1px solid var(--line) }
  const secondItem = await readStyles(page, '.obs-row-item:nth-child(2)', ['border-top-width']);
  expect(secondItem['border-top-width']).toBe('1px');
  // .rows .tt inherits 14px/20px with weight 500; .cn .t { 14px/20px/500 }
  const title = await readStyles(page, '.obs-row-title', ['font-size', 'font-weight', 'line-height']);
  expect(title).toEqual({ 'font-size': '14px', 'font-weight': '500', 'line-height': '20px' });
  // .rows .mm { margin-top: 2px; font-size: 12px; line-height: 16px }
  const meta = await readStyles(page, '.obs-row-meta', ['font-size', 'line-height', 'margin-top']);
  expect(meta).toEqual({ 'font-size': '12px', 'line-height': '16px', 'margin-top': '2px' });
  // .rows .x { padding-top: 2px; font-size: 12px }
  const trailing = await readStyles(page, '.obs-row-trailing', ['font-size', 'padding-top']);
  expect(trailing).toEqual({ 'font-size': '12px', 'padding-top': '2px' });
});

test('data table matches the prototype table t', async ({ page }, testInfo) => {
  await open(page, 'dataTable');
  const edgeInset = isPhone(testInfo.project.name) ? '16px' : '20px';
  // .t th { height: 40px; padding: 0 12px; border-bottom: 1px solid; font-size: 12px; font-weight: 400 }
  const headerCell = await readStyles(page, '.obs-data-table thead th:nth-child(2)', ['height', 'padding-left', 'padding-right', 'border-bottom-width', 'font-size', 'font-weight']);
  expect(headerCell['height']).toBe('40px');
  expect(`${headerCell['padding-left']} ${headerCell['padding-right']}`).toBe('12px 12px');
  expect(headerCell['border-bottom-width']).toBe('1px');
  expect(headerCell['font-size']).toBe('12px');
  expect(headerCell['font-weight']).toBe('400');
  // .t th:first-child, .t td:first-child { padding-left: 20px } (16px below 860px)
  const firstHeader = await readStyles(page, '.obs-data-table thead th:first-child', ['padding-left']);
  expect(firstHeader['padding-left']).toBe(edgeInset);
  // .t td { padding: 12px; border-top: 1px solid }
  const openableCells = await page.evaluate(() => {
    const row = document.querySelector('.obs-data-table tbody tr[data-openable="true"]')!;
    return [...row.querySelectorAll('td')].map((cell) => {
      const styles = getComputedStyle(cell);
      return { paddingLeft: styles.paddingLeft, paddingRight: styles.paddingRight, paddingTop: styles.paddingTop, width: cell.getBoundingClientRect().width };
    });
  });
  expect(openableCells[0].paddingTop).toBe('12px');
  expect(openableCells[0].paddingLeft).toBe(edgeInset);
  // .t td.chevc { width: 36px; padding: 0 16px 0 0 }
  const arrow = openableCells[openableCells.length - 1];
  expect(Math.round(arrow.width)).toBe(36);
  expect(arrow.paddingRight).toBe('16px');
  // Without an arrow column the last data column carries the outer inset
  // (.t th/td:last-child { padding-right: 20px }, 16px below 860px).
  const staticRow = await page.evaluate(() => {
    const table = document.querySelectorAll('.obs-data-table')[1];
    const row = table!.querySelector('tbody tr')!;
    return [...row.querySelectorAll('td')].map((cell) => {
      const styles = getComputedStyle(cell);
      return { paddingLeft: styles.paddingLeft, paddingRight: styles.paddingRight };
    });
  });
  expect(staticRow[0].paddingLeft).toBe(edgeInset);
  expect(staticRow[staticRow.length - 1].paddingRight).toBe(edgeInset);
});

test('detail panel matches the prototype panel and ptabs', async ({ page }, testInfo) => {
  await open(page, 'detailPanel');
  const phone = isPhone(testInfo.project.name);
  // .panel { margin-top: 24px; border: 1px solid var(--line); border-radius: 16px }
  const panel = await readStyles(page, '.obs-detail-panel', ['border-top-width', 'border-top-left-radius', 'margin-top']);
  expect(panel).toEqual({ 'border-top-width': '1px', 'border-top-left-radius': '16px', 'margin-top': '24px' });
  // .ptabs { gap: 24px; min-height: 48px; padding: 0 20px } (gap 20px / padding 0 16px below 860px)
  const nav = await readStyles(page, '.obs-detail-tabs .ant-tabs-nav', ['min-height', 'padding-left']);
  expect(nav['min-height']).toBe('48px');
  expect(nav['padding-left']).toBe(phone ? '16px' : '20px');
  const navBox = await readBox(page, '.obs-detail-tabs .ant-tabs-nav');
  expect(Math.round(navBox.height)).toBe(48);
  const tabGap = await page.evaluate(() => {
    const tabs = [...document.querySelectorAll('.obs-detail-tabs .ant-tabs-tab')];
    return tabs[1].getBoundingClientRect().left - tabs[0].getBoundingClientRect().right;
  });
  expect(Math.round(tabGap)).toBe(phone ? 20 : 24);
  // .ptabs button.tab[aria-selected] { font-weight: 500 }; button.tab .n { margin-left: 6px; font-size: 12px }
  const activeTab = await readStyles(page, '.obs-detail-tabs .ant-tabs-tab-active .ant-tabs-tab-btn', ['font-size', 'font-weight']);
  expect(activeTab).toEqual({ 'font-size': '14px', 'font-weight': '500' });
  const tabCount = await readStyles(page, '.obs-detail-tab-count', ['font-size', 'font-weight', 'margin-left']);
  expect(tabCount).toEqual({ 'font-size': '12px', 'font-weight': '400', 'margin-left': '6px' });
  // selected tab border-bottom: 2px solid var(--accent)
  const inkBar = await readStyles(page, '.obs-detail-tabs .ant-tabs-ink-bar', ['height']);
  expect(inkBar['height']).toBe('2px');
  // footer: 12px/16px muted note on a hairline (.pnote)
  const footer = await readStyles(page, '.obs-detail-footer', ['padding-top', 'padding-left', 'font-size', 'line-height', 'border-top-width']);
  expect(`${footer['padding-top']} ${footer['padding-left']}`).toBe('12px 20px');
  expect(footer['font-size']).toBe('12px');
  expect(footer['line-height']).toBe('16px');
  expect(footer['border-top-width']).toBe('1px');
});

test('timeline matches the prototype tl', async ({ page }) => {
  await open(page, 'timeline');
  // .tl { margin: 0; padding: 4px 20px 16px }
  const root = await readStyles(page, '.obs-timeline', ['padding-top', 'padding-right', 'padding-bottom', 'padding-left']);
  expect(`${root['padding-top']} ${root['padding-right']} ${root['padding-bottom']} ${root['padding-left']}`).toBe('4px 20px 16px 20px');
  // .tl li { padding: 12px 0 0 24px }
  const item = await readStyles(page, '.obs-timeline-item', ['padding-top', 'padding-left']);
  expect(`${item['padding-top']} ${item['padding-left']}`).toBe('12px 24px');
  // .tl .w { font-size: 12px; line-height: 16px }
  const time = await readStyles(page, '.obs-timeline-time', ['font-size', 'line-height']);
  expect(time).toEqual({ 'font-size': '12px', 'line-height': '16px' });
  // .tl .x { margin-top: 2px; font-size: 13px }
  const detail = await readStyles(page, '.obs-timeline-detail', ['font-size', 'margin-top']);
  expect(detail).toEqual({ 'font-size': '13px', 'margin-top': '2px' });
  // .tl li::before { left: 3px; top: 18px; width: 8px; height: 8px; border-radius: 50% }
  const dot = await readStyles(page, '.obs-timeline-item', ['width', 'height', 'left', 'top', 'border-top-left-radius'], '::before');
  expect(`${dot['width']} ${dot['height']} ${dot['left']} ${dot['top']} ${dot['border-top-left-radius']}`).toBe('8px 8px 3px 18px 50%');
  // .tl li::after { left: 6.5px; top: 28px; width: 1px }
  const connector = await readStyles(page, '.obs-timeline-item', ['width', 'left', 'top'], '::after');
  expect(`${connector['width']} ${connector['left']} ${connector['top']}`).toBe('1px 6.5px 28px');
});

test('detail page header matches the prototype hero, sum, meta and back', async ({ page }) => {
  await open(page, 'pageHeader');
  // .hero h1 { font-size: 24px; line-height: 32px }
  const title = await readStyles(page, '.obs-page-title', ['font-size', 'font-weight', 'line-height']);
  expect(title).toEqual({ 'font-size': '24px', 'font-weight': '500', 'line-height': '32px' });
  // .hero .sum { max-width: 760px; font-size: 16px; line-height: 24px }
  const summary = await readStyles(page, '.obs-page-summary', ['max-width', 'font-size', 'line-height']);
  expect(summary).toEqual({ 'max-width': '760px', 'font-size': '16px', 'line-height': '24px' });
  // .hero .meta { gap: 8px 16px; margin-top: 12px; font-size: 13px }
  const meta = await readStyles(page, '.obs-page-meta', ['margin-top', 'row-gap', 'column-gap', 'font-size']);
  expect(meta).toEqual({ 'margin-top': '12px', 'row-gap': '8px', 'column-gap': '16px', 'font-size': '13px' });
  // .back { gap: 6px; margin-top: 20px; font-size: 13px }
  const back = await readStyles(page, '.obs-page-back', ['margin-top', 'column-gap', 'font-size']);
  expect(back).toEqual({ 'margin-top': '20px', 'column-gap': '6px', 'font-size': '13px' });
  // .hero { padding: 12px 0 24px }
  const header = await readStyles(page, '.obs-page-header[data-variant="detail"]', ['padding-top', 'padding-bottom']);
  expect(`${header['padding-top']} ${header['padding-bottom']}`).toBe('12px 24px');
});

test('list page header matches the prototype head and h1', async ({ page }, testInfo) => {
  await open(page, 'pageHeaderCompact');
  // h1 { font-size: 22px; line-height: 28px; font-weight: 500 }
  const title = await readStyles(page, '.obs-page-title', ['font-size', 'font-weight', 'line-height']);
  expect(title).toEqual({ 'font-size': '22px', 'font-weight': '500', 'line-height': '28px' });
  // .head .sub { font-size: 13px }
  const subtitle = await readStyles(page, '.obs-page-subtitle', ['font-size']);
  expect(subtitle['font-size']).toBe('13px');
  // .head { display: flex; align-items: baseline; gap: 12px; padding: 24px 0 16px }
  const line = await readStyles(page, '.obs-page-title-line', ['align-items', 'column-gap']);
  expect(line).toEqual({ 'align-items': 'baseline', 'column-gap': '12px' });
  // .head { padding: 24px 0 16px } (16px 0 12px below 860px)
  const header = await readStyles(page, '.obs-page-header', ['padding-top', 'padding-bottom']);
  if (isPhone(testInfo.project.name)) {
    expect(`${header['padding-top']} ${header['padding-bottom']}`).toBe('16px 12px');
  } else {
    expect(`${header['padding-top']} ${header['padding-bottom']}`).toBe('24px 16px');
  }
});

test('notice matches the prototype callout', async ({ page }) => {
  await open(page, 'notice');
  // .callout { gap: 12px; margin-bottom: 24px; padding: 12px 16px; border-radius: 12px; font-size: 13px }
  const styles = await readStyles(page, '.obs-notice', ['padding-top', 'padding-left', 'column-gap', 'border-top-left-radius', 'margin-bottom', 'font-size', 'line-height']);
  expect(`${styles['padding-top']} ${styles['padding-left']}`).toBe('12px 16px');
  expect(styles['column-gap']).toBe('12px');
  expect(styles['border-top-left-radius']).toBe('12px');
  expect(styles['margin-bottom']).toBe('24px');
  expect(styles['font-size']).toBe('13px');
  expect(styles['line-height']).toBe('20px');
});

test('perspective matches the prototype as, seg and compact top seg measurements', async ({ page }, testInfo) => {
  await open(page, 'perspective');
  const phone = isPhone(testInfo.project.name);
  // .as { display: flex; align-items: center; gap: 8px }
  const root = await readStyles(page, '.shell-perspective', ['display', 'align-items', 'column-gap', 'flex-wrap']);
  expect(root).toEqual({ display: 'flex', 'align-items': 'center', 'column-gap': '8px', 'flex-wrap': phone ? 'wrap' : 'nowrap' });
  // .as .cap { color: var(--ink-3); font-size: 12px; white-space: nowrap }
  // @media (max-width: 1040px) hides the caption, while the 860px layout remains useCompactViewport's sole source.
  const caption = await readStyles(page, '.shell-perspective-caption', ['display', 'font-size', 'white-space']);
  expect(caption.display).toBe(phone ? 'none' : 'block');
  expect(caption['font-size']).toBe('12px');
  expect(caption['white-space']).toBe('nowrap');
  // .seg { display: inline-flex; gap: 2px; padding: 2px; border-radius: 8px }
  // As in the prototype, being a flex item blockifies its computed display to flex.
  const segment = await readStyles(page, '.shell-perspective-segment', [
    'display', 'column-gap', 'padding-top', 'padding-left', 'border-top-left-radius', 'overflow-x',
  ]);
  expect(segment).toEqual({
    display: 'flex',
    'column-gap': '2px',
    'padding-top': '2px',
    'padding-left': '2px',
    'border-top-left-radius': '8px',
    // @media (max-width: 860px) .top .seg { flex: 1; overflow-x: auto }
    'overflow-x': phone ? 'auto' : 'visible',
  });
  // .seg button — prototype values: 28px height, 10px inline padding, 6px radius, 13px font, nowrap.
  const option = await readStyles(page, '.shell-perspective-option', [
    'height', 'padding-left', 'padding-right', 'border-top-left-radius', 'font-size', 'white-space',
  ]);
  expect(option).toEqual({
    height: '28px',
    'padding-left': '10px',
    'padding-right': '10px',
    'border-top-left-radius': '6px',
    'font-size': '13px',
    'white-space': 'nowrap',
  });
  // .seg button[aria-pressed="true"] uses a surface, a 1px line, var(--shadow-s) and weight 500.
  const selected = await readStyles(page, '.shell-perspective-option[aria-pressed="true"]', ['box-shadow', 'font-weight']);
  expect(selected['box-shadow']).not.toBe('none');
  expect(selected['box-shadow']).toContain('1px');
  expect(selected['font-weight']).toBe('500');
  const optionTops = await page.locator('.shell-perspective-option').evaluateAll((buttons) => (
    buttons.map((button) => button.getBoundingClientRect().top)
  ));
  expect(new Set(optionTops).size, 'options must stay on one line').toBe(1);

  if (phone) {
    // Force overflow below the required 390px check as well, proving the segment itself scrolls.
    await page.setViewportSize({ width: 320, height: 844 });
    const scrolled = await page.locator('.shell-perspective-segment').evaluate((element) => {
      element.scrollLeft = 80;
      return { left: element.scrollLeft, scrollWidth: element.scrollWidth, clientWidth: element.clientWidth };
    });
    expect(scrolled.scrollWidth).toBeGreaterThan(scrolled.clientWidth);
    expect(scrolled.left).toBeGreaterThan(0);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
  }
});
