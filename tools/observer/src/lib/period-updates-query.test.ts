import fs from 'node:fs';
import path from 'node:path';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { ApiError } from './api';
import {
  createAppQueryClient,
  isObservationReadRevoked,
  periodUpdateQueryOptions,
  periodUpdatesQueryOptions,
  POLL_INTERVAL_MS,
  queryKeys,
  resumeObservationReads,
  revokeObservationReads,
} from './queries';
import type { TeamPeriodUpdateResponse, TeamPeriodUpdatesResponse } from './types';

const FIXTURES_DIR = path.resolve(__dirname, '../../test/fixtures');
const UPDATES_LIST_SAMPLE = JSON.parse(fs.readFileSync(path.join(FIXTURES_DIR, 'updates-list.json'), 'utf8'));
const UPDATE_INSPECT_SAMPLE = JSON.parse(fs.readFileSync(path.join(FIXTURES_DIR, 'update-inspect.json'), 'utf8'));
const SAMPLE_PROJECT = UPDATES_LIST_SAMPLE.data.project_id as string;
const SAMPLE_UPDATE_ID = UPDATE_INSPECT_SAMPLE.data.update.update_id as string;

function bridgeUpdatesReply(
  revision = '6',
  overrides: {
    snapshot?: string;
    epoch?: string;
    atUnixMs?: number;
    mutate?: (payload: Record<string, unknown>) => void;
  } = {},
): TeamPeriodUpdatesResponse {
  const result = structuredClone(UPDATES_LIST_SAMPLE);
  result.project_revision = revision;
  result.data.as_of.project_revision = revision;
  if (overrides.snapshot) {
    result.source_snapshot_id = overrides.snapshot;
    result.data.as_of.source_snapshot_id = overrides.snapshot;
  }
  if (overrides.epoch) result.coordinator_epoch = overrides.epoch;
  if (overrides.atUnixMs !== undefined) result.data.as_of.at_unix_ms = overrides.atUnixMs;
  const payload: Record<string, unknown> = {
    ok: true,
    result,
    query: { protocol_version: 1, op: 'updates.list' },
    freshness: {
      report_received: { status: 'not_measured' },
      authoritative: {
        status: 'measured',
        stale_after_ms: 120000,
        age_ms: 1000,
        project_revision: result.project_revision,
        source_snapshot_id: result.source_snapshot_id,
        coordinator_epoch: result.coordinator_epoch,
        at_unix_ms: result.data.as_of.at_unix_ms,
      },
      page_read: { status: 'measured', at_unix_ms: result.data.as_of.at_unix_ms + 1000 },
    },
  };
  overrides.mutate?.(payload);
  return payload as unknown as TeamPeriodUpdatesResponse;
}

function bridgeUpdateReply(
  revision = '6',
  overrides: {
    snapshot?: string;
    epoch?: string;
    mutate?: (payload: Record<string, unknown>) => void;
  } = {},
): TeamPeriodUpdateResponse {
  const result = structuredClone(UPDATE_INSPECT_SAMPLE);
  result.project_revision = revision;
  result.data.as_of.project_revision = revision;
  if (overrides.snapshot) {
    result.source_snapshot_id = overrides.snapshot;
    result.data.as_of.source_snapshot_id = overrides.snapshot;
  }
  if (overrides.epoch) result.coordinator_epoch = overrides.epoch;
  const payload: Record<string, unknown> = {
    ok: true,
    result,
    query: { protocol_version: 1, op: 'update.inspect', update_id: SAMPLE_UPDATE_ID },
    freshness: {
      report_received: { status: 'not_measured' },
      authoritative: {
        status: 'measured',
        stale_after_ms: 120000,
        age_ms: 1000,
        project_revision: result.project_revision,
        source_snapshot_id: result.source_snapshot_id,
        coordinator_epoch: result.coordinator_epoch,
        at_unix_ms: result.data.as_of.at_unix_ms,
      },
      page_read: { status: 'measured', at_unix_ms: result.data.as_of.at_unix_ms + 1000 },
    },
  };
  overrides.mutate?.(payload);
  return payload as unknown as TeamPeriodUpdateResponse;
}

function jsonResponse(payload: unknown, status = 200) {
  return new Response(JSON.stringify(payload), { status, headers: { 'content-type': 'application/json' } });
}

afterEach(() => vi.unstubAllGlobals());

