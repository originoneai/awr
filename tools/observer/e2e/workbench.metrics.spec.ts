import { expect, test, type Page } from '@playwright/test';

// Computed-metric assertions for the real workbench page. Expected values come from the
// prototype's own panorama-v2.html stylesheet; every assertion names the source selector.
// The three projects exercise 1440 light, 1440 dark and 390 light, all against server.js --demo.

async function open(page: Page, dark: boolean) {
  await page.addInitScript(({ theme }) => {
    window.localStorage.setItem('awr-observer-theme', theme);
    window.localStorage.setItem('i18nextLng', 'zh-CN');
  }, { theme: dark ? 'dark' : 'light' });
  await page.goto('/workbench?scope=P-001&perspective=me');
  await page.locator('[data-testid="workbench-frame"]').waitFor();
  await page.evaluate(async () => { await document.fonts.ready; });
}

async function styles(page: Page, selector: string, properties: string[]) {
  return page.evaluate(({ selector, properties }) => {
    const element = document.querySelector(selector);
    if (!element) throw new Error(`Missing ${selector}`);
    const computed = getComputedStyle(element);
    return Object.fromEntries(properties.map((property) => [property, computed.getPropertyValue(property)]));
  }, { selector, properties });
}

async function resolvedColor(page: Page, value: string) {
  return page.evaluate((color) => {
    const probe = document.createElement('span');
    probe.style.color = color;
    (document.querySelector('.obs-workbench-instrument') ?? document.body).append(probe);
    const resolved = getComputedStyle(probe).color;
    probe.remove();
    return resolved;
  }, value);
}

test.beforeEach(async ({ page }, testInfo) => {
  await page.emulateMedia({ reducedMotion: 'reduce' });
  await open(page, testInfo.project.name === 'desktop-dark');
});

