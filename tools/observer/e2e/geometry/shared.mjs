export async function settle(page) {
  await page.waitForLoadState('domcontentloaded');
  await page.waitForFunction(() => document.querySelectorAll('.ant-spin-spinning').length === 0, null, { timeout: 20_000 });
  await page.waitForTimeout(250);
}

export function parseMetricNumber(value) {
  return typeof value === 'number' ? value : Number.parseFloat(value ?? '');
}

export function createEqual(failures) {
  return (actual, expected, label) => {
    if (Math.abs(parseMetricNumber(actual) - expected) > 0.01) {
      failures.push(`${label}: expected ${expected}, got ${actual}`);
    }
  };
}
