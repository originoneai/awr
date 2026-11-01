import { createHash, webcrypto } from 'node:crypto';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { fetchArtifactContent, fetchArtifactSegments } from './lib/artifact';

const WORK = 'issue-filter-api';
const ARTIFACT = 'artifact-1';
const SELECTOR = { work_id: WORK, artifact_id: ARTIFACT, expected_sha256: '' };

function sha256(bytes: Buffer | string) {
  return createHash('sha256').update(bytes).digest('hex');
}

function response(data: unknown) {
  return new Response(JSON.stringify({ ok: true, result: { protocol_version: 1, data } }), {
    status: 200, headers: { 'content-type': 'application/json' },
  });
}

function selector(text: string) {
  return { ...SELECTOR, expected_sha256: sha256(text) };
}

function stubContent(data: Record<string, unknown>) {
  const fetchMock = vi.fn(async () => response(data));
  vi.stubGlobal('fetch', fetchMock);
  return fetchMock;
}

function segmentResponse(text: string, parts: string[], disclosure?: string) {
  const full = Buffer.from(text, 'utf8');
  const pages = parts.map((part, index) => {
    const start = Buffer.byteLength(parts.slice(0, index).join(''), 'utf8');
    const bytes = Buffer.from(part, 'utf8');
    return {
      artifact_id: ARTIFACT,
      work_id: WORK,
      byte_length: full.length,
      content_base64: bytes.toString('base64'),
      encoding: 'base64',
      range: { start, end_exclusive: start + bytes.length },
      segment_sha256: sha256(bytes),
      sha256: sha256(full),
      disclosure_sha256: disclosure ?? null,
      end_of_artifact: index === parts.length - 1,
      next_cursor: index === parts.length - 1 ? null : `cursor-${index + 1}`,
      next_query: null,
    };
  });
  const calls: Array<Record<string, unknown>> = [];
  const fetchMock = vi.fn(async (_input: RequestInfo | URL, init?: RequestInit) => {
    const query = JSON.parse(String(init?.body)).query;
    calls.push(query);
    const index = query.cursor ? Number(String(query.cursor).split('-')[1]) : 0;
    return response(pages[index]);
  });
  vi.stubGlobal('fetch', fetchMock);
  return { calls, pages };
}

afterEach(() => vi.unstubAllGlobals());

