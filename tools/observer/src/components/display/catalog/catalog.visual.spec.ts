import { expect, test } from '@playwright/test';

const components = [
  'section',
  'pageHeader',
  'pageHeaderCompact',
  'notice',
  'empty',
  'keyValue',
  'rowList',
  'dataTable',
  'detailPanel',
  'timeline',
  'kpiStrip',
  'statusTag',
  'completionBar',
  'chain',
  'todoRail',
  'acceptanceList',
] as const;

test.beforeEach(async ({ page }, testInfo) => {
  await page.emulateMedia({ reducedMotion: 'reduce' });
  const mode = testInfo.project.name === 'desktop-dark' ? 'dark' : 'light';
  await page.addInitScript((themeMode) => {
    window.localStorage.setItem('awr-observer-theme', themeMode);
    window.localStorage.setItem('i18nextLng', 'zh-CN');
  }, mode);
});

for (const component of components) {
  test(component, async ({ page }, testInfo) => {
    await page.goto(`/src/components/display/catalog/index.html?component=${component}`);
    const canvas = page.getByTestId('example-canvas');
    await expect(canvas).toBeVisible();
    await expect(page.locator('html')).toHaveAttribute(
      'data-theme',
      testInfo.project.name === 'desktop-dark' ? 'dark' : 'light',
    );
    await page.evaluate(async () => {
      await document.fonts.ready;
    });
    await expect(canvas).toHaveScreenshot(`${component}.png`);
  });
}

test('dataTable keyboard focus and Enter navigation', async ({ page }) => {
  await page.goto('/src/components/display/catalog/index.html?component=dataTable');
  const row = page.locator('tr[data-openable="true"]').first();
  await row.focus();
  const outline = await row.evaluate((element) => {
    const style = getComputedStyle(element);
    return { style: style.outlineStyle, width: style.outlineWidth };
  });
  expect(outline).toEqual({ style: 'solid', width: '2px' });
  await page.keyboard.press('Enter');
  await expect(page.getByText('已打开工作：核对页面交互')).toBeVisible();
  const staticRow = page.getByText('复查交付说明').locator('tr');
  await expect(staticRow.locator('.obs-table-arrow-column svg')).toHaveCount(0);
});

test('kpi cells open with a visible keyboard focus ring', async ({ page }) => {
  await page.goto('/src/components/display/catalog/index.html?component=kpiStrip');
  const cell = page.locator('button.obs-kpi-open').first();
  await cell.focus();
  const outline = await cell.evaluate((element) => {
    const style = getComputedStyle(element);
    return { style: style.outlineStyle, width: style.outlineWidth, color: style.outlineColor };
  });
  expect(outline.style).toBe('solid');
  expect(outline.width).toBe('2px');
  await page.keyboard.press('Enter');
  await expect(page.getByText('已打开数字：本周通过核验')).toBeVisible();
});

test('chain nodes open with the keyboard and keep their red root marker', async ({ page }) => {
  await page.goto('/src/components/display/catalog/index.html?component=chain');
  const root = page.locator('button.obs-chain-node-root');
  await expect(root).toHaveCount(1);
  await root.focus();
  const outline = await root.evaluate((element) => {
    const style = getComputedStyle(element);
    return { style: style.outlineStyle, width: style.outlineWidth };
  });
  expect(outline).toEqual({ style: 'solid', width: '2px' });
  await page.keyboard.press('Enter');
  await expect(page.getByText('已打开这一环：周宁的复核队列')).toBeVisible();
  const quietEnd = page.locator('.obs-chain-node[data-node-kind="commitment"]');
  await expect(quietEnd).toHaveCount(1);
  await expect(quietEnd.locator('button, a')).toHaveCount(0);
});

test('todo rail collapses everything but the first card', async ({ page }) => {
  await page.goto('/src/components/display/catalog/index.html?component=todoRail');
  const cards = page.locator('.obs-todo-card');
  await expect(cards).toHaveCount(3);
  await expect(page.locator('.obs-todo-detail')).toHaveCount(1);
  await expect(page.getByText('还有 2 件')).toBeVisible();
  await page.locator('.obs-todo-min').first().click();
  await expect(page.getByText('已打开待办：要不要为这条工作线增加一位复核人')).toBeVisible();
});
