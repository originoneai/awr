import { describe, expect, it } from 'vitest';
import { assembleSegments, judgeContentRead } from '../content';

/**
 * Real-shape drivers, cut from the live samples of the TF-014AN rehearsal run-09
 * (service-artifact-segment-api-run09.json, 2753 bytes, digest 37f728ac…): the base64 payloads
 * below are the file's first 192 bytes, split exactly as the service splits pages.
 */
const PAGE_A_B64 = 'aXNzdWUtZmlsdGVyLWFwaSBmaXJzdCB2ZXJzaW9uCkZpbHRlcnM6IHN0YXR1cyBhbmQga2V5d29yZCBjb21iaW5lIHdpdGggQU5ELiBCbGFuayB2YWx1ZXMgYXJlIGln';
const PAGE_B_B64 = 'bm9yZWQuCktleXdvcmQgbWF0Y2hlcyB0aXRsZSBvbmx5LCBjYXNlLWluc2Vuc2l0aXZlLiBJc3N1ZSByZWNvcmRzIGFyZSBub3Qgd3JpdHRlbi4KbnBtIHRlc3Q6IDUg';

function bytesOf(base64: string): Uint8Array {
  return Uint8Array.from(atob(base64), (char) => char.charCodeAt(0));
}

const PAGE_A = bytesOf(PAGE_A_B64);
const PAGE_B = bytesOf(PAGE_B_B64);

/** The multi-byte character 费 spans a page boundary when split after its first UTF-8 byte. */
const FEI = new TextEncoder().encode('费');

describe('assembleSegments', () => {
  it('joins contiguous pages from byte 0 into one text, multi-byte characters included', () => {
    const split = new Uint8Array([...FEI.subarray(0, 1)]);
    const rest = new Uint8Array([...FEI.subarray(1), ...PAGE_A]);
    const assembly = assembleSegments([
      { start: 0, endExclusive: 1, bytes: split, nextCursor: 'c1' },
      { start: 1, endExclusive: 1 + rest.length, bytes: rest, nextCursor: null },
    ]);
    expect(assembly.kind).toBe('assembled');
    if (assembly.kind !== 'assembled') return;
    expect(assembly.text.startsWith('费issue-filter-api first version')).toBe(true);
    expect(assembly.bytes.length).toBe(1 + rest.length);
    expect(assembly.more).toBe(false);
    expect(assembly.pageCount).toBe(2);
  });

  it('reports pages remaining while the last page names a cursor', () => {
    const assembly = assembleSegments([
      { start: 0, endExclusive: PAGE_A.length, bytes: PAGE_A, nextCursor: 'run09-page-2' },
    ]);
    expect(assembly).toMatchObject({ kind: 'assembled', more: true, pageCount: 1 });
    if (assembly.kind !== 'assembled') return;
    expect(assembly.text).toContain('issue-filter-api first version');
  });

  it('accepts a whole-object single page whose range matches its bytes (live sample shape)', () => {
    const assembly = assembleSegments([
      { start: 0, endExclusive: PAGE_A.length, bytes: PAGE_A, nextCursor: null },
    ]);
    expect(assembly).toMatchObject({ kind: 'assembled', more: false, pageCount: 1 });
  });

  it('refuses a first page that does not start at byte 0', () => {
    const assembly = assembleSegments([
      { start: 96, endExclusive: 96 + PAGE_A.length, bytes: PAGE_A, nextCursor: null },
    ]);
    expect(assembly).toEqual({ kind: 'inconsistent', reason: 'not-from-zero', pageCount: 1 });
  });

  it('refuses a gap between pages as an incomplete read', () => {
    const assembly = assembleSegments([
      { start: 0, endExclusive: PAGE_A.length, bytes: PAGE_A, nextCursor: 'a' },
      { start: PAGE_A.length + 1, endExclusive: PAGE_A.length + 1 + PAGE_B.length, bytes: PAGE_B, nextCursor: null },
    ]);
    expect(assembly).toEqual({ kind: 'inconsistent', reason: 'gap', pageCount: 2 });
  });

  it('refuses an overlap between pages (a repeated byte range)', () => {
    const assembly = assembleSegments([
      { start: 0, endExclusive: PAGE_A.length, bytes: PAGE_A, nextCursor: 'a' },
      { start: PAGE_A.length - 8, endExclusive: PAGE_A.length - 8 + PAGE_B.length, bytes: PAGE_B, nextCursor: null },
    ]);
    expect(assembly).toEqual({ kind: 'inconsistent', reason: 'overlap', pageCount: 2 });
  });

  it('refuses a regression to an earlier position', () => {
    const assembly = assembleSegments([
      { start: 0, endExclusive: PAGE_A.length, bytes: PAGE_A, nextCursor: 'a' },
      { start: 0, endExclusive: PAGE_A.length, bytes: PAGE_A, nextCursor: null },
    ]);
    expect(assembly).toEqual({ kind: 'inconsistent', reason: 'overlap', pageCount: 2 });
  });

  it('refuses a page whose declared range disagrees with the bytes it carries', () => {
    const assembly = assembleSegments([
      { start: 0, endExclusive: PAGE_A.length + 1, bytes: PAGE_A, nextCursor: null },
    ]);
    expect(assembly).toEqual({ kind: 'inconsistent', reason: 'range-bytes-mismatch', pageCount: 1 });
  });

  it('refuses a cursor that repeats (the read is looping)', () => {
    const assembly = assembleSegments([
      { start: 0, endExclusive: PAGE_A.length, bytes: PAGE_A, nextCursor: 'same' },
      { start: PAGE_A.length, endExclusive: PAGE_A.length + PAGE_B.length, bytes: PAGE_B, nextCursor: 'same' },
      { start: PAGE_A.length + PAGE_B.length, endExclusive: PAGE_A.length + 2 * PAGE_B.length, bytes: PAGE_B, nextCursor: null },
    ]);
    expect(assembly).toEqual({ kind: 'inconsistent', reason: 'cursor-loop', pageCount: 2 });
  });

  it('refuses an empty range that would pass as progress', () => {
    const assembly = assembleSegments([
      { start: 0, endExclusive: 0, bytes: new Uint8Array(0), nextCursor: 'a' },
    ]);
    expect(assembly).toEqual({ kind: 'inconsistent', reason: 'no-progress', pageCount: 1 });
  });
});

