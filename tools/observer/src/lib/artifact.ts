import { ApiError, apiPost } from './api';
import { asText } from './json';
import type { TeamReadResponse } from './types';

export type ArtifactVerification = 'verified' | 'mismatch' | 'unverified';

export interface ArtifactSelector {
  work_id: string;
  artifact_id: string;
  expected_sha256: string;
}

export interface ArtifactReadResult {
  status: ArtifactVerification;
  text: string | null;
}

interface ArtifactContentData {
  work_id?: string;
  artifact_id?: string;
  byte_length?: number;
  sha256?: string;
  disclosure_sha256?: string | null;
  segment_sha256?: string;
  text?: string | null;
  content_base64?: string | null;
  range?: { start?: number; end_exclusive?: number };
  end_of_artifact?: boolean;
  next_cursor?: string | null;
  next_query?: { cursor?: string } | null;
}

function decodeBase64(value: string): Uint8Array {
  const binary = atob(value);
  return Uint8Array.from(binary, (character) => character.charCodeAt(0));
}

function bytesOf(data: ArtifactContentData): Uint8Array | null {
  if (typeof data.text === 'string') return new TextEncoder().encode(data.text);
  if (typeof data.content_base64 === 'string') return decodeBase64(data.content_base64);
  return null;
}

async function sha256(bytes: Uint8Array): Promise<string | null> {
  if (typeof crypto === 'undefined' || !crypto.subtle) return null;
  const buffer = await crypto.subtle.digest('SHA-256', new Uint8Array(bytes));
  return Array.from(new Uint8Array(buffer), (byte) => byte.toString(16).padStart(2, '0')).join('');
}

async function verifyBytes(
  bytes: Uint8Array,
  expectedSha256: string,
  declaredLength: unknown,
  declaredSha256: unknown,
): Promise<ArtifactVerification> {
  if (declaredSha256 !== expectedSha256) return 'mismatch';
  if (declaredLength != null && Number(declaredLength) !== bytes.length) return 'mismatch';
  const actual = await sha256(bytes);
  if (actual === null) return 'unverified';
  return actual === expectedSha256 ? 'verified' : 'mismatch';
}

function result(bytes: Uint8Array | null, status: ArtifactVerification): ArtifactReadResult {
  return { status, text: bytes && status !== 'mismatch' ? new TextDecoder().decode(bytes) : null };
}

export async function fetchArtifactContent(
  project: string,
  selector: ArtifactSelector,
  signal?: AbortSignal,
): Promise<ArtifactReadResult> {
  const response = await apiPost<TeamReadResponse<ArtifactContentData>>('/api/team/read', {
    project,
    query: {
      protocol_version: 1,
      op: 'artifact.content',
      work_id: selector.work_id,
      artifact_id: selector.artifact_id,
      expected_sha256: selector.expected_sha256,
      max_context_bytes: 1048576,
    },
  }, signal);
  const data = response.result.data;
  if (data.work_id !== selector.work_id || data.artifact_id !== selector.artifact_id) {
    throw new ApiError('BadGateway', 'Artifact response is not for the requested object');
  }
  const bytes = bytesOf(data);
  if (!bytes) return { status: 'unverified', text: null };
  const status = await verifyBytes(bytes, selector.expected_sha256, data.byte_length, data.sha256);
  return result(bytes, status);
}

export async function fetchArtifactSegments(
  project: string,
  selector: ArtifactSelector,
  signal?: AbortSignal,
): Promise<ArtifactReadResult> {
  const chunks: Uint8Array[] = [];
  const seenCursors = new Set<string>();
  let cursor: string | undefined;
  let expectedStart = 0;
  let declaredLength: number | undefined;
  let fullDigest: string | undefined;

  for (let page = 0; page < 1024; page += 1) {
    const response = await apiPost<TeamReadResponse<ArtifactContentData>>('/api/team/read', {
      project,
      query: {
        protocol_version: 1,
        op: 'artifact.segment',
        work_id: selector.work_id,
        artifact_id: selector.artifact_id,
        expected_sha256: selector.expected_sha256,
        max_context_bytes: 4096,
        ...(cursor ? { cursor } : {}),
      },
    }, signal);
    const data = response.result.data;
    if (data.artifact_id !== selector.artifact_id) {
      throw new ApiError('BadGateway', 'Artifact segment is not for the requested artifact');
    }
    const start = data.range?.start;
    const end = data.range?.end_exclusive;
    if (start !== expectedStart || end == null || end <= start) {
      throw new ApiError('BadGateway', 'Artifact segment range is not contiguous');
    }
    const bytes = typeof data.content_base64 === 'string' ? decodeBase64(data.content_base64) : null;
    if (!bytes || bytes.length !== end - start) {
      throw new ApiError('BadGateway', 'Artifact segment length does not match its range');
    }
    if (data.segment_sha256) {
      const actual = await sha256(bytes);
      if (actual !== null && actual !== data.segment_sha256) {
        throw new ApiError('BadGateway', 'Artifact segment digest mismatch');
      }
    }
    chunks.push(bytes);
    expectedStart = end;
    declaredLength = data.byte_length;
    fullDigest = asText(data.disclosure_sha256) ?? asText(data.sha256) ?? selector.expected_sha256;

    if (data.end_of_artifact) {
      const total = new Uint8Array(expectedStart);
      let offset = 0;
      for (const chunk of chunks) {
        total.set(chunk, offset);
        offset += chunk.length;
      }
      if (declaredLength == null || declaredLength !== total.length) {
        throw new ApiError('BadGateway', 'Artifact byte_length does not match the reconstructed bytes');
      }
      const status = await verifyBytes(total, selector.expected_sha256, declaredLength, fullDigest);
      return result(total, status);
    }

    const next = asText(data.next_cursor) ?? asText(data.next_query?.cursor);
    if (!next) throw new ApiError('BadGateway', 'Artifact continuation is missing before end_of_artifact');
    if (seenCursors.has(next)) throw new ApiError('BadGateway', 'Artifact segment cursor loop detected');
    seenCursors.add(next);
    cursor = next;
  }
  throw new ApiError('ResponseTooLarge', 'Artifact has too many segments');
}
