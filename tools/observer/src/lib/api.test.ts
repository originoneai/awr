import { describe, expect, it, vi } from 'vitest';
import { ApiError, apiGet, apiPost, isAuthError } from './api';
import { stubFetch } from '../test-utils';

describe('api envelope handling', () => {
  it('returns the payload for ok envelopes', async () => {
    stubFetch({ '/api/team/projects': { ok: true, projects: [] } });
    await expect(apiGet<{ ok: boolean }>('/api/team/projects')).resolves.toMatchObject({ ok: true });
  });

  it('throws ApiError with the envelope code on failure', async () => {
    stubFetch({ '/api/team/overview': { ok: false, error: { code: 'ReadTimeout', message: 'slow' } } });
    const err = await apiGet('/api/team/overview?project=demo').then(
      () => null,
      (e: unknown) => e as ApiError,
    );
    expect(err).toBeInstanceOf(ApiError);
    expect(err?.code).toBe('ReadTimeout');
    expect(isAuthError(err)).toBe(false);
  });

  it.each(['Unauthenticated', 'SessionExpired', 'Forbidden'])(
    'marks %s envelope errors as auth errors',
    async (code) => {
      stubFetch({ '/api/team/overview': { ok: false, error: { code, message: 'denied' } } });
      const err = await apiGet('/api/team/overview?project=demo').then(
        () => null,
        (e: unknown) => e as ApiError,
      );
      expect(err).toBeInstanceOf(ApiError);
      expect(isAuthError(err)).toBe(true);
    },
  );

  it('treats HTTP 401/403 as auth errors even without an envelope', async () => {
    vi.stubGlobal('fetch', async () => new Response('nope', { status: 403 }));
    const err = await apiGet('/api/team/overview').then(
      () => null,
      (e: unknown) => e as ApiError,
    );
    expect(err).toBeInstanceOf(ApiError);
    expect(err?.code).toBe('Forbidden');
    expect(isAuthError(err)).toBe(true);
  });

  it('sends the mutation guard header on POST', async () => {
    const fetchMock = stubFetch({ '/api/team/logout': { ok: true, logged_out: true } });
    await apiPost('/api/team/logout', {});
    const call = fetchMock.mock.calls[0] as unknown as [string, RequestInit];
    const init = call[1];
    expect((init.headers as Record<string, string>)['x-awr-observer']).toBe('1');
  });
});
