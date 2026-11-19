import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { build } from 'vite';
import { expect, it } from 'vitest';

const here = path.dirname(fileURLToPath(import.meta.url));
const observerRoot = path.resolve(here, '../../../..');

interface OutputFile {
  type: string;
  fileName: string;
  code?: string;
  source?: string;
}

interface BuildOutput {
  output: OutputFile[];
}

/** Copy that exists only in the development catalog module (catalog-messages.ts).
 *  合成数据 is deliberately absent: the production demo vocabulary already uses that word. */
const CATALOG_ONLY_MARKERS = [
  '组件目录',
  'Component catalog',
  '星桥交付',
  'Starbridge delivery',
  '更新至 11 月 19 日上午',
];

it('ships no catalog-only copy in the production bundle', async () => {
  const result = await build({
    root: observerRoot,
    configFile: path.join(observerRoot, 'vite.config.ts'),
    logLevel: 'error',
    build: {
      write: false,
      rollupOptions: { input: path.join(observerRoot, 'index.html') },
    },
  }) as unknown as BuildOutput | BuildOutput[];
  const files = (Array.isArray(result) ? result : [result]).flatMap((output) => output.output);
  expect(files.some((file) => file.type === 'chunk' && file.fileName.endsWith('.js'))).toBe(true);
  const text = files
    .map((file) => (file.type === 'chunk' ? file.code ?? '' : typeof file.source === 'string' ? file.source : ''))
    .join('\n');
  for (const marker of CATALOG_ONLY_MARKERS) {
    expect(text, `production output must not contain ${marker}`).not.toContain(marker);
  }
}, 120_000);
