import {
  PERSPECTIVE_IDS,
  isPerspectiveId,
  type PerspectiveDerivation,
  type PerspectiveFacts,
  type PerspectiveId,
  type PerspectiveLimitation,
} from './types';

function countAtLeast(value: number | null, minimum: number): boolean {
  return typeof value === 'number' && Number.isSafeInteger(value) && value >= minimum;
}

/**
 * Implements docs/dev/reference/perspective-rules.md. Every rule is additive and every
 * unknown fact fails closed. In particular, an all-projects URL request is not an input here.
 */
export function derivePerspectives(facts: PerspectiveFacts): PerspectiveDerivation {
  const available: PerspectiveId[] = ['me'];
  const limitations: PerspectiveLimitation[] = [];
  const directoryComplete = facts.directory.groupsRegistered === true
    && facts.directory.departmentsRegistered === true;

  const hasGroupScope = countAtLeast(facts.supervisedGroupCount, 1)
    || countAtLeast(facts.memberGroupProjectCount, 1);
  if (hasGroupScope) available.push('lead');
  else if (facts.supervisedGroupCount === null || facts.memberGroupProjectCount === null) {
    limitations.push('groupsUnknown');
  }

  if (facts.directory.groupsRegistered === false) limitations.push('groupsNotRegistered');
  if (facts.directory.departmentsRegistered === false) limitations.push('departmentsNotRegistered');
  if (facts.directory.groupsRegistered === null || facts.directory.departmentsRegistered === null) {
    limitations.push('directoryUnknown');
  }

  const hasDepartmentScope = directoryComplete && countAtLeast(facts.visibleProjectGroupCount, 2);
  if (hasDepartmentScope) available.push('dept');
  else if (directoryComplete && facts.visibleProjectGroupCount === null) {
    limitations.push('groupCoverageUnknown');
  }

  const hasOrganizationScope = directoryComplete && (
    countAtLeast(facts.visibleProjectDepartmentCount, 2) || facts.canViewAllProjects === true
  );
  if (hasOrganizationScope) available.push('top');
  else if (directoryComplete) {
    if (facts.visibleProjectDepartmentCount === null) limitations.push('departmentCoverageUnknown');
    if (facts.canViewAllProjects === null) limitations.push('allProjectsUnknown');
  }

  return { available, limitations };
}

/** Narrowest height with a known positive pending count, otherwise the narrowest available. */
export function defaultPerspective(facts: PerspectiveFacts): PerspectiveId {
  const { available } = derivePerspectives(facts);
  return available.find((id) => countAtLeast(facts.pending[id], 1)) ?? available[0] ?? PERSPECTIVE_IDS[0];
}

/**
 * A present URL parameter is only a request. An invalid or unauthorized request falls straight
 * back to the default; without one, a remembered value is accepted only while still authorized.
 */
export function resolvePerspective(
  facts: PerspectiveFacts,
  requested: string | null,
  remembered: PerspectiveId | null,
): PerspectiveId {
  const { available } = derivePerspectives(facts);
  const fallback = defaultPerspective(facts);
  if (requested !== null) {
    return isPerspectiveId(requested) && available.includes(requested) ? requested : fallback;
  }
  return remembered !== null && available.includes(remembered) ? remembered : fallback;
}
