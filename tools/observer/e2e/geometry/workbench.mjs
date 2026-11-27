import assert from 'node:assert/strict';
import { settle } from './shared.mjs';

export async function workbenchGeometry(page) {
  return page.evaluate(() => {
    const title = getComputedStyle(document.querySelector('.obs-page-title'));
    const panel = getComputedStyle(document.querySelector('.obs-detail-panel'));
    const body = getComputedStyle(document.querySelector('.obs-workbench-body'));
    const rail = document.querySelector('.obs-todo-rail').getBoundingClientRect();
    return {
      titleSize: title.fontSize,
      titleLeading: title.lineHeight,
      panelRadius: panel.borderTopLeftRadius,
      bodyColumns: body.gridTemplateColumns.split(' ').length,
      railWidth: Math.round(rail.width),
      overflow: document.documentElement.scrollWidth > document.documentElement.clientWidth,
    };
  });
}

export function checkWorkbenchGeometry(geometry, viewportWidth) {
  assert.deepEqual([geometry.titleSize, geometry.titleLeading, geometry.panelRadius], ['22px', '28px', '16px']);
  assert.equal(geometry.overflow, false, 'workbench has no horizontal overflow');
  if (viewportWidth >= 1240) {
    assert.equal(geometry.bodyColumns, 2);
    assert.equal(geometry.railWidth, 348);
  } else {
    assert.equal(geometry.bodyColumns, 1);
  }
}

export async function workbenchQueue(page) {
  await page.getByTestId('workbench-frame').waitFor();
  assert.equal(await page.locator('.obs-kpi').count(), 4, 'workbench has four KPI cells');
  assert.equal(await page.locator('.obs-workbench-instrument').count(), 1, 'workbench has one verdict instrument');
  assert.equal(await page.locator('.obs-detail-panel').count(), 1, 'workbench has one detail panel');
  assert.ok(await page.locator('.obs-chain-node-root').count() <= 1, 'one visible chain has at most one root');
  if (await page.locator('.obs-chain-node-root').count()) {
    assert.equal(await page.locator('.obs-chain-node-root .obs-lamp[data-lamp="blocked"]').count(), 1, 'chain root is red');
  }
  const geometry = await workbenchGeometry(page);
  checkWorkbenchGeometry(geometry, page.viewportSize().width);
  await page.locator('.obs-kpi').first().click();
  await page.getByTestId('workbench-metric-detail').waitFor();
  const detailCount = Number(await page.getByTestId('workbench-metric-detail').getAttribute('data-count'));
  assert.equal(await page.locator('#workbench-metric-detail, [data-testid="workbench-metric-detail"]').locator('.obs-row-item').count(), detailCount,
    'KPI detail rows equal the number');
  await page.getByRole('button', { name: /返回工作台|Back to workbench/ }).click();
  await page.getByRole('tab', { name: /我在等|Waiting on others/ }).click();
  await settle(page);
}

/* The canonical variant exists so the workbench can be compared with the prototype with
   every fact known. The server promises are asserted, and so are the page's four KPI
   numbers: under the canonical variant they must equal the prototype's [2, 1, 2, 3]; a
   red number assertion means the page or the derivation regressed. */
export async function recordCanonicalWorkbench(page) {
  await settle(page);
  await page.getByTestId('workbench-frame').waitFor();
  const pageNumbers = await page.evaluate(() =>
    [...document.querySelectorAll('.obs-kpis .obs-kpi-value')]
      .map((element) => (element.textContent ?? '').trim())
      .map((text) => (/^(\d+)/.exec(text) ?? [])[0] ?? null));
  assert.deepEqual(pageNumbers, ['2', '1', '2', '3'],
    'the canonical workbench KPI numbers must equal the prototype [2, 1, 2, 3]');
  const response = await page.request
    .get(new URL('/api/team/overview-model?project=P-001', page.url()).href);
  assert.equal(response.status(), 200);
  const model = await response.json();
  assert.equal(model.ok, true);
  for (const [plane, detail] of Object.entries(model.coverage)) {
    assert.deepEqual(detail, { status: 'complete', missing: [] }, `canonical ${plane} coverage`);
  }
  assert.deepEqual(model.input.options.views.me, { personId: 'M-002', projectId: 'P-001' });
  assert.equal(model.input.tasks.items.length, 14);
  assert.equal(model.input.approvals.items.length, 2);
  // Demo clock decoupling (the #709 review decision): read times and every freshness
  // measurement report the moment the server answered, so the badge reads measured on
  // any day the demo runs. The derivation clock stays on the whole-week fixture time
  // and keeps trailing the answer by at most half a week.
  assert.equal(model.freshness.authoritative.status, 'measured',
    `canonical authoritative status ${model.freshness.authoritative.status}`);
  assert.ok(typeof model.freshness.authoritative.age_ms === 'number'
    && model.freshness.authoritative.age_ms >= 0 && model.freshness.authoritative.age_ms < 60_000,
    `canonical authoritative age ${model.freshness.authoritative.age_ms} is not the answer moment's age`);
  assert.ok(Math.abs(model.input.options.nowUnixMs - model.read_at_unix_ms) <= 3.5 * 24 * 3600 * 1000,
    'the canonical "now" stays within half a week of the real read');
  // End-to-end default clock check: the fixture's "now" is a Thursday 12:00 UTC, and the
  // whole-week rounding keeps it there through the real server.js startup path.
  assert.equal(new Date(model.input.options.nowUnixMs).getUTCDay(), 4,
    'the demo clock keeps the fixture weekday');
  assert.equal(model.input.options.nowUnixMs % (24 * 3600 * 1000), (12 * 3600 * 1000),
    'the demo clock keeps the fixture time of day');
  return {
    route: '/workbench?scope=P-001&perspective=me',
    prototypeNumbers: [2, 1, 2, 3],
    pageNumbers,
    modelReadAtUnixMs: model.read_at_unix_ms,
    modelNowUnixMs: model.input.options.nowUnixMs,
  };
}

export default {
  name: 'workbench',
  routeActions: {
    workbench: workbenchQueue,
  },
  canonical: {
    reportKey: 'canonicalWorkbenchRecords',
    route: {
      name: 'workbench-canonical',
      path: '/workbench?scope=P-001&perspective=me',
      // Record the default tab first (where the KPI numbers live), then take the
      // same queue-and-detail walk the full variant uses, so the shared body checks
      // see the same settled state they already guard for the full variant.
      run: async (page, recordCanonical) => {
        await settle(page);
        recordCanonical(await recordCanonicalWorkbench(page));
        await workbenchQueue(page);
      },
    },
  },
};
