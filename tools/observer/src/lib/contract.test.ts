import { describe, expect, it } from 'vitest';
import {
  OBSERVATION_STALE_AFTER_MS,
  OVERVIEW_DEADLINE_MS,
  PLANE_STATES,
  READ_TIMEOUT_MS,
  compareAuthoritative,
  resolveCurrency,
} from './contract';

describe('observation-contract alias (single source: tools/inspector)', () => {
  it('exposes the four-state vocabulary and read bounds', () => {
    expect([...PLANE_STATES]).toEqual(['measured', 'stale', 'failed', 'not_measured']);
    expect(OBSERVATION_STALE_AFTER_MS).toBe(120000);
    expect(READ_TIMEOUT_MS).toBe(4000);
    expect(OVERVIEW_DEADLINE_MS).toBe(12000);
  });

  it('compareAuthoritative and resolveCurrency behave per contract', () => {
    const base = { project_revision: '9', source_snapshot_id: 's1', coordinator_epoch: 'e1' };
    expect(compareAuthoritative(base, { ...base, project_revision: '10' })).toBe('replace');
    expect(compareAuthoritative(base, { ...base, project_revision: '8' })).toBe('keep');
    expect(compareAuthoritative(base, { ...base, source_snapshot_id: 's2' })).toBe('incomparable');
    expect(resolveCurrency({ atUnixMs: 0, readAtMs: 1, staleAfterMs: 100 }).status).toBe('measured');
    expect(resolveCurrency({ atUnixMs: 0, readAtMs: 200, staleAfterMs: 100 }).status).toBe('stale');
    expect(resolveCurrency({ serviceStale: true }).status).toBe('stale');
  });
});
