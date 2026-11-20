export const PROJECT_DETAIL_FILTERS = ['all', 'risk', 'no-update', 'takeover'] as const;
export type ProjectDetailFilter = (typeof PROJECT_DETAIL_FILTERS)[number];

export function normalizeProjectDetail(value: string | null): ProjectDetailFilter {
  return (PROJECT_DETAIL_FILTERS as readonly string[]).includes(value ?? '') ? value as ProjectDetailFilter : 'all';
}

export function projectsDetailPath(detail: ProjectDetailFilter): string {
  return detail === 'all' ? '/projects' : `/projects?detail=${encodeURIComponent(detail)}`;
}

/** Keep project identifiers in paths only, always through the same encoder. */
export function projectPath(project: string): string {
  return `/projects/${encodeURIComponent(project)}`;
}

/** AWR-PPO-059 owns this destination; the list only constructs its read-only link. */
export function takeoverResultPath(result: string): string {
  return `/takeover/${encodeURIComponent(result)}`;
}
