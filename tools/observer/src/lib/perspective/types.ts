/** The four prototype heights, ordered from the narrowest to the widest. */
export const PERSPECTIVE_IDS = ['me', 'lead', 'dept', 'top'] as const;

export type PerspectiveId = (typeof PERSPECTIVE_IDS)[number];

/** null means that an already-attempted read did not establish the fact. */
export type PerspectiveFact<T> = T | null;

export interface PerspectiveFacts {
  /** Groups the member supervises. Unknown never counts as a supervisory duty. */
  supervisedGroupCount: PerspectiveFact<number>;
  /** Groups with visible projects in which the member participates. */
  memberGroupProjectCount: PerspectiveFact<number>;
  /** Distinct groups and departments covered by the visible project set. */
  visibleProjectGroupCount: PerspectiveFact<number>;
  visibleProjectDepartmentCount: PerspectiveFact<number>;
  /** An explicit authorization fact, never inferred from a URL or a remembered choice. */
  canViewAllProjects: PerspectiveFact<boolean>;
  directory: {
    groupsRegistered: PerspectiveFact<boolean>;
    departmentsRegistered: PerspectiveFact<boolean>;
  };
  /** Open things waiting for this member at each height. Unknown is not a positive count. */
  pending: Record<PerspectiveId, PerspectiveFact<number>>;
}

export type PerspectiveLimitation =
  | 'groupsUnknown'
  | 'groupsNotRegistered'
  | 'departmentsNotRegistered'
  | 'directoryUnknown'
  | 'groupCoverageUnknown'
  | 'departmentCoverageUnknown'
  | 'allProjectsUnknown';

export interface PerspectiveDerivation {
  available: PerspectiveId[];
  limitations: PerspectiveLimitation[];
}

export function isPerspectiveId(value: string | null | undefined): value is PerspectiveId {
  return PERSPECTIVE_IDS.includes(value as PerspectiveId);
}

export function unknownPerspectiveFacts(myPending: number | null = null): PerspectiveFacts {
  return {
    supervisedGroupCount: null,
    memberGroupProjectCount: null,
    visibleProjectGroupCount: null,
    visibleProjectDepartmentCount: null,
    canViewAllProjects: null,
    directory: { groupsRegistered: null, departmentsRegistered: null },
    pending: { me: myPending, lead: null, dept: null, top: null },
  };
}