describe('judgeContentRead', () => {
  it('says partial while a cursor says pages remain, whatever a partial digest check might say', () => {
    expect(judgeContentRead({ verdict: 'mismatch', byteLength: 96, declaredByteLength: 2753, more: true })).toBe('partial');
    expect(judgeContentRead({ verdict: 'verified', byteLength: 96, declaredByteLength: 2753, more: true })).toBe('partial');
  });

  it('says changed only when the digests in hand disagree, and the old text goes', () => {
    expect(judgeContentRead({ verdict: 'mismatch', byteLength: 2753, declaredByteLength: 2753, more: false })).toBe('changed');
  });

  it('says unverified when the check could not run, never verified', () => {
    expect(judgeContentRead({ verdict: 'unverified', byteLength: 2753, declaredByteLength: 2753, more: false })).toBe('unverified');
    expect(judgeContentRead({ verdict: 'unverified', byteLength: null, declaredByteLength: null, more: false })).toBe('unverified');
  });

  it('says complete when the digest matches and the length is the declared one (live sample shape)', () => {
    expect(judgeContentRead({ verdict: 'verified', byteLength: 2753, declaredByteLength: 2753, more: false })).toBe('complete');
    expect(judgeContentRead({ verdict: 'verified', byteLength: 433, declaredByteLength: 433, more: false })).toBe('complete');
  });

  it('says partial for a verified read shorter than the declared length (truncated)', () => {
    expect(judgeContentRead({ verdict: 'verified', byteLength: 96, declaredByteLength: 2753, more: false })).toBe('partial');
    expect(judgeContentRead({ verdict: 'verified', byteLength: 433, declaredByteLength: 4330, more: false })).toBe('partial');
  });

  it('does not invent a verdict from a missing declared length', () => {
    expect(judgeContentRead({ verdict: 'verified', byteLength: 96, declaredByteLength: null, more: false })).toBe('complete');
    expect(judgeContentRead({ verdict: 'verified', byteLength: null, declaredByteLength: 2753, more: false })).toBe('complete');
  });
});
