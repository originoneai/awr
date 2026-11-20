import { expect, test } from '@playwright/test';
import { mkdir, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { pathToFileURL } from 'node:url';

const root = process.cwd();
const evidence = path.resolve(process.env.OBSERVER_EVIDENCE_DIR ?? path.join(root, '../../.local/ppo-020/browser'));
const prototypeFile = process.env.OBSERVER_PROTOTYPE_FILE;

// The prototype file is supplied by the local evidence command, never copied into Git.
// CI still runs the real-page metric spec when the read-only prototype bundle is unavailable.
test.skip(!prototypeFile, 'Set OBSERVER_PROTOTYPE_FILE to the local read-only panorama.html');

test('capture real page and My Work prototype side-by-side inputs', async ({ page }, testInfo) => {
  await mkdir(evidence, { recursive: true });
  const mode = testInfo.project.name === 'desktop-dark' ? 'dark' : 'light';
  await page.emulateMedia({ reducedMotion: 'reduce' });
  await page.addInitScript(({ theme }) => {
    window.localStorage.setItem('awr-observer-theme', theme);
    window.localStorage.setItem('i18nextLng', 'zh-CN');
  }, { theme: mode });
  await page.goto('/workbench?scope=P-001&perspective=me');
  await page.locator('[data-testid="workbench-frame"]').waitFor();
  await page.evaluate(async () => { await document.fonts.ready; });
  const actual = path.join(evidence, `actual-${testInfo.project.name}.png`);
  await page.screenshot({ path: actual, fullPage: true, animations: 'disabled' });

  const prototypePage = await page.context().newPage();
  await prototypePage.emulateMedia({ reducedMotion: 'reduce' });
  await prototypePage.setViewportSize(testInfo.project.use.viewport as { width: number; height: number });
  await prototypePage.addInitScript(({ theme }) => {
    try { window.localStorage.setItem('awr-proto-theme', theme); } catch { /* file origins may deny storage */ }
  }, { theme: mode });
  const prototypeUrl = new URL(pathToFileURL(prototypeFile!));
  prototypeUrl.hash = 'v=0&p=home&id=&tab=';
  await prototypePage.goto(prototypeUrl.toString());
  await prototypePage.locator('.inst').waitFor();
  await prototypePage.evaluate(async () => { await document.fonts.ready; });
  const prototype = path.join(evidence, `prototype-${testInfo.project.name}.png`);
  await prototypePage.screenshot({ path: prototype, fullPage: true, animations: 'disabled' });
  await prototypePage.close();

  const record = {
    project: testInfo.project.name,
    viewport: testInfo.project.use.viewport,
    actual,
    prototype,
    actualSource: '/workbench?scope=P-001&perspective=me on server.js --demo',
    prototypeSource: 'panorama.html home(), My Work view',
  };
  await writeFile(path.join(evidence, `capture-${testInfo.project.name}.json`), `${JSON.stringify(record, null, 2)}\n`);
  expect((await page.locator('.obs-workbench-instrument').count())).toBe(1);
});