test('heading, verdict band, KPI strip, chain, rail and detail panel match the prototype metrics', async ({ page }, testInfo) => {
  test.skip(testInfo.project.name === 'desktop-new-york');
  const phone = testInfo.project.name === 'phone-light';

  // .head h1 { font-size: 22px; line-height: 28px; font-weight: 500 }
  expect(await styles(page, '.obs-page-title', ['font-size', 'line-height', 'font-weight']))
    .toEqual({ 'font-size': '22px', 'line-height': '28px', 'font-weight': '500' });
  // .head .sub { font-size: 13px }
  expect((await styles(page, '.obs-page-subtitle', ['font-size']))['font-size']).toBe('13px');
  // .head { padding: 24px 0 16px } (16px 0 12px below 860px)
  const heading = await styles(page, '.obs-page-header', ['padding-top', 'padding-bottom']);
  expect(`${heading['padding-top']} ${heading['padding-bottom']}`).toBe(phone ? '16px 12px' : '24px 16px');

  // Third-draft .inst { border-radius: 16px; background-image: none; box-shadow: var(--shadow-f) }
  const instrument = await styles(page, '.obs-workbench-instrument', ['border-top-left-radius', 'background-image', 'box-shadow']);
  expect(instrument['border-top-left-radius']).toBe('16px');
  expect(instrument['background-image']).toBe('none');
  expect(instrument['box-shadow']).toBe(testInfo.project.name === 'desktop-dark'
    ? 'rgba(0, 0, 0, 0.5) 0px 16px 40px 0px'
    : 'rgba(17, 24, 39, 0.1) 0px 8px 24px 0px');
  // .inst follows the page surface in the third draft.
  expect((await styles(page, '.obs-workbench-instrument', ['background-color']))['background-color'])
    .toBe(await resolvedColor(page, 'var(--craft-color-surface)'));

  // .inst-top { display: flex; align-items: flex-start; gap: 24px; padding: 24px 28px 20px }
  // below 860px: flex-direction column, gap 16px, padding 20px 20px 16px.
  const top = await styles(page, '.obs-workbench-verdict-top', [
    'display', 'align-items', 'flex-direction', 'column-gap', 'padding-top', 'padding-right', 'padding-bottom', 'padding-left',
  ]);
  expect(top.display).toBe('flex');
  expect(top['align-items']).toBe('flex-start');
  expect(top['flex-direction']).toBe(phone ? 'column' : 'row');
  expect(top['column-gap']).toBe(phone ? '16px' : '24px');
  expect(`${top['padding-top']} ${top['padding-right']} ${top['padding-bottom']} ${top['padding-left']}`)
    .toBe(phone ? '20px 20px 16px 20px' : '24px 28px 20px 28px');

  // .verdict { flex: 1; min-width: 0 }
  const verdict = await styles(page, '.obs-workbench-verdict', ['flex-grow', 'min-width']);
  expect(verdict['flex-grow']).toBe('1');
  expect(verdict['min-width']).toBe('0px');
  // .verdict .say { margin: 12px 0 0; font-size: 24px; line-height: 32px; font-weight: 500 }
  // below 860px: 20px/28px.
  const say = await styles(page, '.obs-workbench-sentence', ['margin-top', 'font-size', 'line-height', 'font-weight']);
  expect(say).toEqual({
    'margin-top': '12px', 'font-size': phone ? '20px' : '24px',
    'line-height': phone ? '28px' : '32px', 'font-weight': '500',
  });
  // .verdict .by { margin-top: 8px; font-size: 13px }
  const byline = await styles(page, '.obs-workbench-byline', ['margin-top', 'font-size']);
  expect(byline).toEqual({ 'margin-top': '8px', 'font-size': '13px' });

  // .kpis { grid 4 columns } (2 below 860px), .kpi { padding: 16px 28px } (12px 20px).
  const tracks = await page.locator('.obs-kpis').evaluate((element) => getComputedStyle(element).gridTemplateColumns.split(' ').length);
  expect(tracks).toBe(phone ? 2 : 4);
  const kpi = await styles(page, '.obs-kpi:nth-child(2)', ['padding-top', 'padding-right', 'padding-bottom', 'padding-left']);
  expect(`${kpi['padding-top']} ${kpi['padding-right']} ${kpi['padding-bottom']} ${kpi['padding-left']}`)
    .toBe(phone ? '12px 20px 12px 20px' : '16px 28px 16px 28px');

  // .inst-body { grid-template-columns: minmax(0, 1fr) 348px } (one column below 1240px).
  const bodyColumns = await page.locator('.obs-workbench-body').evaluate((element) => getComputedStyle(element).gridTemplateColumns.split(' ').length);
  expect(bodyColumns).toBe(phone ? 1 : 2);
  if (!phone) {
    const railWidth = await page.locator('.obs-todo-rail').evaluate((element) => Math.round(element.getBoundingClientRect().width));
    expect(railWidth).toBe(348);
  }

  // .why { padding: 20px 28px 24px; background: canvas + radial-gradient on a 16px grid }
  const why = await styles(page, '.obs-chain-block', [
    'padding-top', 'padding-right', 'padding-bottom', 'padding-left',
    'background-color', 'background-image', 'background-size',
  ]);
  expect(`${why['padding-top']} ${why['padding-right']} ${why['padding-bottom']} ${why['padding-left']}`)
    .toBe(phone ? '16px 20px 20px 20px' : '20px 28px 24px 28px');
  expect(why['background-color']).toBe(await resolvedColor(page, 'var(--craft-color-canvas)'));
  expect(why['background-image']).toContain('radial-gradient');
  expect(why['background-size']).toBe('16px 16px');
  // .why-head { gap: 12px } and .why-head .tag uses the surface with an inset hairline.
  expect((await styles(page, '.obs-workbench-chain-head', ['column-gap']))['column-gap']).toBe('12px');
  const chainTag = await styles(page, '.obs-workbench-chain-head > .obs-tag', ['background-color', 'box-shadow']);
  expect(chainTag['background-color']).toBe(await resolvedColor(page, 'var(--craft-color-surface)'));
  expect(chainTag['box-shadow']).toContain('inset');
  expect(chainTag['box-shadow']).toContain('1px');
  // .verdict .word { padding-right: 10px }
  expect((await styles(page, '.obs-workbench-health-word', ['padding-right']))['padding-right']).toBe('10px');
  // .cn.root gets the negative marker and .lamp[data-st="blocked"] is red.
  expect(await page.locator('.obs-chain-node-root .obs-lamp[data-lamp="blocked"]').count()).toBe(1);
  expect((await styles(page, '.obs-chain-node-root .obs-lamp', ['background-color']))['background-color'])
    .toBe(await resolvedColor(page, 'var(--craft-color-negative)'));
  // The complete first todo card opens the item, not only its trailing controls.
  const firstCard = page.locator('.obs-todo-card').first();
  expect(await firstCard.evaluate((element) => element.tagName)).toBe('BUTTON');
  expect((await styles(page, '.obs-todo-card', ['cursor']))['cursor']).toBe('pointer');

  // .rail { padding: 20px; border-left: 1px } (16px 20px 20px, top border below 860px)
  const rail = await styles(page, '.obs-todo-rail', ['padding-top', 'padding-right', 'padding-bottom', 'padding-left', 'border-left-width', 'border-top-width']);
  expect(`${rail['padding-top']} ${rail['padding-right']} ${rail['padding-bottom']} ${rail['padding-left']}`)
    .toBe(phone ? '16px 20px 20px 20px' : '20px 20px 20px 20px');
  expect(`${rail['border-left-width']} ${rail['border-top-width']}`).toBe(phone ? '0px 1px' : '1px 0px');

  // .panel { margin-top: 24px; border: 1px; border-radius: 16px }
  const panel = await styles(page, '.obs-detail-panel', ['margin-top', 'border-top-width', 'border-top-left-radius']);
  expect(panel).toEqual({ 'margin-top': '24px', 'border-top-width': '1px', 'border-top-left-radius': '16px' });

  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
  expect(await page.locator('body').innerText()).not.toMatch(/\b(?:undefined|null|NaN|protocol_version|workstream_id|contract_hash)\b/);
});

test('empty chain uses the flat why.plain surface instead of the dotted canvas', async ({ page }, testInfo) => {
  test.skip(testInfo.project.name === 'desktop-new-york');
  await page.locator('.obs-chain-block').evaluate((element) => { (element as HTMLElement).dataset.empty = 'true'; });
  const plain = await styles(page, '.obs-chain-block[data-empty="true"]', ['background-color', 'background-image']);
  expect(plain['background-color']).toBe(await resolvedColor(page, 'var(--craft-color-surface)'));
  expect(plain['background-image']).toBe('none');
});

test('subtitle uses the browser-local date shown by the read timestamp', async ({ page }, testInfo) => {
  test.skip(testInfo.project.name !== 'desktop-new-york');
  const byline = await page.locator('.obs-workbench-byline').innerText();
  const match = byline.match(/(\d{4})\/(\d{1,2})\/(\d{1,2})/);
  expect(match).not.toBeNull();
  const subtitle = await page.locator('.obs-page-subtitle').innerText();
  expect(subtitle).toContain(`${Number(match![2])} 月 ${Number(match![3])} 日`);
});