describe('artifact byte verification (AWR-OBS-006)', () => {
  it('verifies UTF-8 text bytes including multibyte content', async () => {
    vi.stubGlobal('crypto', webcrypto);
    const text = '# 客户支持 😀\n';
    const read = selector(text);
    stubContent({ work_id: WORK, artifact_id: ARTIFACT, byte_length: Buffer.byteLength(text), sha256: read.expected_sha256, text, content_base64: null });
    const result = await fetchArtifactContent('demo', read);
    expect(result.status).toBe('verified');
    expect(result.text).toBe(text);
  });

  it('flags a same-length text change as mismatch', async () => {
    vi.stubGlobal('crypto', webcrypto);
    const read = selector('abd');
    stubContent({ work_id: WORK, artifact_id: ARTIFACT, byte_length: 3, sha256: read.expected_sha256, text: 'abc', content_base64: null });
    const result = await fetchArtifactContent('demo', read);
    expect(result.status).toBe('mismatch');
    expect(result.text).toBeNull();
  });

  it('says unverified without Web Crypto, never verified', async () => {
    vi.stubGlobal('crypto', {});
    const text = 'abc';
    const read = selector(text);
    stubContent({ work_id: WORK, artifact_id: ARTIFACT, byte_length: 3, sha256: read.expected_sha256, text, content_base64: null });
    const result = await fetchArtifactContent('demo', read);
    expect(result.status).toBe('unverified');
    expect(result.text).toBe(text);
  });

  it('verifies decoded base64 bytes, not the base64 string', async () => {
    vi.stubGlobal('crypto', webcrypto);
    const text = 'abc😀';
    const bytes = Buffer.from(text, 'utf8');
    const read = selector(text);
    stubContent({ work_id: WORK, artifact_id: ARTIFACT, byte_length: bytes.length, sha256: read.expected_sha256, text: null, content_base64: bytes.toString('base64') });
    const result = await fetchArtifactContent('demo', read);
    expect(result.status).toBe('verified');
    expect(result.text).toBe(text);
  });

  it('verifies contiguous segments and accepts null original disclosure_sha256', async () => {
    vi.stubGlobal('crypto', webcrypto);
    const text = 'abcdefghij';
    const read = selector(text);
    const { calls } = segmentResponse(text, ['abcd', 'ef', 'ghij']);
    const result = await fetchArtifactSegments('demo', read);
    expect(result.status).toBe('verified');
    expect(result.text).toBe(text);
    expect(calls.map((call) => call.cursor)).toEqual([undefined, 'cursor-1', 'cursor-2']);
  });

  it('uses a non-empty disclosure digest for exported bytes', async () => {
    vi.stubGlobal('crypto', webcrypto);
    const text = 'exported';
    const disclosure = sha256(text);
    const read = selector(text);
    segmentResponse(text, ['expo', 'rted'], disclosure);
    const result = await fetchArtifactSegments('demo', read);
    expect(result.status).toBe('verified');
    expect(result.text).toBe(text);
  });

  it('rejects a range gap even when reconstructed length and digest happen to match', async () => {
    vi.stubGlobal('crypto', webcrypto);
    const text = 'abcdefgh';
    // Without the continuity guard, the buffer is [a, b, 0, d, e]. Make even
    // that incorrect reconstruction pass every downstream length/digest check.
    const expected = Buffer.from([97, 98, 0, 100, 101]);
    const read = { ...SELECTOR, expected_sha256: sha256(expected) };
    const ranges = [[0, 2], [3, 5]];
    const pages = ranges.map(([start, end], index) => ({
      artifact_id: ARTIFACT, byte_length: expected.length, content_base64: Buffer.from(text.slice(start, end)).toString('base64'),
      range: { start, end_exclusive: end }, segment_sha256: sha256(text.slice(start, end)), sha256: sha256(expected),
      end_of_artifact: index === 1, next_cursor: index === 0 ? 'cursor-1' : null,
    }));
    vi.stubGlobal('fetch', vi.fn(async (_input: RequestInfo | URL, init?: RequestInit) => {
      const query = JSON.parse(String(init?.body)).query;
      return response(pages[query.cursor ? 1 : 0]);
    }));
    await expect(fetchArtifactSegments('demo', read)).rejects.toThrow();
  });

  it('rejects overlapping ranges', async () => {
    vi.stubGlobal('crypto', webcrypto);
    const text = 'abcdefgh';
    const read = selector('abcc');
    const ranges = [[0, 3], [2, 4]];
    const pages = ranges.map(([start, end], index) => ({
      artifact_id: ARTIFACT, byte_length: 4, content_base64: Buffer.from(text.slice(start, end)).toString('base64'),
      range: { start, end_exclusive: end }, segment_sha256: sha256(text.slice(start, end)), sha256: sha256('abcc'),
      end_of_artifact: index === 1, next_cursor: index === 0 ? 'cursor-1' : null,
    }));
    vi.stubGlobal('fetch', vi.fn(async (_input: RequestInfo | URL, init?: RequestInit) => {
      const query = JSON.parse(String(init?.body)).query;
      return response(pages[query.cursor ? 1 : 0]);
    }));
    await expect(fetchArtifactSegments('demo', read)).rejects.toThrow();
  });

  it('rejects cursor loops and premature last pages', async () => {
    vi.stubGlobal('crypto', webcrypto);
    const read = selector('abcd');
    const page = {
      artifact_id: ARTIFACT, byte_length: 8, content_base64: Buffer.from('ab').toString('base64'),
      range: { start: 0, end_exclusive: 2 }, segment_sha256: sha256('ab'), sha256: sha256('abcd'),
      end_of_artifact: false, next_cursor: 'cursor-1',
    };
    let calls = 0;
    vi.stubGlobal('fetch', vi.fn(async () => {
      const start = calls * 2;
      calls += 1;
      // Ranges stay contiguous, so only the repeated-cursor guard can fail first.
      return response({ ...page, range: { start, end_exclusive: start + 2 } });
    }));
    await expect(fetchArtifactSegments('demo', read)).rejects.toThrow(/cursor|continuation|range/i);

    vi.stubGlobal('fetch', vi.fn(async () => response({ ...page, end_of_artifact: true, next_cursor: null })));
    await expect(fetchArtifactSegments('demo', read)).rejects.toThrow(/byte_length|end_of_artifact|length/i);
  });

  it('rejects wrong object echoes instead of displaying bytes', async () => {
    vi.stubGlobal('crypto', webcrypto);
    const text = 'abc';
    const read = selector(text);
    stubContent({ work_id: 'OTHER', artifact_id: ARTIFACT, byte_length: 3, sha256: read.expected_sha256, text, content_base64: null });
    await expect(fetchArtifactContent('demo', read)).rejects.toThrow();
  });
});
