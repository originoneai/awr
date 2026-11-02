// Public synthetic extracts from real reply shapes; no credentials or private paths.
// service-review-inspect-api-approved-run09.json
export const APPROVED_REVIEW = { round_id: '01M6AVQ870N7MBRJ5MPCN79T55', state: 'approved' } as const;
// observer-activity-development.json
export const DEVELOPMENT_ACTIVITY = {
  action: 'access.changed', result: 'committed', created_at_unix_ms: 1793479144330,
  unknown_means: 'completion_not_recorded; inspect business request before retrying',
} as const;
// service-audit-requests-run09.json
export const UNKNOWN_REQUEST = {
  result: 'unknown', created_at_unix_ms: 1793526356827,
  unknown_means: 'completion_not_recorded; inspect business request before retrying',
} as const;
// observer-overview.json
export const OVERVIEW_REPLY = { status: 'unknown', owner_person: null, pageReadAtUnixMs: 1793475397385 } as const;
// lin-upstream-work-snapshot-api.json
export const SNAPSHOT_REPLY = { clientReportedAtUnixMs: 1793474474920, observedAtUnixMs: 1793475439978 } as const;
