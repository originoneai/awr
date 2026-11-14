import { describe, expect, it } from 'vitest';
import page from 'awr-progress-map/project-map/page.js';
import { teamProjectSnapshot, type GraphWork, type TeamGraphResponse } from './project-map';

function graph(): TeamGraphResponse {
  const work = (id: string, key: string, dependencies: string[]): GraphWork => ({
    work_id: id, external_key: key, workstream_id: 'development', title: key,
    state: 'planned', owner: null, last_event_at: null, dependencies_complete: true,
    required_dependencies: dependencies, claims: [],
  });
  return {
    ok: true, project: 'portal', read_at_unix_ms: 1_790_000_000_000,
    as_of: { project_revision: '12', source_snapshot_id: 'source-12', coordinator_epoch: 'current' },
    workstreams: [{ id: 'development', title: 'Development' }],
    items: [work('api', 'API-001', []), work('ui', 'UI-001', ['api'])],
  };
}

describe('Team projection into the shared project maps', () => {
  it('maps identities, dependencies and states without inventing missing data', () => {
    const response = graph();
    response.items[0].state = 'completed';
    const snapshot = teamProjectSnapshot(response);
    expect(snapshot.edges).toEqual([{ dependent: 'UI-001', prerequisite: 'API-001', required: true }]);
    expect(snapshot.nodes).toMatchObject([{ key: 'API-001', status: 'completed' }, { key: 'UI-001', status: 'planned' }]);
    expect(snapshot.doctor).toEqual({ ok: null, findings: [] });
    expect(snapshot.sessions).toEqual([]);
    expect(page.renderProjectMap(snapshot, undefined, {
      t: (key) => key, lang: 'en', interactive: true,
    }).body).toContain('<svg');
    expect(teamProjectSnapshot({ ...response, read_at_unix_ms: response.read_at_unix_ms + 1000 }).fingerprint).toBe(snapshot.fingerprint);
    expect(teamProjectSnapshot({ ...response, as_of: { ...response.as_of!, project_revision: '13' } }).fingerprint).not.toBe(snapshot.fingerprint);
  });

  it('refuses incomplete enumeration and conflicting display identities', () => {
    const response = graph();
    response.items[1].required_dependencies = ['unread'];
    expect(() => teamProjectSnapshot(response)).toThrow('incomplete');
    response.items[1].required_dependencies = [];
    response.items[1].external_key = response.items[0].external_key;
    expect(() => teamProjectSnapshot(response)).toThrow('identity');
  });

  it('preserves recorded claim times without inventing a session start', () => {
    const response = graph();
    response.items[0].claims = [{
      id: 'claim-api', actor_id: 'developer', actor_name: 'Developer', session_id: 'session-api',
      acquired_at_unix_ms: 1_789_999_000_000, expires_at_unix_ms: 1_790_001_000_000,
    }];
    const snapshot = teamProjectSnapshot(response);
    expect(snapshot.nodes).toMatchObject([{ claims: [{ acquired_at: 1_789_999_000_000 }] }, { claims: [] }]);
    expect(snapshot.sessions).toMatchObject([{ started_at: null, agent_id: 'Developer' }]);
  });

  it('maps unclaimed Team work to the backlog and preserves its original state', () => {
    const response = graph();
    response.items[0].state = 'unclaimed';
    expect((teamProjectSnapshot(response).nodes as object[])[0]).toMatchObject({ status: 'planned', source_status: 'unclaimed' });
    response.items[0].state = 'unsupported-future-state';
    expect((teamProjectSnapshot(response).nodes as object[])[0]).toMatchObject({ status: 'unknown', source_status: 'unsupported-future-state' });
  });

  it('keeps opaque source identifiers out of card subtitles while retaining their identity', () => {
    const response = graph();
    response.items[0].external_key = `WORK-${'b'.repeat(32)}`;
    expect(teamProjectSnapshot(response).nodes).toMatchObject([
      { key: 'UI-001' }, { key: response.items[0].external_key, display_key: '' },
    ]);
  });
});
