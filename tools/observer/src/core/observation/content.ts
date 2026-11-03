import type { DigestVerdict } from './types';

/** One page of a segmented content read: the service names a byte range and hands over its bytes. */
export interface SegmentPageInput {
  start: number;
  endExclusive: number;
  /** The page's payload, already decoded from the transport encoding by the caller. */
  bytes: Uint8Array;
  /** The cursor the page says to continue with, or null when it declares the end. */
  nextCursor: string | null;
}

/** How a sequence of pages failed to be the contiguous read from byte 0 the service prescribes. */
export type SegmentInconsistency =
  | 'not-from-zero'
  | 'gap'
  | 'overlap'
  | 'no-progress'
  | 'range-bytes-mismatch'
  | 'cursor-loop';

export type SegmentAssembly =
  | {
    kind: 'assembled';
    bytes: Uint8Array;
    /** The joined bytes as UTF-8 text; multi-byte characters may span page boundaries. */
    text: string;
    pageCount: number;
    /** True when the last page still names a cursor: what is held is the front part only. */
    more: boolean;
  }
  | {
    kind: 'inconsistent';
    reason: SegmentInconsistency;
    pageCount: number;
  };

/**
 * Assemble the pages of a segmented read the way the service prescribes: concatenate contiguous
 * ranges from byte 0, then the caller verifies byte_length and sha256. A first page that does not
 * start at zero, a gap, an overlap or a regression between pages, a range whose width disagrees
 * with the bytes it carries, or a cursor that repeats (the read is looping) refuses the whole
 * assembly — an inconsistent read is never shown as content.
 */
export function assembleSegments(pages: ReadonlyArray<SegmentPageInput>): SegmentAssembly {
  const chunks: Uint8Array[] = [];
  let total = 0;
  const seenCursors = new Set<string>();
  for (const [index, page] of pages.entries()) {
    const pageCount = index + 1;
    if (index === 0 && page.start !== 0) {
      return { kind: 'inconsistent', reason: 'not-from-zero', pageCount };
    }
    if (page.endExclusive <= page.start) {
      return { kind: 'inconsistent', reason: 'no-progress', pageCount };
    }
    if (index > 0) {
      const previous = pages[index - 1];
      if (page.start > previous.endExclusive) {
        return { kind: 'inconsistent', reason: 'gap', pageCount };
      }
      if (page.start < previous.endExclusive) {
        return { kind: 'inconsistent', reason: 'overlap', pageCount };
      }
    }
    if (page.bytes.length !== page.endExclusive - page.start) {
      return { kind: 'inconsistent', reason: 'range-bytes-mismatch', pageCount };
    }
    if (page.nextCursor != null) {
      if (seenCursors.has(page.nextCursor)) {
        return { kind: 'inconsistent', reason: 'cursor-loop', pageCount };
      }
      seenCursors.add(page.nextCursor);
    }
    chunks.push(page.bytes);
    total += page.bytes.length;
  }
  const bytes = new Uint8Array(total);
  let offset = 0;
  for (const chunk of chunks) {
    bytes.set(chunk, offset);
    offset += chunk.length;
  }
  return {
    kind: 'assembled',
    bytes,
    text: new TextDecoder().decode(bytes),
    pageCount: pages.length,
    more: pages.length > 0 && pages[pages.length - 1].nextCursor != null,
  };
}

/** What a content read amounts to for the person reading it. */
export type ContentReadStatus = 'complete' | 'partial' | 'changed' | 'unverified';

/**
 * Judge a content read against the reference that listed the document: a digest mismatch (proved
 * only by the two digests in hand, never by an error code) says the source changed and the old
 * text goes; while a cursor says pages remain, what is held is honest about being the front part
 * only, whatever a partial digest check might say; without a check the read stays unverified;
 * a verified read shorter than the declared length is a truncated part, not the whole.
 */
export function judgeContentRead(input: {
  verdict: DigestVerdict;
  /** Measured byte length of the text being held. */
  byteLength: number | null;
  /** The byte length the citing reference declared for this content. */
  declaredByteLength?: number | null;
  /** True when the read still names a cursor. */
  more: boolean;
}): ContentReadStatus {
  if (input.more) return 'partial';
  if (input.verdict === 'mismatch') return 'changed';
  if (input.verdict === 'unverified') return 'unverified';
  if (input.byteLength != null && input.declaredByteLength != null
    && input.byteLength < input.declaredByteLength) {
    return 'partial';
  }
  return 'complete';
}
