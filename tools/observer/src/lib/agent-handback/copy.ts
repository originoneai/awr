/**
 * Copying a handback line is a browser-only concern: the only API used is
 * navigator.clipboard.writeText. No request of any kind is made, and when the clipboard
 * is unavailable or refuses, the same text comes back so the page can select it for the
 * person to copy by hand.
 */

export type CopyTextResult = { ok: true } | { ok: false; fallbackText: string };

export async function copyText(text: string): Promise<CopyTextResult> {
  const clipboard = typeof navigator === 'undefined' ? undefined : navigator.clipboard;
  if (!clipboard || typeof clipboard.writeText !== 'function') {
    return { ok: false, fallbackText: text };
  }
  try {
    await clipboard.writeText(text);
    return { ok: true };
  } catch {
    return { ok: false, fallbackText: text };
  }
}
