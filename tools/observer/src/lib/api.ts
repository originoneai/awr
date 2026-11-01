/**
 * Thin fetch wrapper for the /api/team/* bridge routes. The bridge answers
 * failures with HTTP 200 and an { ok:false, error:{code} } envelope. The
 * envelope code is authoritative: local guard 403s are not session revocation;
 * only the bridge auth codes (or an envelope-free 401/403) clear restricted data.
 */

/** Envelope codes that revoke identity or permission (bridge vocabulary). */
export const AUTH_ERROR_CODES: ReadonlySet<string> = new Set([
  'Unauthenticated',
  'SessionExpired',
  'Forbidden',
]);

/** Mutations must carry this header; cross-site form posts cannot send it. */
export const GUARD_HEADER = 'x-awr-observer';

export class ApiError extends Error {
  code: string;
  httpStatus?: number;

  constructor(code: string, message: string, httpStatus?: number) {
    super(message);
    this.name = 'ApiError';
    this.code = code;
    this.httpStatus = httpStatus;
  }
}

export function isAuthError(error: unknown): boolean {
  return error instanceof ApiError && AUTH_ERROR_CODES.has(error.code);
}

async function request<T>(path: string, init?: RequestInit): Promise<T> {
  let res: Response;
  try {
    res = await fetch(path, { credentials: 'same-origin', ...init });
  } catch (cause) {
    throw new ApiError('NetworkError', String(cause instanceof Error ? cause.message : cause));
  }
  const payload = (await res.json().catch(() => null)) as
    | ({ ok: true } & T)
    | { ok: false; error?: { code?: string; message?: string } }
    | null;
  if (payload && typeof payload === 'object' && payload.ok === false) {
    const error = payload.error ?? {};
    throw new ApiError(
      error.code || 'UnknownError',
      error.message || error.code || 'request failed',
      res.status,
    );
  }
  if (res.status === 401) throw new ApiError('Unauthenticated', 'Authentication required', 401);
  if (res.status === 403) throw new ApiError('Forbidden', 'Access denied', 403);
  if (!res.ok) throw new ApiError('HttpError', `HTTP ${res.status}`, res.status);
  if (!payload) throw new ApiError('InvalidResponse', 'The bridge returned no JSON envelope', res.status);
  return payload as T;
}

export function apiGet<T>(path: string): Promise<T> {
  return request<T>(path);
}

export function apiPost<T>(path: string, body: unknown): Promise<T> {
  return request<T>(path, {
    method: 'POST',
    headers: { 'content-type': 'application/json', [GUARD_HEADER]: '1' },
    body: JSON.stringify(body ?? {}),
  });
}
