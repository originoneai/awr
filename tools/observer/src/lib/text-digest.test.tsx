import { createHash, webcrypto } from 'node:crypto';
import { act, renderHook, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { useTextDigest } from './text-digest';

const sha256 = (text: string) => createHash('sha256').update(text, 'utf8').digest('hex');

afterEach(() => {
  vi.unstubAllGlobals();
});

describe('useTextDigest', () => {
  it('is pending while the hash runs and then verified for an untouched text', async () => {
    vi.stubGlobal('crypto', webcrypto);
    const { result } = renderHook(() => useTextDigest('abc', sha256('abc')));
    expect(['pending', 'unverified']).toContain(result.current);
    await waitFor(() => expect(result.current).toBe('verified'));
  });

  it('says mismatch for a text that does not hash to the declared digest', async () => {
    vi.stubGlobal('crypto', webcrypto);
    const { result } = renderHook(() => useTextDigest('abd', sha256('abc')));
    await waitFor(() => expect(result.current).toBe('mismatch'));
  });

  it('checks again when only the declared digest changes for the same text', async () => {
    vi.stubGlobal('crypto', webcrypto);
    const { result, rerender } = renderHook(({ digest }) => useTextDigest('abc', digest), { initialProps: { digest: sha256('abc') } });
    await waitFor(() => expect(result.current).toBe('verified'));
    rerender({ digest: sha256('abd') });
    await waitFor(() => expect(result.current).toBe('mismatch'));
  });

  it('says unverified at once, never verified, without Web Crypto', async () => {
    vi.stubGlobal('crypto', {});
    const { result } = renderHook(() => useTextDigest('abc', sha256('abc')));
    expect(result.current).toBe('unverified');
    await act(async () => { await new Promise((resolve) => setTimeout(resolve, 30)); });
    expect(result.current).toBe('unverified');
  });

  it('says unverified without the text or the declared digest', () => {
    vi.stubGlobal('crypto', webcrypto);
    for (const [text, digest] of [[null, sha256('abc')], ['abc', null], [undefined, undefined], ['', sha256('')]] as const) {
      const { result } = renderHook(() => useTextDigest(text, digest));
      expect(result.current, String(text)).toBe('unverified');
    }
  });

  it('lets a slow answer for an earlier text never land on a later one', async () => {
    const releases: Array<() => void> = [];
    const slow = {
      subtle: {
        digest: (algorithm: string, data: Uint8Array) => new Promise<ArrayBuffer>((resolve) => {
          releases.push(() => { void webcrypto.subtle.digest(algorithm, data).then(resolve); });
        }),
      },
    };
    vi.stubGlobal('crypto', slow);
    const { result, rerender } = renderHook(({ text }) => useTextDigest(text, sha256('first')), { initialProps: { text: 'first' } });
    await waitFor(() => expect(releases).toHaveLength(1));
    rerender({ text: 'second' });
    await waitFor(() => expect(releases).toHaveLength(2));
    await act(async () => { releases[0](); await new Promise((resolve) => setTimeout(resolve, 20)); });
    expect(result.current).toBe('pending');
    await act(async () => { releases[1](); });
    await waitFor(() => expect(result.current).toBe('mismatch'));
  });
});
