// Single source of truth: tools/inspector/observation-contract.js (AWR-OBS-002),
// aliased as 'observation-contract' in vite.config.ts. Never copy it here.
import * as contract from 'observation-contract';

export const PLANE_STATES = contract.PLANE_STATES;
export const OBSERVATION_STALE_AFTER_MS = contract.OBSERVATION_STALE_AFTER_MS;
export const READ_TIMEOUT_MS = contract.READ_TIMEOUT_MS;
export const OVERVIEW_DEADLINE_MS = contract.OVERVIEW_DEADLINE_MS;
export const compareAuthoritative = contract.compareAuthoritative;
export const resolveCurrency = contract.resolveCurrency;

export type PlaneState = (typeof PLANE_STATES)[number];