describe('period updates query options: ordering, contract validation, partial failure and revocation', () => {
  it('accepts verbatim PPD-010/011 replies and enforces numeric out-of-order and incomparable rules', async () => {
    const releases: Array<(value: Response) => void> = [];
    const fetchMock = vi.fn((_input: RequestInfo | URL) => new Promise<Response>((resolve) => releases.push(resolve)));
    vi.stubGlobal('fetch', fetchMock);
    const client = createAppQueryClient({ retry: false });

    const listOpts = periodUpdatesQueryOptions(SAMPLE_PROJECT, 'cursor-1');
    expect(listOpts.refetchInterval).toBe(POLL_INTERVAL_MS);
    const context = { signal: new AbortController().signal, client } as never;

    const older = listOpts.queryFn!(context);
    const newer = listOpts.queryFn!(context);
    expect(String(fetchMock.mock.calls[0]?.[0])).toBe(
      `/api/team/updates?project=${encodeURIComponent(SAMPLE_PROJECT)}&cursor=cursor-1`,
    );

    // Revision 10 lands first, then revision 9 lands late: numeric compare keeps 10 (not lexicographical "9" > "10")
    releases[1]?.(jsonResponse(bridgeUpdatesReply('10')));
    releases[0]?.(jsonResponse(bridgeUpdatesReply('9')));
    const [oldResult, newResult] = await Promise.all([older, newer]);
    expect(newResult.freshness.authoritative.project_revision).toBe('10');
    expect(oldResult.freshness.authoritative.project_revision).toBe('10');
    expect(oldResult.ordering).toBeUndefined();

    // A different source snapshot reports incomparable without silently overwriting revision 10
    const diffSnapshot = listOpts.queryFn!(context);
    releases[2]?.(jsonResponse(bridgeUpdatesReply('11', { snapshot: 'different-snapshot' })));
    const incomparable = await diffSnapshot;
    expect(incomparable.ordering).toBe('incomparable');
    expect(incomparable.freshness.authoritative.project_revision).toBe('10');

    // Same ordering guarantees apply to periodUpdateQueryOptions (/api/team/update?project=&id=)
    const detailOpts = periodUpdateQueryOptions(SAMPLE_PROJECT, SAMPLE_UPDATE_ID);
    expect(detailOpts.refetchInterval).toBe(POLL_INTERVAL_MS);
    const detailOld = detailOpts.queryFn!(context);
    const detailNew = detailOpts.queryFn!(context);
    expect(String(fetchMock.mock.calls[3]?.[0])).toBe(
      `/api/team/update?project=${encodeURIComponent(SAMPLE_PROJECT)}&id=${encodeURIComponent(SAMPLE_UPDATE_ID)}`,
    );
    releases[4]?.(jsonResponse(bridgeUpdateReply('10')));
    releases[3]?.(jsonResponse(bridgeUpdateReply('9')));
    const [dOld, dNew] = await Promise.all([detailOld, detailNew]);
    expect(dNew.freshness.authoritative.project_revision).toBe('10');
    expect(dOld.freshness.authoritative.project_revision).toBe('10');

    const diffEpoch = detailOpts.queryFn!(context);
    releases[5]?.(jsonResponse(bridgeUpdateReply('11', { epoch: 'epoch-b' })));
    const detailIncomparable = await diffEpoch;
    expect(detailIncomparable.ordering).toBe('incomparable');
    expect(detailIncomparable.freshness.authoritative.project_revision).toBe('10');
    // No cursor omits cursor query parameter cleanly
    const firstPageOpts = periodUpdatesQueryOptions(SAMPLE_PROJECT, null);
    const firstPagePromise = firstPageOpts.queryFn!(context);
    expect(String(fetchMock.mock.calls[6]?.[0])).toBe(
      `/api/team/updates?project=${encodeURIComponent(SAMPLE_PROJECT)}`,
    );
    releases[6]?.(jsonResponse(bridgeUpdatesReply('12')));
    const firstPage = await firstPagePromise;
    expect(firstPage.freshness.authoritative.project_revision).toBe('12');
  });

  it('rejects missing fields, extra fields and wrong types into partial failure without caching partial data or clearing sibling queries', async () => {
    const client = createAppQueryClient({ retry: false });
    client.setQueryData(queryKeys.projects, { ok: true, projects: [{ key: SAMPLE_PROJECT }] });
    client.setQueryData(queryKeys.overview(SAMPLE_PROJECT), { ok: true, project: SAMPLE_PROJECT });

    const listMutations: Array<[string, (payload: Record<string, unknown>) => void]> = [
      ['missing items', (p) => { delete ((p.result as Record<string, unknown>).data as Record<string, unknown>).items; }],
      ['extra data.total', (p) => { ((p.result as Record<string, unknown>).data as Record<string, unknown>).total = 1; }],
      ['extra top-level field', (p) => { p.extra = 'bad'; }],
      ['wrong status type', (p) => {
        (((p.result as Record<string, unknown>).data as Record<string, unknown>).items as Array<Record<string, unknown>>)[0]!.status = 'good';
      }],
      ['wrong project_id', (p) => { ((p.result as Record<string, unknown>).data as Record<string, unknown>).project_id = 'other'; }],
      ['execution_authorized true', (p) => { ((p.result as Record<string, unknown>).data as Record<string, unknown>).execution_authorized = true; }],
    ];
    for (const [label, mutate] of listMutations) {
      vi.stubGlobal('fetch', vi.fn(() => Promise.resolve(jsonResponse(bridgeUpdatesReply('6', { mutate })))));
      const opts = periodUpdatesQueryOptions(SAMPLE_PROJECT, null);
      await expect(client.fetchQuery({ ...opts, retry: false })).rejects.toMatchObject({
        name: 'ApiError',
        code: 'InvalidResponse',
      });
      expect(client.getQueryData(opts.queryKey), label).toBeUndefined();
      expect(client.getQueryData(queryKeys.projects), `sibling projects preserved (${label})`).toBeDefined();
      expect(client.getQueryData(queryKeys.overview(SAMPLE_PROJECT)), `sibling overview preserved (${label})`).toBeDefined();
      expect(isObservationReadRevoked(client), `not revoked on contract error (${label})`).toBe(false);
    }

    const detailMutations: Array<[string, (payload: Record<string, unknown>) => void]> = [
      ['missing fact_snapshot_coverage', (p) => {
        delete ((p.result as Record<string, unknown>).data as Record<string, unknown>).fact_snapshot_coverage;
      }],
      ['extra fact_snapshot field', (p) => {
        (((p.result as Record<string, unknown>).data as Record<string, unknown>).fact_snapshot as Record<string, unknown>).extra = { count: 0, work_ids: [] };
      }],
      ['wrong verified.count', (p) => {
        ((((p.result as Record<string, unknown>).data as Record<string, unknown>).fact_snapshot as Record<string, unknown>).verified as Record<string, unknown>).count = 99;
      }],
      ['mismatched update_id', (p) => {
        (((p.result as Record<string, unknown>).data as Record<string, unknown>).update as Record<string, unknown>).update_id = 'other';
      }],
      ['null ref with non-null snapshot', (p) => {
        (((p.result as Record<string, unknown>).data as Record<string, unknown>).update as Record<string, unknown>).fact_snapshot_ref = null;
      }],
    ];
    for (const [label, mutate] of detailMutations) {
      vi.stubGlobal('fetch', vi.fn(() => Promise.resolve(jsonResponse(bridgeUpdateReply('6', { mutate })))));
      const opts = periodUpdateQueryOptions(SAMPLE_PROJECT, SAMPLE_UPDATE_ID);
      await expect(client.fetchQuery({ ...opts, retry: false })).rejects.toMatchObject({
        name: 'ApiError',
        code: 'InvalidResponse',
      });
      expect(client.getQueryData(opts.queryKey), label).toBeUndefined();
      expect(client.getQueryData(queryKeys.projects), `sibling preserved (${label})`).toBeDefined();
      expect(isObservationReadRevoked(client), `not revoked (${label})`).toBe(false);
    }

    // Legacy schema-64 update (all three snapshot fields null) is accepted
    vi.stubGlobal('fetch', vi.fn(() => Promise.resolve(jsonResponse(bridgeUpdateReply('6', {
      mutate: (p) => {
        const data = (p.result as Record<string, unknown>).data as Record<string, unknown>;
        (data.update as Record<string, unknown>).fact_snapshot_ref = null;
        data.fact_snapshot = null;
        data.fact_snapshot_coverage = null;
      },
    })))));
    const legacy = await client.fetchQuery({ ...periodUpdateQueryOptions(SAMPLE_PROJECT, SAMPLE_UPDATE_ID), retry: false });
    expect(legacy.result.data.fact_snapshot).toBeNull();
    expect(legacy.result.data.fact_snapshot_coverage).toBeNull();
  });

  it('clears cached updates immediately and stops polling on revocation or auth failure', async () => {
    for (const code of ['Unauthenticated', 'SessionExpired', 'Forbidden'] as const) {
      const client = createAppQueryClient({ retry: false });
      const listOpts = periodUpdatesQueryOptions(SAMPLE_PROJECT, null);
      const detailOpts = periodUpdateQueryOptions(SAMPLE_PROJECT, SAMPLE_UPDATE_ID);
      client.setQueryData(listOpts.queryKey, bridgeUpdatesReply('6'));
      client.setQueryData(detailOpts.queryKey, bridgeUpdateReply('6'));

      vi.stubGlobal('fetch', vi.fn(() => Promise.resolve(jsonResponse({ ok: false, error: { code, message: 'revoked' } }))));
      await expect(client.fetchQuery({ ...listOpts, retry: false })).rejects.toBeInstanceOf(ApiError);
      expect(isObservationReadRevoked(client), code).toBe(true);
      expect(client.getQueryCache().getAll(), code).toHaveLength(0);
      expect(client.getQueryData(listOpts.queryKey), code).toBeUndefined();
      expect(client.getQueryData(detailOpts.queryKey), code).toBeUndefined();
    }

    // In-flight requests (both list and detail) are aborted and cannot repopulate after revokeObservationReads
    const releases: Array<(value: Response) => void> = [];
    const fetchMock = vi.fn(() => new Promise<Response>((resolve) => releases.push(resolve)));
    vi.stubGlobal('fetch', fetchMock);
    const client = createAppQueryClient({ retry: false });
    const listOpts = periodUpdatesQueryOptions(SAMPLE_PROJECT, null);
    const detailOpts = periodUpdateQueryOptions(SAMPLE_PROJECT, SAMPLE_UPDATE_ID);
    const context = { signal: new AbortController().signal, client } as never;

    // Seed revision 10 first, then revoke & resume: held ordering state must have been cleared on revocation
    const seedList = listOpts.queryFn!(context);
    const seedDetail = detailOpts.queryFn!(context);
    releases[0]?.(jsonResponse(bridgeUpdatesReply('10')));
    releases[1]?.(jsonResponse(bridgeUpdateReply('10')));
    expect((await seedList).freshness.authoritative.project_revision).toBe('10');
    expect((await seedDetail).freshness.authoritative.project_revision).toBe('10');

    const pendingList = listOpts.queryFn!(context);
    const pendingDetail = detailOpts.queryFn!(context);
    revokeObservationReads(client);
    releases[2]?.(jsonResponse(bridgeUpdatesReply('11')));
    releases[3]?.(jsonResponse(bridgeUpdateReply('11')));
    await expect(pendingList).rejects.toMatchObject({ name: 'AbortError' });
    await expect(pendingDetail).rejects.toMatchObject({ name: 'AbortError' });
    await expect(listOpts.queryFn!(context)).rejects.toMatchObject({ name: 'AbortError' });
    await expect(detailOpts.queryFn!(context)).rejects.toMatchObject({ name: 'AbortError' });
    expect(fetchMock).toHaveBeenCalledTimes(4);

    // After resuming on a new session at revision 6, the revoked revision 10 must NOT leak back from held ordering state
    resumeObservationReads(client);
    const resumedList = listOpts.queryFn!(context);
    const resumedDetail = detailOpts.queryFn!(context);
    releases[4]?.(jsonResponse(bridgeUpdatesReply('6')));
    releases[5]?.(jsonResponse(bridgeUpdateReply('6')));
    expect((await resumedList).freshness.authoritative.project_revision).toBe('6');
    expect((await resumedDetail).freshness.authoritative.project_revision).toBe('6');

    expect(periodUpdatesQueryOptions(SAMPLE_PROJECT, null, true).enabled).toBe(false);
    expect(periodUpdateQueryOptions(SAMPLE_PROJECT, SAMPLE_UPDATE_ID, true).enabled).toBe(false);
    expect(periodUpdatesQueryOptions(null, null, false).enabled).toBe(false);
    expect(periodUpdateQueryOptions(SAMPLE_PROJECT, null, false).enabled).toBe(false);
    expect(periodUpdateQueryOptions(null, SAMPLE_UPDATE_ID, false).enabled).toBe(false);
  });
});
