import { afterEach, expect, it, vi } from 'vitest';
import { createAppQueryClient, resumeObservationReads, revokeObservationReads, sourceContentQueryOptions } from './lib/queries';

afterEach(() => vi.unstubAllGlobals());

it('fences source reads prepared before revocation until the observation session resumes', async () => {
  const fetchMock = vi.fn(async () => new Response(JSON.stringify({ ok: true, result: { data: {} } }), { status: 200 }));
  vi.stubGlobal('fetch', fetchMock);
  const client = createAppQueryClient({ retry: false });
  // A previously scheduled read retains its enabled options while the UI processes revocation.
  const prepared = sourceContentQueryOptions('demo', 'docs/guide.md', 'a'.repeat(64), false);
  try {
    revokeObservationReads(client);
    await expect(client.fetchQuery(prepared)).rejects.toMatchObject({ name: 'AbortError' });
    expect(fetchMock).not.toHaveBeenCalled();

    resumeObservationReads(client);
    await expect(client.fetchQuery(prepared)).resolves.toMatchObject({ result: { data: {} } });
    expect(fetchMock).toHaveBeenCalledTimes(1);
  } finally {
    client.clear();
  }
});

it('does not dispatch an already cancelled source read to the transport', async () => {
  const fetchMock = vi.fn();
  vi.stubGlobal('fetch', fetchMock);
  const client = createAppQueryClient({ retry: false });
  const abort = new AbortController();
  abort.abort();
  const prepared = sourceContentQueryOptions('demo', 'docs/guide.md', 'a'.repeat(64));
  await expect(Promise.resolve().then(() => prepared.queryFn({ signal: abort.signal, client }))).rejects.toMatchObject({ name: 'AbortError' });
  expect(fetchMock).not.toHaveBeenCalled();
  client.clear();
});
