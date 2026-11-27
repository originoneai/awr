import { settle } from './shared.mjs';

export async function acceptanceDetail(page) {
  await page.getByTestId('acceptance-evidence-chain').waitFor();
  await settle(page);
  const earlier = page.getByRole('button', { name: /第 1 次复核|Review 1/ });
  if (await earlier.count()) {
    await earlier.first().click();
    await page.getByTestId('acceptance-review-round-t-101-1').waitFor();
    const latest = page.getByRole('button', { name: /返回最近一次复核|Return to latest review/ });
    if (await latest.count()) await latest.click();
  }
  const artifact = page.locator('[data-testid^="artifact-expand-"]').first();
  if (await artifact.count()) {
    await artifact.click();
    await page.locator('[data-testid^="artifact-read-content-"]').first().waitFor();
    await page.locator('[data-testid^="artifact-digest-"]').first().waitFor();
  }
  await settle(page);
}

export default {
  name: 'project-acceptance',
  routeActions: {
    'project-acceptance': acceptanceDetail,
  },
};
