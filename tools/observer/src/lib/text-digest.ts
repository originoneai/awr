import { useEffect, useState } from 'react';
import { verifyTextDigest, type DigestVerdict } from '../core/observation';

export type DigestCheck = DigestVerdict | 'pending';

/**
 * The digest check of the text on screen: pending while the hash runs, and never verified without
 * Web Crypto, without the text or without a declared digest.
 */
export function useTextDigest(text: string | null | undefined, declaredSha256: string | null | undefined): DigestCheck {
  const [check, setCheck] = useState<DigestCheck>('unverified');
  useEffect(() => {
    if (!text || !declaredSha256) {
      setCheck('unverified');
      return;
    }
    const subtle = typeof crypto === 'undefined' ? null : crypto.subtle ?? null;
    if (!subtle) {
      setCheck('unverified');
      return;
    }
    let cancelled = false;
    setCheck('pending');
    void verifyTextDigest({ text, declaredSha256, subtle }).then((verdict) => {
      if (!cancelled) setCheck(verdict);
    });
    return () => { cancelled = true; };
  }, [text, declaredSha256]);
  return check;
}
