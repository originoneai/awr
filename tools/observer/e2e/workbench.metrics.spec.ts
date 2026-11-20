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
  expect.soft(await styles(page, '.obs-page-title', ['font-size', 'line-height', 'font-weight']))
    .toEqual({ 'font-size': '22px', 'line-height': '28px', 'font-weight': '500' });
  // .head .sub { font-size: 13px }; home() prefixes the period with the viewer's name.
  expect.soft((await styles(page, '.obs-page-subtitle', ['font-size']))['font-size']).toBe('13px');
  await expect.soft(page.locator('.obs-page-subtitle')).toContainText('林悦 · 今天，');
  // .head { padding: 24px 0 16px } (16px 0 12px below 860px)
  const heading = await styles(page, '.obs-page-header', ['padding-top', 'padding-bottom']);
  expect.soft(`${heading['padding-top']} ${heading['padding-bottom']}`).toBe(phone ? '16px 12px' : '24px 16px');

  // Third-draft .inst { border-radius: 16px; background-image: none; box-shadow: var(--shadow-f) }
  const instrument = await styles(page, '.obs-workbench-instrument', ['border-top-left-radius', 'background-image', 'box-shadow']);
  expect.soft(instrument['border-top-left-radius']).toBe('16px');
  expect.soft(instrument['background-image']).toBe('none');
  expect.soft(instrument['box-shadow']).toBe(testInfo.project.name === 'desktop-dark'
    ? 'rgba(0, 0, 0, 0.5) 0px 16px 40px 0px'
    : 'rgba(17, 24, 39, 0.1) 0px 8px 24px 0px');
  // .inst follows the page surface in the third draft.
  expect.soft((await styles(page, '.obs-workbench-instrument', ['background-color']))['background-color'])
    .toBe(await resolvedColor(page, 'var(--craft-color-surface)'));

  // .inst-top { display: flex; align-items: flex-start; gap: 24px; padding: 24px 28px 20px }
  // below 860px: flex-direction column, gap 16px, padding 20px 20px 16px.
  const top = await styles(page, '.obs-workbench-verdict-top', [
    'display', 'align-items', 'flex-direction', 'column-gap', 'padding-top', 'padding-right', 'padding-bottom', 'padding-left',
  ]);
  expect.soft(top.display).toBe('flex');
  expect.soft(top['align-items']).toBe('flex-start');
  expect.soft(top['flex-direction']).toBe(phone ? 'column' : 'row');
  expect.soft(top['column-gap']).toBe(phone ? '16px' : '24px');
  expect.soft(`${top['padding-top']} ${top['padding-right']} ${top['padding-bottom']} ${top['padding-left']}`)
    .toBe(phone ? '20px 20px 16px 20px' : '24px 28px 20px 28px');

  // .verdict { flex: 1; min-width: 0 }
  const verdict = await styles(page, '.obs-workbench-verdict', ['flex-grow', 'min-width']);
  expect.soft(verdict['flex-grow']).toBe('1');
  expect.soft(verdict['min-width']).toBe('0px');
  // .verdict .say { margin: 12px 0 0; font-size: 24px; line-height: 32px; font-weight: 500 }
  // below 860px: 20px/28px.
  const say = await styles(page, '.obs-workbench-sentence', ['margin-top', 'font-size', 'line-height', 'font-weight']);
  expect.soft(say).toEqual({
    'margin-top': '12px', 'font-size': phone ? '20px' : '24px',
    'line-height': phone ? '28px' : '32px', 'font-weight': '500',
  });
  // .verdict .by { margin-top: 8px; font-size: 13px }
  const byline = await styles(page, '.obs-workbench-byline', ['margin-top', 'font-size']);
  expect.soft(byline).toEqual({ 'margin-top': '8px', 'font-size': '13px' });

  // .kpis { grid 4 columns } (2 below 860px), .kpi { padding: 16px 28px } (12px 20px).
  const tracks = await page.locator('.obs-kpis').evaluate((element) => getComputedStyle(element).gridTemplateColumns.split(' ').length);
  expect.soft(tracks).toBe(phone ? 2 : 4);
  const kpi = await styles(page, '.obs-kpi:nth-child(2)', ['padding-top', 'padding-right', 'padding-bottom', 'padding-left']);
  expect.soft(`${kpi['padding-top']} ${kpi['padding-right']} ${kpi['padding-bottom']} ${kpi['padding-left']}`)
    .toBe(phone ? '12px 20px 12px 20px' : '16px 28px 16px 28px');

  // .inst-body { grid-template-columns: minmax(0, 1fr) 348px } (one column below 1240px).
  const bodyColumns = await page.locator('.obs-workbench-body').evaluate((element) => getComputedStyle(element).gridTemplateColumns.split(' ').length);
  expect.soft(bodyColumns).toBe(phone ? 1 : 2);
  if (!phone) {
    const railWidth = await page.locator('.obs-todo-rail').evaluate((element) => Math.round(element.getBoundingClientRect().width));
    expect.soft(railWidth).toBe(348);
  }

  // .why { padding: 20px 28px 24px; background: canvas + radial-gradient on a 16px grid }
  const why = await styles(page, '.obs-chain-block', [
    'padding-top', 'padding-right', 'padding-bottom', 'padding-left',
    'background-color', 'background-image', 'background-size',
  ]);
  expect.soft(`${why['padding-top']} ${why['padding-right']} ${why['padding-bottom']} ${why['padding-left']}`)
    .toBe(phone ? '16px 20px 20px 20px' : '20px 28px 24px 28px');
  expect.soft(why['background-color']).toBe(await resolvedColor(page, 'var(--craft-color-canvas)'));
  expect.soft(why['background-image']).toContain('radial-gradient');
  expect.soft(why['background-size']).toBe('16px 16px');
  // .why-head { gap: 12px } and .why-head .tag uses the surface with an inset hairline.
  expect.soft((await styles(page, '.obs-workbench-chain-head', ['column-gap']))['column-gap']).toBe('12px');
  const chainTag = await styles(page, '.obs-workbench-chain-head > .obs-tag', ['background-color', 'box-shadow']);
  expect.soft(chainTag['background-color']).toBe(await resolvedColor(page, 'var(--craft-color-surface)'));
  expect.soft(chainTag['box-shadow']).toContain('inset');
  expect.soft(chainTag['box-shadow']).toContain('1px');
  // .verdict .word { padding-right: 10px }
  expect.soft((await styles(page, '.obs-workbench-health-word', ['padding-right']))['padding-right']).toBe('10px');
  // .cn.root gets the negative marker and .lamp[data-st="blocked"] is red.
  expect.soft(await page.locator('.obs-chain-node-root .obs-lamp[data-lamp="blocked"]').count()).toBe(1);
  expect.soft((await styles(page, '.obs-chain-node-root .obs-lamp', ['background-color']))['background-color'])
    .toBe(await resolvedColor(page, 'var(--craft-color-negative)'));
  // The complete first todo card opens the item, not only its trailing controls.
  const firstCard = page.locator('.obs-todo-card').first();
  expect.soft(await firstCard.evaluate((element) => element.tagName)).toBe('BUTTON');
  expect.soft((await styles(page, '.obs-todo-card', ['cursor']))['cursor']).toBe('pointer');

  // .rail { padding: 20px; border-left: 1px } (16px 20px 20px, top border below 860px)
  const rail = await styles(page, '.obs-todo-rail', ['padding-top', 'padding-right', 'padding-bottom', 'padding-left', 'border-left-width', 'border-top-width']);
  expect.soft(`${rail['padding-top']} ${rail['padding-right']} ${rail['padding-bottom']} ${rail['padding-left']}`)
    .toBe(phone ? '16px 20px 20px 20px' : '20px 20px 20px 20px');
  expect.soft(`${rail['border-left-width']} ${rail['border-top-width']}`).toBe(phone ? '0px 1px' : '1px 0px');

  // .panel { margin-top: 24px; border: 1px; border-radius: 16px }
  const panel = await styles(page, '.obs-detail-panel', ['margin-top', 'border-top-width', 'border-top-left-radius']);
  expect.soft(panel).toEqual({ 'margin-top': '24px', 'border-top-width': '1px', 'border-top-left-radius': '16px' });

  // .ptabs { min-height: 48px; border-bottom: 1px solid var(--line) } and its tabs pull up
  // 1px over the hairline (.ptabs button.tab { height: 48px; margin-bottom: -1px }).
  const tabsNav = await styles(page, '.obs-detail-tabs .ant-tabs-nav', ['height', 'border-bottom-width', 'border-bottom-color', 'padding-left']);
  expect.soft(tabsNav.height).toBe('48px');
  expect.soft(tabsNav['border-bottom-width']).toBe('1px');
  expect.soft(tabsNav['border-bottom-color']).toBe(await resolvedColor(page, 'var(--craft-color-hairline)'));
  expect.soft(tabsNav['padding-left']).toBe(phone ? '16px' : '20px');
  const tabButton = await styles(page, '.obs-detail-tabs .ant-tabs-tab', ['height', 'margin-bottom']);
  expect.soft(tabButton).toEqual({ height: '48px', 'margin-bottom': '-1px' });
  // A plain <button> keeps its default centre alignment in the prototype.
  expect.soft((await styles(page, '.obs-detail-tabs .ant-tabs-tab-btn', ['text-align']))['text-align']).toBe('center');

  // .t { border-collapse: collapse } — rows carry the head's border exactly like the prototype.
  expect.soft((await styles(page, '.obs-detail-panel table', ['border-collapse']))['border-collapse']).toBe('collapse');
  // .t td { border-top: 1px; no bottom border } so nothing is drawn under the last row;
  // the first body row drops its top border exactly like .t tbody tr:first-child td.
  const firstBodyCell = await styles(page, '.obs-detail-panel .ant-table-tbody tr:first-child td', ['border-top-width', 'border-bottom-width', 'border-bottom-style']);
  expect.soft(`${firstBodyCell['border-top-width']} ${firstBodyCell['border-bottom-width']} ${firstBodyCell['border-bottom-style']}`).toBe('0px 0px none');
  const secondBodyCell = await styles(page, '.obs-detail-panel .ant-table-tbody tr:nth-child(2) td', ['border-top-width', 'border-bottom-width']);
  expect.soft(`${secondBodyCell['border-top-width']} ${secondBodyCell['border-bottom-width']}`).toBe('1px 0px');
  // .t td.chevc { padding-right: 16px; color: ink-3; text-align: right; line-height: 0 }
  const arrow = await styles(page, '.obs-detail-panel td.obs-table-arrow-column', ['padding-right', 'color', 'text-align', 'line-height', 'width']);
  expect.soft(arrow).toEqual({
    'padding-right': '16px',
    color: await resolvedColor(page, 'var(--craft-color-muted)'),
    'text-align': 'right',
    'line-height': '0px',
    width: '36px',
  });

  // .who { gap: 10px } with the agent name in a 32px .ki box; .who .n { font-weight: 500 }.
  const agentCell = await styles(page, '.obs-detail-panel td:first-child .obs-workbench-agent-name', ['column-gap']);
  expect.soft(agentCell['column-gap']).toBe('10px');
  const badge = await styles(page, '.obs-detail-panel .obs-workbench-agent-badge', ['width', 'height', 'border-top-left-radius', 'background-color', 'color']);
  expect.soft(`${badge.width} ${badge.height} ${badge['border-top-left-radius']}`).toBe('32px 32px 8px');
  expect.soft(badge['background-color']).toBe(await resolvedColor(page, 'var(--craft-color-raised)'));
  expect.soft(badge.color).toBe(await resolvedColor(page, 'var(--craft-color-secondary)'));
  expect.soft((await styles(page, '.obs-detail-panel .obs-workbench-agent-name > span:last-child', ['font-weight']))['font-weight']).toBe('500');

  // .kpi .v and .rail-head .n keep the prototype's proportional digits (no .num class).
  expect.soft((await styles(page, '.obs-kpi-value', ['font-variant-numeric']))['font-variant-numeric']).toBe('normal');
  expect.soft((await styles(page, '.obs-todo-count', ['font-variant-numeric']))['font-variant-numeric']).toBe('normal');
  // .todo .r1 inherits the body's 20px leading.
  expect.soft((await styles(page, '.obs-todo-first-meta', ['line-height']))['line-height']).toBe('20px');
  // .where-s { font-size: 12px } even when the span is the keyboard control.
  expect.soft((await styles(page, '.obs-todo-acts .obs-todo-agent', ['font-size']))['font-size']).toBe('12px');
  // The count columns the prototype marks .num (tabular digits): the age and behind columns
  // (the age column itself is one of the small-screen hides in both designs).
  if (!phone) {
    expect.soft((await styles(page, '.obs-detail-panel th.obs-table-numeric-column', ['font-variant-numeric']))['font-variant-numeric']).toBe('tabular-nums');
  }

  // .cn keeps a readable floor and the row scrolls instead of squeezing: the node min-width
  // token holds, and the row only enters scroll mode when the nodes cannot fit.
  const nodeMin = await styles(page, '.obs-workbench-body .obs-chain-node', ['min-width']);
  expect.soft(nodeMin['min-width']).toBe('132px');
  const chainRow = page.locator('.obs-workbench-body .obs-chain');
  const overflowState = await chainRow.evaluate((element) => ({
    flag: element.getAttribute('data-overflow'),
    fits: element.scrollWidth <= element.clientWidth + 1,
  }));
  expect.soft(overflowState.fits).toBe(true);
  if (overflowState.flag === 'true') {
    const scrolled = await chainRow.evaluate((element) => getComputedStyle(element).overflowX);
    expect.soft(scrolled).toBe('auto');
  } else {
    expect.soft(await chainRow.evaluate((element) => getComputedStyle(element).overflowX)).toBe('visible');
  }

  // Prototype dark: the selected segment's shadow declaration mixes "none" into a list and
  // computes to none (invalid at computed-value time); the ring exists in light mode only.
  if (testInfo.project.name === 'desktop-dark') {
    expect.soft((await styles(page, '.obs-workbench-chain-switch button[aria-pressed="true"]', ['box-shadow']))['box-shadow']).toBe('none');
  }

  expect.soft(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
  expect.soft(await page.locator('body').innerText()).not.toMatch(/\b(?:undefined|null|NaN|protocol_version|workstream_id|contract_hash)\b/);
});

test('empty chain uses the flat why.plain surface instead of the dotted canvas', async ({ page }, testInfo) => {
  test.skip(testInfo.project.name === 'desktop-new-york');
  await page.locator('.obs-chain-block').evaluate((element) => { (element as HTMLElement).dataset.empty = 'true'; });
  const plain = await styles(page, '.obs-chain-block[data-empty="true"]', ['background-color', 'background-image']);
  expect.soft(plain['background-color']).toBe(await resolvedColor(page, 'var(--craft-color-surface)'));
  expect.soft(plain['background-image']).toBe('none');
});

test('subtitle uses the viewer name and browser-local date from the read timestamp', async ({ page }, testInfo) => {
  test.skip(testInfo.project.name !== 'desktop-new-york');
  const expected = await page.evaluate(async () => {
    const response = await fetch('/api/team/overview-model?project=P-001');
    const body = await response.json() as { read_at_unix_ms: number };
    const date = new Date(body.read_at_unix_ms);
    return `${date.getMonth() + 1} 月 ${date.getDate()} 日`;
  });
  const subtitle = await page.locator('.obs-page-subtitle').innerText();
  expect.soft(subtitle).toBe(`林悦 · 今天，${expected}`);
});
