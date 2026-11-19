import { afterEach, describe, expect, it, vi } from 'vitest';
import { createAppQueryClient, overviewModelQueryOptions, POLL_INTERVAL_MS, revokeObservationReads } from './queries';
import type { TeamOverviewModelResponse } from './types';

function model(revision: string, readAt: number): TeamOverviewModelResponse {
  const empty = { coverage: 'complete', items: [] } as const;
  return {
    ok: true,
    project: 'portal',
    read_at_unix_ms: readAt,
    input: {
      tasks: { coverage: 'complete', items: [{ id: `task-${revision}`, title: `Revision ${revision}` }] },
      dependencies: empty,
      decisions: empty,
      reviewRounds: empty,
      handoffs: empty,
      members: empty,
      activity: empty,
      options: { nowUnixMs: readAt, stalledAfterDays: 3 },
    },
    coverage: Object.fromEntries(['tasks', 'dependencies', 'decisions', 'reviewRounds', 'handoffs', 'members', 'activity']
      .map((part) => [part, { status: 'complete', missing: [] }])) as unknown as TeamOverviewModelResponse['coverage'],
    freshness: {
      report_received: { status: 'not_measured' },
      authoritative: { status: 'measured', project_revision: revision,
        source_snapshot_id: 'synthetic-source', coordinator_epoch: 'synthetic-epoch' },
      page_read: { status: 'measured', at_unix_ms: readAt },
    },
  };
}

function response(value: TeamOverviewModelResponse) {
  return new Response(JSON.stringify(value), { status: 200, headers: { 'content-type': 'application/json' } });
}

afterEach(() => vi.unstubAllGlobals());

describe('overview-model query ordering and revocation', () => {
  it('keeps the newer authoritative model when the older browser reply lands last', async () => {
    const releases: Array<(value: Response) => void> = [];
    const fetchMock = vi.fn((_input: RequestInfo | URL) => new Promise<Response>((resolve) => releases.push(resolve)));
    vi.stubGlobal('fetch', fetchMock);
    const client = createAppQueryClient({ retry: false });
    const options = overviewModelQueryOptions('portal');
    const context = { signal: new AbortController().signal, client } as never;
    const older = options.queryFn!(context);
    const newer = options.queryFn!(context);
    expect(options.refetchInterval).toBe(POLL_INTERVAL_MS);
    expect(String(fetchMock.mock.calls[0]?.[0])).toBe('/api/team/overview-model?project=portal');

    releases[1]?.(response(model('42', 4200)));
    releases[0]?.(response(model('41', 4100)));
    const [oldResult, newResult] = await Promise.all([older, newer]);
    expect(newResult.freshness.authoritative.project_revision).toBe('42');
    expect(oldResult.freshness.authoritative.project_revision).toBe('42');
    expect(oldResult.read_at_unix_ms).toBe(4200);
    expect(oldResult.input.tasks.items?.[0]?.id).toBe('task-42');
  });

  it('drops an in-flight model and stops reads after observation revocation', async () => {
    let release!: (value: Response) => void;
    const fetchMock = vi.fn(() => new Promise<Response>((resolve) => { release = resolve; }));
    vi.stubGlobal('fetch', fetchMock);
    const client = createAppQueryClient({ retry: false });
    const options = overviewModelQueryOptions('portal');
    const context = { signal: new AbortController().signal, client } as never;
    const pending = options.queryFn!(context);
    revokeObservationReads(client);
    release(response(model('42', 4200)));
    await expect(pending).rejects.toMatchObject({ name: 'AbortError' });
    await expect(options.queryFn!(context)).rejects.toMatchObject({ name: 'AbortError' });
    expect(fetchMock).toHaveBeenCalledTimes(1);
    expect(overviewModelQueryOptions('portal', true).enabled).toBe(false);
  });
});
