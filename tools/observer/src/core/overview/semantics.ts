/**
 * Machine-checkable rule identifiers mirrored by docs/dev/reference/overview-fact-semantics.md.
 * These are documentation contract keys, not UI labels or service status values.
 */
export const OVERVIEW_RULES = {
  verified: ['all-criteria-supported', 'independent-review-approved', 'integrated'],
  claimed: ['report-complete', 'no-submitted-result'],
  returned: ['latest-current-review-rejected'],
  review: ['latest-current-review-open'],
  running: ['started', 'agent-active', 'activity-before-caller-threshold'],
  stalled: ['started', 'inactivity-at-least-caller-threshold'],
  waiting: ['open-decision-or-unsatisfied-dependency-or-external-party'],
  ready: ['acceptance-written', 'not-started', 'dependencies-satisfied'],
  planned: ['not-started', 'acceptance-or-upstream-incomplete'],
  decisionWait: ['open-current-decision'],
  dependencyWait: ['started-consumer', 'unsatisfied-dependency'],
  reviewWait: ['latest-current-review-open'],
  externalWait: ['recorded-external-wait'],
  behind: ['include-waiting-task', 'include-transitive-dependents', 'deduplicate', 'exclude-unknown'],
  countDetails: ['count-from-detail-array', 'unknown-count-is-null'],
  coverage: ['unread-is-not-empty', 'partial-count-is-unknown'],
  unknown: ['missing-is-unknown', 'unknown-is-not-zero'],
  chains: ['non-dependency-roots', 'group-review-by-explicit-person', 'union-impact', 'sort-impact-descending'],
  commitments: ['affected-task-commitments', 'unknown-commitments-are-not-empty'],
  approvals: ['approval-kinds-explicit', 'approval-recipient-explicit', 'approval-project-scoped', 'decisions-exclude-approvals'],
  people: ['explicit-person-identity', 'pending-nondependency-and-handoff', 'prototype-in-progress', 'owned-waits', 'owed-provider-results'],
  identity: ['explicit-equal-only', 'missing-is-neither-self-nor-other'],
  views: [
    'four-metrics-in-prototype-order',
    'caller-scopes-and-periods',
    'agents-running-or-waiting-active',
    'unknown-scope-is-not-empty',
  ],
  vcount: ['verified-only', 'verified-at-in-inclusive-window', 'caller-passes-non-overlapping-periods'],
} as const;
