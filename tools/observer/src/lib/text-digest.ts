import { useEffect, useState } from 'react';
import { verifyTextDigest, type DigestVerdict } from '../core/observation';

export type DigestCheck = DigestVerdict | 'pending';

/**
 * The digest check of the text on screen: pending while the hash runs, and never verified without
 * Web Crypto, without the text or without a declared digest.
 */
export function useTextDigest(text: string | null | undefined, declaredSha256: string | null | undefined): DigestCheck {
  // Starts as pending, not unverified: the first render must not show a text as "not checked"
  // before the check has even begun — a text that turns out to mismatch must never appear.
  const [check, setCheck] = useState<DigestCheck>('pending');
  useEffect(() => {
    if (!text || !declaredSha256) {
      // Nothing to check yet (or the text has gone away again): stay pending rather than
      // claiming unverified, so a text arriving in a later render is never shown under
      // "not checked" before its check runs.
      setCheck('pending');
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
