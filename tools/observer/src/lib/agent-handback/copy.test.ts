// @vitest-environment node
import { readFileSync, readdirSync } from 'node:fs';
import path from 'node:path';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { buildHandback } from './build';
import { copyText } from './copy';

const MODULE_ROOT = path.resolve(process.cwd(), 'src/lib/agent-handback');

function stubClipboard(writeText: (text: string) => Promise<void>): void {
  Object.defineProperty(navigator, 'clipboard', { value: { writeText }, configurable: true });
}

describe('copyText', () => {
  afterEach(() => {
    Object.defineProperty(navigator, 'clipboard', { value: undefined, configurable: true });
    vi.unstubAllGlobals();
  });

  it('copies through the clipboard API and reports ok', async () => {
    const writeText = vi.fn(async (text: string) => {
      expect(text).toBe('接下订单协作平台派给我的任务：导出按钮的权限判断');
    });
    stubClipboard(writeText);
    await expect(copyText('接下订单协作平台派给我的任务：导出按钮的权限判断')).resolves.toEqual({ ok: true });
    expect(writeText).toHaveBeenCalledTimes(1);
  });

  it('returns the text for manual selection when the clipboard refuses', async () => {
    stubClipboard(vi.fn(async () => {
      throw new DOMException('Permission denied', 'NotAllowedError');
    }));
    await expect(copyText('回答')).resolves.toEqual({ ok: false, fallbackText: '回答' });
  });

  it('returns the text for manual selection when there is no clipboard API', async () => {
    Object.defineProperty(navigator, 'clipboard', { value: undefined, configurable: true });
    await expect(copyText('回答')).resolves.toEqual({ ok: false, fallbackText: '回答' });
  });
});

describe('the handback module performs no request of any kind', () => {
  it('never touches fetch, XMLHttpRequest or sendBeacon while building and copying', async () => {
    const fetchSpy = vi.fn(() => {
      throw new Error('the handback module must not fetch');
    });
    const beaconSpy = vi.fn(() => {
      throw new Error('the handback module must not sendBeacon');
    });
    const xhrSpy = vi.fn(() => {
      throw new Error('the handback module must not open an XMLHttpRequest');
    });
    vi.stubGlobal('fetch', fetchSpy);
    vi.stubGlobal('XMLHttpRequest', xhrSpy);
    Object.defineProperty(navigator, 'sendBeacon', { value: beaconSpy, configurable: true });
    Object.defineProperty(navigator, 'clipboard', { value: undefined, configurable: true });

    const results = ['zh-CN', 'en-US'].flatMap((language) => ['answer', 'decision', 'plan-approval', 'assignment', 'lesson']
      .map((kind) => buildHandback(
        { kind: kind as never, projectName: '订单协作平台', taskName: '空状态检查', question: '要不要加引导' },
        { person: { agentConnections: [{ name: 'Codex', kind: null, lastSeenUnixMs: null }] }, task: { executionAgentName: 'Codex' } },
        language,
      )));
    expect(results).toHaveLength(10);
    await copyText(results[0].sentence);
    await copyText(results[1].sentence);

    expect(fetchSpy).not.toHaveBeenCalled();
    expect(xhrSpy).not.toHaveBeenCalled();
    expect(beaconSpy).not.toHaveBeenCalled();
  });

  it('contains no network call in any module source', () => {
    const files = readdirSync(MODULE_ROOT).filter((file) => file.endsWith('.ts') && !file.includes('.test.'));
    expect(files.length).toBeGreaterThan(3);
    for (const file of files) {
      const source = readFileSync(path.join(MODULE_ROOT, file), 'utf8');
      expect(source, file).not.toMatch(/\bfetch\s*\(/);
      expect(source, file).not.toMatch(/XMLHttpRequest/);
      expect(source, file).not.toMatch(/sendBeacon/);
      expect(source, file).not.toMatch(/\baxios\b/);
    }
  });
});
