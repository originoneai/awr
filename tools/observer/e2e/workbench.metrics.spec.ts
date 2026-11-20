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
  if (testInfo.project.name === 'desktop-dark') expect(instrument['box-shadow']).toBe('none');
  else expect(instrument['box-shadow']).not.toBe('none');
  // .inst { background-color: var(--inst-bg) }
  expect((await styles(page, '.obs-workbench-instrument', ['background-color']))['background-color'])
    .toBe(await resolvedColor(page, 'var(--craft-instrument-background)'));

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

  // .why { padding: 20px 28px 24px } (16px 20px 20px below 860px)
  const why = await styles(page, '.obs-chain-block', ['padding-top', 'padding-right', 'padding-bottom', 'padding-left']);
  expect(`${why['padding-top']} ${why['padding-right']} ${why['padding-bottom']} ${why['padding-left']}`)
    .toBe(phone ? '16px 20px 20px 20px' : '20px 28px 24px 28px');
  // .cn.root gets the negative marker and .lamp[data-st="blocked"] is red.
  expect(await page.locator('.obs-chain-node-root .obs-lamp[data-lamp="blocked"]').count()).toBe(1);
  expect((await styles(page, '.obs-chain-node-root .obs-lamp', ['background-color']))['background-color'])
    .toBe(await resolvedColor(page, 'var(--craft-color-negative)'));

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
