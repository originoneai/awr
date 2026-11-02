import { createHash, webcrypto } from 'node:crypto';
import { describe, expect, it } from 'vitest';
import { utf8ByteLength, verifyTextDigest, type DigestSubtle } from '../index';

const subtle = webcrypto.subtle as unknown as DigestSubtle;
const sha256 = (text: string) => createHash('sha256').update(text, 'utf8').digest('hex');

describe('utf8ByteLength', () => {
  it('counts UTF-8 bytes, not UTF-16 code units', () => {
    expect(utf8ByteLength('')).toBe(0);
    expect(utf8ByteLength('abcdef')).toBe(6);
    expect(utf8ByteLength('é')).toBe(2);
    expect(utf8ByteLength('中文')).toBe(6);
    expect(utf8ByteLength('😀')).toBe(4);
    expect('😀'.length).toBe(2);
  });
});

describe('verifyTextDigest', () => {
  const text = 'Issue filter API contract: query parameters, pagination bounds and the error vocabulary.\n中文 😀';

  it('verifies a text against its declared digest', async () => {
    expect(await verifyTextDigest({ text, declaredSha256: sha256(text), subtle })).toBe('verified');
  });

  it('reads the declared digest case-insensitively and ignores surrounding blanks', async () => {
    expect(await verifyTextDigest({ text, declaredSha256: `  ${sha256(text).toUpperCase()}\n`, subtle })).toBe('verified');
  });

  it('says mismatch for a text changed by one character of the same length', async () => {
    expect(await verifyTextDigest({ text: 'abd', declaredSha256: sha256('abc'), subtle })).toBe('mismatch');
    expect(await verifyTextDigest({ text, declaredSha256: sha256(`${text} `), subtle })).toBe('mismatch');
    expect(await verifyTextDigest({ text, declaredSha256: 'not-a-digest', subtle })).toBe('mismatch');
  });

  it('is never verified without Web Crypto, without the text or without a declared digest', async () => {
    for (const subtleValue of [null, undefined]) {
      expect(await verifyTextDigest({ text, declaredSha256: sha256(text), subtle: subtleValue })).toBe('unverified');
    }
    for (const missing of [null, undefined, '']) {
      expect(await verifyTextDigest({ text: missing, declaredSha256: sha256(text), subtle })).toBe('unverified');
      expect(await verifyTextDigest({ text, declaredSha256: missing, subtle })).toBe('unverified');
    }
  });

  it('says unverified, not mismatch and not verified, when hashing itself fails', async () => {
    const broken: DigestSubtle = { digest: () => Promise.reject(new Error('no hashing here')) };
    expect(await verifyTextDigest({ text, declaredSha256: sha256(text), subtle: broken })).toBe('unverified');
    const throwing: DigestSubtle = { digest: () => { throw new Error('sync failure'); } };
    expect(await verifyTextDigest({ text, declaredSha256: sha256(text), subtle: throwing })).toBe('unverified');
  });

  it('hashes the UTF-8 bytes of the text with SHA-256 and nothing else', async () => {
    const seen: Array<{ algorithm: string; bytes: number[] }> = [];
    const spy: DigestSubtle = {
      digest: async (algorithm, data) => {
        seen.push({ algorithm, bytes: Array.from(data) });
        return webcrypto.subtle.digest('SHA-256', data);
      },
    };
    expect(await verifyTextDigest({ text: 'é', declaredSha256: sha256('é'), subtle: spy })).toBe('verified');
    expect(seen).toEqual([{ algorithm: 'SHA-256', bytes: [0xc3, 0xa9] }]);
  });
});
