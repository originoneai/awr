// Existing package path only: src/lib/contract.ts aliases tools/observer/server/observation-contract.js.
// The core never duplicates the four states or freshness thresholds.
export {
  PLANE_STATES,
  OBSERVATION_STALE_AFTER_MS,
  READ_TIMEOUT_MS,
  OVERVIEW_DEADLINE_MS,
} from '../../lib/contract';
export type { PlaneState } from '../../lib/contract';
