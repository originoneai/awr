import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { build } from 'vite';
import { expect, it } from 'vitest';

const here = path.dirname(fileURLToPath(import.meta.url));
const observerRoot = path.resolve(here, '../../../..');

interface OutputFile {
  type: string;
  fileName: string;
}

interface BuildOutput {
  output: OutputFile[];
}

it('builds the development catalog and every statically imported example in CI', async () => {
  const result = await build({
    root: observerRoot,
    configFile: path.join(observerRoot, 'vite.config.ts'),
    logLevel: 'error',
    build: {
      write: false,
      rollupOptions: { input: path.join(here, 'index.html') },
    },
  }) as unknown as BuildOutput | BuildOutput[];
  const files = (Array.isArray(result) ? result : [result]).flatMap((output) => output.output);
  expect(files.some((file) => file.type === 'asset' && file.fileName.endsWith('.html'))).toBe(true);
  expect(files.some((file) => file.type === 'chunk' && file.fileName.endsWith('.js'))).toBe(true);
  expect(files.some((file) => file.type === 'asset' && file.fileName.endsWith('.css'))).toBe(true);
}, 60_000);
