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

/** Resolves any CSS color value, including var() tokens and color-mix forms. */
function resolveColor(page: Page, value: string) {
  return page.evaluate((cssColor) => {
    const probe = document.createElement('div');
    probe.style.color = cssColor;
    document.body.append(probe);
    const resolved = getComputedStyle(probe).color;
    probe.remove();
    return resolved;
  }, value);
}

function colorOf(page: Page, selector: string, pseudo?: string) {
  return page.evaluate(({ selector, pseudo }) => {
    const element = document.querySelector(selector);
    if (!element) throw new Error(`Missing element for ${selector}`);
    return getComputedStyle(element, pseudo ?? null).color;
  }, { selector, pseudo });
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

test('metric strip matches the prototype kpis and kpi', async ({ page }, testInfo) => {
  await open(page, 'kpiStrip');
  const phone = isPhone(testInfo.project.name);
  // .kpis { display: grid; grid-template-columns: repeat(4, minmax(0, 1fr)); border-top/bottom 1px }
  const strip = await readStyles(page, '.obs-kpis', ['display', 'border-top-width', 'border-bottom-width']);
  expect(strip['display']).toBe('grid');
  expect(strip['border-top-width']).toBe('1px');
  expect(strip['border-bottom-width']).toBe('1px');
  const tracks = await page.evaluate(() => getComputedStyle(document.querySelector('.obs-kpis')!).gridTemplateColumns.split(' ').length);
  expect(tracks).toBe(phone ? 2 : 4);
  // .kpi { padding: 16px 28px; border-left: 1px solid var(--line) } (12px 20px below 860px)
  const cell = await readStyles(page, '.obs-kpi:nth-child(2)', ['padding-top', 'padding-left', 'padding-right', 'border-left-width']);
  expect(`${cell['padding-top']} ${cell['padding-left']} ${cell['padding-right']}`).toBe(phone ? '12px 20px 20px' : '16px 28px 28px');
  expect(cell['border-left-width']).toBe('1px');
  const first = await readStyles(page, '.obs-kpi:first-child', ['border-left-width']);
  expect(first['border-left-width']).toBe('0px');
  // .kpi .l { font-size: 13px }
  const label = await readStyles(page, '.obs-kpi-label', ['font-size']);
  expect(label['font-size']).toBe('13px');
  // .kpi .v { margin-top: 4px; font-size: 32px; line-height: 40px; font-weight: 500 }
  // (28px/36px below 860px). The prototype's .v is a block whose unit small inherits the
  // absolute metric line height, so the cell measures 121px tall (107px on phone) with a
  // 47px value row (41px compact) — review R1.
  const value = await readStyles(page, '.obs-kpi-value', ['margin-top', 'font-size', 'line-height', 'font-weight', 'display']);
  expect(value).toEqual({
    'margin-top': '4px',
    'font-size': phone ? '28px' : '32px',
    'line-height': phone ? '36px' : '40px',
    'font-weight': '500',
    display: 'block',
  });
  const cellBox = await readBox(page, '.obs-kpi:nth-child(2)');
  expect(Math.round(cellBox.height)).toBe(phone ? 107 : 121);
  const valueBox = await readBox(page, '.obs-kpi:nth-child(2) .obs-kpi-value');
  expect(Math.round(valueBox.height)).toBe(phone ? 41 : 47);
  // .kpi .v small { margin-left: 4px; font-size: 14px; font-weight: 400 }
  const unit = await readStyles(page, '.obs-kpi-value small', ['margin-left', 'font-size', 'font-weight']);
  expect(unit).toEqual({ 'margin-left': '4px', 'font-size': '14px', 'font-weight': '400' });
  // .kpi .c { gap: 4px; margin-top: 2px; font-size: 12px; line-height: 16px }
  const change = await readStyles(page, '.obs-kpi-change', ['column-gap', 'margin-top', 'font-size', 'line-height']);
  expect(change).toEqual({ 'column-gap': '4px', 'margin-top': '2px', 'font-size': '12px', 'line-height': '16px' });
  // .kpi .c[data-d="good"] { color: var(--ok) } and the warn/bad variants.
  const goodChange = await colorOf(page, '.obs-kpi-change[data-tone="good"]');
  expect(goodChange).toBe(await resolveColor(page, 'var(--craft-color-positive)'));
  const warnChange = await colorOf(page, '.obs-kpi-change[data-tone="warn"]');
  expect(warnChange).toBe(await resolveColor(page, 'var(--craft-color-warning)'));
  const badChange = await colorOf(page, '.obs-kpi-change[data-tone="bad"]');
  expect(badChange).toBe(await resolveColor(page, 'var(--craft-color-negative)'));
  // .kpi.me .v { color: var(--accent) }
  const emphasized = await colorOf(page, '.obs-kpi[data-emphasized="true"] .obs-kpi-value');
  expect(emphasized).toBe(await resolveColor(page, 'var(--craft-color-accent)'));
  if (phone) {
    // .kpi:nth-child(3) { border-left: 0 } .kpi:nth-child(n+3) { border-top: 1px solid var(--line) }
    const third = await readStyles(page, '.obs-kpi:nth-child(3)', ['border-left-width', 'border-top-width']);
    expect(third['border-left-width']).toBe('0px');
    expect(third['border-top-width']).toBe('1px');
  }
});

test('status tags match the prototype tag and lamp', async ({ page }) => {
  await open(page, 'statusTag');
  // .tag { height: 22px; padding: 0 8px; radius 6; gap: 6px; font-size: 12px; line-height: 16px }
  const tag = await readStyles(page, '.obs-tag', ['height', 'padding-left', 'padding-right', 'border-top-left-radius', 'column-gap', 'font-size', 'line-height']);
  expect(tag).toEqual({
    height: '22px',
    'padding-left': '8px',
    'padding-right': '8px',
    'border-top-left-radius': '6px',
    'column-gap': '6px',
    'font-size': '12px',
    'line-height': '16px',
  });
  // .tag[data-h="ok"] { background: var(--ok-soft); color: var(--ok) }
  const okTag = await readStyles(page, '.obs-tag[data-tone="ok"]', ['background-color', 'color']);
  expect(okTag['color']).toBe(await resolveColor(page, 'var(--craft-color-positive)'));
  expect(okTag['background-color']).toBe(await resolveColor(page, 'var(--craft-color-positive-soft)'));
  const riskTag = await readStyles(page, '.obs-tag[data-tone="risk"]', ['background-color', 'color']);
  expect(riskTag['color']).toBe(await resolveColor(page, 'var(--craft-color-warning)'));
  // .lamp { width: 8px; height: 8px; border-radius: 50% }
  const lamp = await readStyles(page, '.obs-lamp', ['width', 'height', 'border-top-left-radius']);
  expect(lamp).toEqual({ width: '8px', height: '8px', 'border-top-left-radius': '50%' });
  // [data-st="done"] { --c: var(--ok) }
  const doneLamp = await readStyles(page, '.obs-lamp[data-lamp="done"]', ['background-color']);
  expect(doneLamp['background-color']).toBe(await resolveColor(page, 'var(--craft-color-positive)'));
  // .lamp.ring { background: transparent; box-shadow: inset 0 0 0 1.5px var(--c) }
  const ring = await readStyles(page, '.obs-lamp[data-ring="true"]', ['background-color', 'box-shadow']);
  expect(ring['background-color']).toBe(await resolveColor(page, 'transparent'));
  expect(ring['box-shadow']).toContain('1.5px');
  // Hover sentence: title carries the meaning, not the label.
  const title = await page.evaluate(() => document.querySelector('.obs-tag[data-tone="ok"]')!.getAttribute('title') ?? '');
  expect(title.length).toBeGreaterThan(6);
});

test('completion bar and legend match the prototype seg4 and legend', async ({ page }, testInfo) => {
  await open(page, 'completionBar');
  // .seg4 { display: flex; gap: 2px; height: 8px }
  const bar = await readStyles(page, '.obs-seg4', ['display', 'column-gap', 'height']);
  expect(bar).toEqual({ display: 'flex', 'column-gap': '2px', height: '8px' });
  // .seg4 i { min-width: 2px; border-radius: 4px; background: var(--k) }
  const part = await readStyles(page, '.obs-seg4-part', ['min-width', 'border-top-left-radius', 'flex-grow']);
  expect(`${part['min-width']} ${part['border-top-left-radius']} ${part['flex-grow']}`).toBe('2px 4px 6');
  const verifiedColor = await readStyles(page, '.obs-seg4-part[data-seg="verified"]', ['background-color']);
  expect(verifiedColor['background-color']).toBe(await resolveColor(page, 'var(--craft-completion-verified)'));
  const inProgressColor = await readStyles(page, '.obs-seg4-part[data-seg="inProgress"]', ['background-color']);
  expect(inProgressColor['background-color']).toBe(await resolveColor(page, 'var(--craft-completion-in-progress)'));
  // .seg4 + .sm { margin-top: 6px } and .sm { font-size: 12px; line-height: 16px }
  const summary = await readStyles(page, '.obs-completion-summary', ['margin-top', 'font-size', 'line-height']);
  expect(summary).toEqual({ 'margin-top': '6px', 'font-size': '12px', 'line-height': '16px' });
  // .legend { gap: 16px; font-size: 12px } .legend span { gap: 6px } .legend i { 8px; radius 2px }
  const legend = await readStyles(page, '.obs-legend', ['column-gap', 'font-size']);
  expect(legend).toEqual({ 'column-gap': '16px', 'font-size': '12px' });
  const legendItem = await readStyles(page, '.obs-legend > span', ['column-gap']);
  expect(legendItem['column-gap']).toBe('6px');
  const swatch = await readStyles(page, '.obs-legend-swatch', ['width', 'height', 'border-top-left-radius']);
  expect(`${swatch['width']} ${swatch['height']} ${swatch['border-top-left-radius']}`).toBe('8px 8px 2px');
  if (isPhone(testInfo.project.name)) {
    // .legend, .hlegend { display: none } below 860px
    const hidden = await readStyles(page, '.obs-legend', ['display']);
    expect(hidden['display']).toBe('none');
  }
});

test('chain matches the prototype chain, cn, cx and ql', async ({ page }, testInfo) => {
  await open(page, 'chain');
  const phone = isPhone(testInfo.project.name);
  // .why { display: flex; flex-direction: column; padding: 20px 28px 24px } on the dotted
  // canvas (--craft-color-canvas, radial-gradient dots on a 16px grid); below 860px the
  // prototype re-pads it to 16px 20px 20px — review R2.
  const block = await readStyles(page, '.obs-chain-block', [
    'display', 'flex-direction', 'padding-top', 'padding-right', 'padding-bottom', 'padding-left',
    'background-color', 'background-image', 'background-size',
  ]);
  expect(block['display']).toBe('flex');
  expect(block['flex-direction']).toBe('column');
  expect(`${block['padding-top']} ${block['padding-right']} ${block['padding-bottom']} ${block['padding-left']}`)
    .toBe(phone ? '16px 20px 20px 20px' : '20px 28px 24px 28px');
  expect(block['background-color']).toBe(await resolveColor(page, 'var(--craft-color-canvas)'));
  expect(block['background-image']).toContain('radial-gradient');
  expect(block['background-size']).toBe('16px 16px');
  // .chain { display: flex; align-items: center; margin-top: 20px }
  const chain = await readStyles(page, '.obs-chain', ['display', 'align-items', 'margin-top', 'flex-direction']);
  expect(chain['display']).toBe('flex');
  // Below 860px the prototype also stretches the stacked nodes (.chain { align-items: stretch }).
  expect(chain['align-items']).toBe(phone ? 'stretch' : 'center');
  expect(chain['margin-top']).toBe('20px');
  expect(chain['flex-direction']).toBe(phone ? 'column' : 'row');
  // .cn { min-height: 104px; padding: 12px 14px 14px; border: 1px solid var(--line-2); radius 12 }
  // (measured on a task node; the first node in the DOM is the root with its own overrides)
  const node = await readStyles(page, '.obs-chain-node[data-node-kind="task"]', [
    'min-height', 'padding-top', 'padding-left', 'padding-right', 'border-width', 'border-top-left-radius', 'flex-grow',
  ]);
  expect(`${node['min-height']} ${node['padding-top']} ${node['padding-left']} ${node['padding-right']}`).toBe(
    phone ? '0px 12px 14px 14px' : '104px 12px 14px 14px',
  );
  expect(node['border-width']).toBe('1px');
  expect(node['border-top-left-radius']).toBe('12px');
  // Below 860px the prototype sets .cn { flex: none }, so growth collapses to zero.
  expect(node['flex-grow']).toBe(phone ? '0' : '1');
  // .cn.root { flex-grow: 1.35; border color-mix(bad 60%); background color-mix(bad 7%, surface) }
  const root = await readStyles(page, '.obs-chain-node-root', ['flex-grow', 'border-top-color', 'background-color']);
  expect(root['flex-grow']).toBe(phone ? '0' : '1.35');
  expect(root['border-top-color']).toBe(await resolveColor(page, 'color-mix(in srgb, var(--craft-color-negative) 60%, transparent)'));
  expect(root['background-color']).toBe(await resolveColor(page, 'color-mix(in srgb, var(--craft-color-negative) 7%, var(--craft-color-surface))'));
  // .cn .k { min-height: 20px; gap: 8px; font-size: 12px }
  const kicker = await readStyles(page, '.obs-chain-kicker', ['min-height', 'column-gap', 'font-size']);
  expect(kicker).toEqual({ 'min-height': '20px', 'column-gap': '8px', 'font-size': '12px' });
  // .cn .t { font-size: 14px; line-height: 20px; font-weight: 500 }
  const title = await readStyles(page, '.obs-chain-title', ['font-size', 'line-height', 'font-weight']);
  expect(title).toEqual({ 'font-size': '14px', 'line-height': '20px', 'font-weight': '500' });
  // .cn .m { font-size: 12px; line-height: 16px }
  const meta = await readStyles(page, '.obs-chain-meta', ['font-size', 'line-height']);
  expect(meta).toEqual({ 'font-size': '12px', 'line-height': '16px' });
  // .ql span { padding: 6px 10px }
  const queueRow = await readStyles(page, '.obs-chain-queue-row', ['padding-top', 'padding-left']);
  expect(`${queueRow['padding-top']} ${queueRow['padding-left']}`).toBe('6px 10px');
  // .cx { width: 48px; gap: 2px } (.cx below 860px: height 36px, column flow, rotated arrow)
  const connector = await readStyles(page, '.obs-chain-connector', ['width', 'height', 'column-gap', 'grid-auto-flow', 'padding-left']);
  const connectorBox = await readBox(page, '.obs-chain-connector');
  // The prototype's connector is a 28x12 thin arrow plus the 16px word under it: 30px tall.
  // On phone the arrow is rotated, so the intrinsic 28x12 is read from the attributes.
  const arrowSize = await page.evaluate(() => {
    const svg = document.querySelector('.obs-chain-connector svg')!;
    return { width: svg.getAttribute('width'), height: svg.getAttribute('height') };
  });
  expect(`${arrowSize.width}x${arrowSize.height}`).toBe('28x12');
  if (phone) {
    expect(`${connector['height']} ${connector['grid-auto-flow']} ${connector['padding-left']}`).toBe('36px column 20px');
    expect(Math.round(connectorBox.height)).toBe(36);
  } else {
    expect(`${connector['width']} ${connector['column-gap']}`).toBe('48px 2px');
    expect(Math.round(connectorBox.height)).toBe(30);
  }
  // .cx.hotx { color: var(--bad) }
  const hot = await colorOf(page, '.obs-chain-connector[data-hot="true"]');
  expect(hot).toBe(await resolveColor(page, 'var(--craft-color-negative)'));
  // .cn .foot { margin-top: auto; padding-top: 8px } .cn .foot .bar { height: 4px }
  const foot = await readStyles(page, '.obs-chain-foot', ['padding-top', 'font-size']);
  expect(`${foot['padding-top']} ${foot['font-size']}`).toBe('8px 12px');
  const progress = await readStyles(page, '.obs-chain-progress', ['height', 'border-top-left-radius']);
  expect(`${progress['height']} ${progress['border-top-left-radius']}`).toBe('4px 2px');
  // .cn .you { position: absolute; top: -11px; right: 12px; height: 20px; padding: 0 8px;
  // radius 6; background: var(--accent); color: var(--on-accent); 12px/20px/500 } — R4.
  const you = await readStyles(page, '.obs-chain-you', [
    'top', 'right', 'height', 'padding-left', 'padding-right', 'border-top-left-radius',
    'background-color', 'color', 'font-size', 'line-height', 'font-weight',
  ]);
  expect(`${you['top']} ${you['right']} ${you['height']} ${you['padding-left']} ${you['border-top-left-radius']}`).toBe('-11px 12px 20px 8px 6px');
  expect(you['background-color']).toBe(await resolveColor(page, 'var(--craft-color-accent)'));
  expect(you['color']).toBe(await resolveColor(page, 'var(--craft-color-on-accent)'));
  expect(`${you['font-size']} ${you['line-height']} ${you['font-weight']}`).toBe('12px 20px 500');
  // .cn.hot { border-color: var(--accent); box-shadow: 0 0 0 1px var(--accent) } — R4.
  const hotNode = await readStyles(page, '.obs-chain-node[data-highlighted="true"]', ['border-top-color', 'box-shadow']);
  expect(hotNode['border-top-color']).toBe(await resolveColor(page, 'var(--craft-color-accent)'));
  expect(hotNode['box-shadow']).toContain('0px 0px 0px 1px');
  // .why-foot { gap: 16px; margin-top: 16px } — R6.
  const footer = await readStyles(page, '.obs-chain-footer', ['column-gap', 'row-gap', 'margin-top']);
  expect(`${footer['column-gap']} ${footer['row-gap']} ${footer['margin-top']}`).toBe('16px 16px 16px');
});

test('todo rail matches the prototype rail and todo cards', async ({ page }, testInfo) => {
  await open(page, 'todoRail');
  const phone = isPhone(testInfo.project.name);
  // .rail { padding: 20px; border-left: 1px solid var(--line) } (16px 20px 20px, top border below 860px)
  const rail = await readStyles(page, '.obs-todo-rail', ['padding-top', 'padding-left', 'padding-bottom', 'border-left-width', 'border-top-width']);
  if (phone) {
    expect(`${rail['padding-top']} ${rail['padding-left']} ${rail['padding-bottom']}`).toBe('16px 20px 20px');
    expect(`${rail['border-left-width']} ${rail['border-top-width']}`).toBe('0px 1px');
  } else {
    expect(`${rail['padding-top']} ${rail['padding-left']} ${rail['padding-bottom']}`).toBe('20px 20px 20px');
    expect(`${rail['border-left-width']} ${rail['border-top-width']}`).toBe('1px 0px');
  }
  // .rail-head { gap: 8px; margin-bottom: 12px } .rail-head .n { 20px pill, radius 10, accent }
  const head = await readStyles(page, '.obs-todo-head', ['column-gap', 'margin-bottom']);
  expect(head).toEqual({ 'column-gap': '8px', 'margin-bottom': '12px' });
  const count = await readStyles(page, '.obs-todo-count', ['height', 'min-width', 'border-top-left-radius', 'background-color', 'color', 'font-size', 'line-height']);
  expect(`${count['height']} ${count['min-width']} ${count['border-top-left-radius']}`).toBe('20px 20px 10px');
  expect(count['background-color']).toBe(await resolveColor(page, 'var(--craft-color-accent)'));
  expect(count['color']).toBe(await resolveColor(page, 'var(--craft-color-on-accent)'));
  expect(`${count['font-size']} ${count['line-height']}`).toBe('12px 20px');
  // .todo { padding: 12px; border-radius: 12px; background mix(raised 45%, surface) }
  const card = await readStyles(page, '.obs-todo-card', ['padding-top', 'padding-left', 'border-top-left-radius', 'background-color']);
  expect(`${card['padding-top']} ${card['padding-left']} ${card['border-top-left-radius']}`).toBe('12px 12px 12px');
  expect(card['background-color']).toBe(await resolveColor(page, 'color-mix(in srgb, var(--craft-color-raised) 45%, var(--craft-color-surface))'));
  // .todo .where-s { gap: 6px } and .link { font-size: 13px } — review R6.
  const agent = await readStyles(page, '.obs-todo-agent', ['column-gap', 'font-size']);
  expect(`${agent['column-gap']} ${agent['font-size']}`).toBe('6px 12px');
  const view = await readStyles(page, '.obs-todo-view', ['column-gap', 'font-size']);
  expect(`${view['column-gap']} ${view['font-size']}`).toBe('4px 13px');
  // .todo .dd { -webkit-line-clamp: 2 }
  const detail = await readStyles(page, '.obs-todo-detail', ['-webkit-line-clamp']);
  expect(detail['-webkit-line-clamp']).toBe('2');
  // .todo.min { padding: 10px 12px } .todo + .todo { margin-top: 8px }
  const min = await readStyles(page, '.obs-todo-min', ['padding-top', 'padding-left']);
  expect(`${min['padding-top']} ${min['padding-left']}`).toBe('10px 12px');
  const stacked = await readStyles(page, '.obs-todo-min', ['margin-top']);
  expect(stacked['margin-top']).toBe('8px');
  // .rail .more { margin-top: 12px }
  const more = await readStyles(page, '.obs-todo-more', ['margin-top']);
  expect(more['margin-top']).toBe('12px');
});

test('acceptance list matches the prototype acc', async ({ page }) => {
  await open(page, 'acceptanceList');
  // .acc li { display: flex; gap: 12px; padding: 12px 20px; border-top: 1px solid var(--line) }
  const item = await readStyles(page, '.obs-acceptance-item:nth-child(2)', ['display', 'column-gap', 'padding-top', 'padding-left', 'border-top-width']);
  expect(item).toEqual({
    display: 'flex',
    'column-gap': '12px',
    'padding-top': '12px',
    'padding-left': '20px',
    'border-top-width': '1px',
  });
  const first = await readStyles(page, '.obs-acceptance-item:first-child', ['border-top-width']);
  expect(first['border-top-width']).toBe('0px');
  // .acc .ic { 18px icon; margin-top: 2px }
  const icon = await readStyles(page, '.obs-acceptance-icon', ['margin-top']);
  expect(icon['margin-top']).toBe('2px');
  const iconBox = await readBox(page, '.obs-acceptance-icon .obs-display-icon');
  expect(Math.round(iconBox.width)).toBe(18);
  expect(Math.round(iconBox.height)).toBe(18);
  // .acc li[data-s="ok"] .ic { color: var(--ok) }; [data-s="bad"] { color: var(--bad) }
  const supported = await colorOf(page, '.obs-acceptance-item[data-status="supported"] .obs-acceptance-icon');
  expect(supported).toBe(await resolveColor(page, 'var(--craft-color-positive)'));
  const failed = await colorOf(page, '.obs-acceptance-item[data-status="failed"] .obs-acceptance-icon');
  expect(failed).toBe(await resolveColor(page, 'var(--craft-color-negative)'));
  // .acc .e { margin-top: 2px; font-size: 12px; line-height: 16px }
  const evidence = await readStyles(page, '.obs-acceptance-evidence', ['margin-top', 'font-size', 'line-height']);
  expect(evidence).toEqual({ 'margin-top': '2px', 'font-size': '12px', 'line-height': '16px' });
  // .acc .s { padding-top: 2px; font-size: 12px; white-space: nowrap }
  const word = await readStyles(page, '.obs-acceptance-word', ['padding-top', 'font-size', 'white-space']);
  expect(word).toEqual({ 'padding-top': '2px', 'font-size': '12px', 'white-space': 'nowrap' });
});

test('perspective matches the prototype as, seg and compact top seg measurements', async ({ page }, testInfo) => {
  await open(page, 'perspective');
  const phone = isPhone(testInfo.project.name);
  // .as { display: flex; align-items: center; gap: 8px }
  const root = await readStyles(page, '.shell-perspective', ['display', 'align-items', 'column-gap', 'flex-wrap']);
  expect(root).toEqual({ display: 'flex', 'align-items': 'center', 'column-gap': '8px', 'flex-wrap': 'nowrap' });
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

test('search panel matches the prototype sr, sr-top, sr-input, sr-list and sr-row metrics', async ({ page }, testInfo) => {
  await open(page, 'search');
  const dialog = page.getByRole('dialog', { name: '搜索' });
  await dialog.waitFor();
  await page.getByRole('option').first().waitFor();
  expect(await page.getByRole('option').count()).toBe(6);
  const phone = isPhone(testInfo.project.name);

  // .sr { width: min(560px, 100%); border: 1px; border-radius: 16px }
  const panel = await readStyles(page, '.search-dialog .sr', ['border-top-width', 'border-top-left-radius']);
  expect(panel).toEqual({ 'border-top-width': '1px', 'border-top-left-radius': '16px' });
  const panelBox = await readBox(page, '.search-dialog .sr');
  expect(Math.round(panelBox.width)).toBe(phone ? 358 : 560);
  // .sr-back { padding-top: 12vh }, implemented as the Modal's 12vh top offset.
  const dialogBox = await readBox(page, '.search-dialog');
  expect(Math.round(dialogBox.top)).toBe(Math.round((phone ? 844 : 900) * 0.12));
  // .sr-back { place-items: start center }: horizontal centering is part of the same contract.
  expect(Math.round(dialogBox.left)).toBe(phone ? 16 : 440);

  // .sr-top { gap: 10px; height: 52px; padding: 0 16px; border-bottom: 1px }
  const top = await readStyles(page, '.sr-top', ['height', 'column-gap', 'padding-left', 'padding-right', 'border-bottom-width']);
  expect(top).toEqual({ height: '52px', 'column-gap': '10px', 'padding-left': '16px', 'padding-right': '16px', 'border-bottom-width': '1px' });
  // .sr-input { font-size: 15px }; the visible 2px focus ring is the accessibility addition.
  const input = await readStyles(page, '.sr-input', ['font-size', 'outline-style', 'outline-width']);
  expect(input).toEqual({ 'font-size': '15px', 'outline-style': 'solid', 'outline-width': '2px' });
  // .sr-list { max-height: 50vh; overflow-y: auto; padding: 6px }
  const list = await readStyles(page, '.sr-list', ['max-height', 'overflow-y', 'padding-top', 'padding-left']);
  expect(list).toEqual({ 'max-height': phone ? '422px' : '450px', 'overflow-y': 'auto', 'padding-top': '6px', 'padding-left': '6px' });
  // .sr-row { gap: 10px; min-height: 40px; padding: 6px 10px; border-radius: 8px }
  const row = await readStyles(page, '.sr-row', ['min-height', 'column-gap', 'padding-top', 'padding-left', 'border-top-left-radius']);
  expect(row).toEqual({ 'min-height': '40px', 'column-gap': '10px', 'padding-top': '6px', 'padding-left': '10px', 'border-top-left-radius': '8px' });
  // .sr-row .tag { height: 22px; padding: 0 8px; border-radius: 6px; font-size: 12px }
  const tag = await readStyles(page, '.sr-row .tag', ['height', 'padding-left', 'border-top-left-radius', 'font-size']);
  expect(tag).toEqual({ height: '22px', 'padding-left': '8px', 'border-top-left-radius': '6px', 'font-size': '12px' });
  // .sr-row .tt and .mm: one-line ellipsis; metadata is 12px and at most 40% wide.
  const title = await readStyles(page, '.sr-tt', ['overflow', 'text-overflow', 'white-space']);
  expect(title).toEqual({ overflow: 'hidden', 'text-overflow': 'ellipsis', 'white-space': 'nowrap' });
  const meta = await readStyles(page, '.sr-mm', ['max-width', 'font-size', 'text-overflow', 'white-space']);
  expect(meta).toEqual({ 'max-width': '40%', 'font-size': '12px', 'text-overflow': 'ellipsis', 'white-space': 'nowrap' });
  // .pnote supplies the required range note: 12px/16px, 12px 20px padding and a hairline.
  const coverage = await readStyles(page, '.sr-coverage', ['padding-top', 'padding-left', 'font-size', 'line-height', 'border-top-width']);
  expect(coverage).toEqual({ 'padding-top': '12px', 'padding-left': '20px', 'font-size': '12px', 'line-height': '16px', 'border-top-width': '1px' });
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
});
