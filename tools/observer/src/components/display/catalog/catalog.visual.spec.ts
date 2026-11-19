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
  'perspective',
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

test('perspective arrow navigation, Enter or Space selection, and visible focus', async ({ page }) => {
  await page.goto('/src/components/display/catalog/index.html?component=perspective');
  const me = page.getByRole('button', { name: '做事的人' });
  const lead = page.getByRole('button', { name: '小组负责人' });
  const department = page.getByRole('button', { name: '部门负责人' });
  await me.focus();
  await page.keyboard.press('ArrowRight');
  await expect(lead).toBeFocused();
  await expect(me).toHaveAttribute('aria-pressed', 'true');
  await expect(lead).toHaveAttribute('aria-pressed', 'false');
  const outline = await lead.evaluate((element) => {
    const style = getComputedStyle(element);
    return { style: style.outlineStyle, width: style.outlineWidth };
  });
  expect(outline).toEqual({ style: 'solid', width: '2px' });
  await page.keyboard.press('Enter');
  await expect(lead).toHaveAttribute('aria-pressed', 'true');
  await page.keyboard.press('ArrowRight');
  await expect(department).toBeFocused();
  await page.keyboard.press('Space');
  await expect(department).toHaveAttribute('aria-pressed', 'true');
});
