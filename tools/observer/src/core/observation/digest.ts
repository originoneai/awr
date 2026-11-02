import type { DigestSubtle, DigestVerdict } from './types';

/** UTF-8 byte length, not UTF-16 code units: documents are multibyte. */
export function utf8ByteLength(text: string): number {
  return new TextEncoder().encode(text).length;
}

function hex(buffer: ArrayBuffer): string {
  return Array.from(new Uint8Array(buffer)).map((byte) => byte.toString(16).padStart(2, '0')).join('');
}

/**
 * Does the text we hold hash to the SHA-256 the service declared for it? Without the text, without
 * a declared digest, without Web Crypto (`subtle`) or when hashing fails, the answer is
 * "unverified": the verdict is never "verified" unless the hashes are equal.
 */
export async function verifyTextDigest(input: {
  text: string | null | undefined;
  declaredSha256: string | null | undefined;
  subtle: DigestSubtle | null | undefined;
}): Promise<DigestVerdict> {
  const { text, declaredSha256, subtle } = input;
  if (!text || !declaredSha256 || !subtle) return 'unverified';
  try {
    const actual = hex(await subtle.digest('SHA-256', new TextEncoder().encode(text)));
    return actual === declaredSha256.trim().toLowerCase() ? 'verified' : 'mismatch';
  } catch {
    return 'unverified';
  }
}
