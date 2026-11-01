import { readFileSync } from 'node:fs';
import path from 'node:path';
import { describe, expect, it, vi } from 'vitest';
import { ApiError, apiGet, apiPost, AUTH_ERROR_CODES, isAuthError } from './api';

async function failure(response: Response): Promise<ApiError> {
  vi.stubGlobal('fetch', vi.fn(async () => response));
  return apiGet('/api/team/overview?project=demo').then(
    () => Promise.reject(new Error('expected request to fail')),
    (error: unknown) => error as ApiError,
  );
}

describe('API revocation and transport contract', () => {
  it('maps HTTP 401 to Unauthenticated', async () => {
    const error = await failure(new Response('no', { status: 401 }));
    expect(error.code).toBe('Unauthenticated');
    expect(isAuthError(error)).toBe(true);
  });

  it('maps an envelope-free HTTP 403 to Forbidden', async () => {
    const error = await failure(new Response('no', { status: 403 }));
    expect(error.code).toBe('Forbidden');
    expect(isAuthError(error)).toBe(true);
  });

  it.each(['ForbiddenHost', 'ForbiddenOrigin', 'ForbiddenSite', 'MissingGuardHeader'])(
    'does not revoke for server guard code %s',
    async (code) => {
      const error = await failure(new Response(JSON.stringify({
        ok: false,
        error: { code, message: 'request rejected' },
      }), { status: 403, headers: { 'content-type': 'application/json' } }));
      expect(error.code).toBe(code);
      expect(error.httpStatus).toBe(403);
      expect(isAuthError(error)).toBe(false);
    },
  );

  it('uses same-origin credentials for reads and guarded writes', async () => {
    const fetchMock = vi.fn(async (_input: RequestInfo | URL, _init?: RequestInit) => new Response(JSON.stringify({ ok: true }), {
      status: 200,
      headers: { 'content-type': 'application/json' },
    }));
    vi.stubGlobal('fetch', fetchMock);
    await apiGet('/api/team/projects');
    await apiPost('/api/team/logout', {});
    expect(fetchMock.mock.calls[0][1]?.credentials).toBe('same-origin');
    expect(fetchMock.mock.calls[1][1]?.credentials).toBe('same-origin');
    expect((fetchMock.mock.calls[1][1]?.headers as Record<string, string>)['x-awr-observer']).toBe('1');
  });

  it('keeps AUTH_ERROR_CODES in sync with the bridge READ_AUTH_FAILURES declaration', () => {
    const source = readFileSync(path.resolve(process.cwd(), '../inspector/team-bridge.js'), 'utf8');
    const declaration = source.match(/const READ_AUTH_FAILURES = new Set\(\[([^\]]+)\]\)/);
    expect(declaration).not.toBeNull();
    const bridgeCodes = [...declaration![1].matchAll(/'([^']+)'/g)].map((match) => match[1]).sort();
    expect([...AUTH_ERROR_CODES].sort()).toEqual(bridgeCodes);
  });
});
