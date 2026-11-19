// @vitest-environment node
import { readFileSync } from 'node:fs';
import path from 'node:path';
import { describe, expect, it } from 'vitest';
import { defaultPerspective, derivePerspectives, resolvePerspective } from './rules';
import type { PerspectiveFacts, PerspectiveId } from './types';

function fullFacts(): PerspectiveFacts {
  return {
    supervisedGroupCount: 1,
    memberGroupProjectCount: 1,
    visibleProjectGroupCount: 3,
    visibleProjectDepartmentCount: 2,
    canViewAllProjects: true,
    directory: { groupsRegistered: true, departmentsRegistered: true },
    pending: { me: 0, lead: 0, dept: 0, top: 0 },
  };
}

function personalFacts(): PerspectiveFacts {
  return {
    ...fullFacts(),
    supervisedGroupCount: 0,
    memberGroupProjectCount: 0,
    visibleProjectGroupCount: 1,
    visibleProjectDepartmentCount: 1,
    canViewAllProjects: false,
  };
}

describe('perspective authorization rules', () => {
  it('gives a member with only personal tasks My work', () => {
    expect(derivePerspectives(personalFacts())).toEqual({ available: ['me'], limitations: [] });
  });

  it('adds Group delivery for a supervisory duty even without membership facts', () => {
    const facts = { ...personalFacts(), supervisedGroupCount: 1, memberGroupProjectCount: null };
    expect(derivePerspectives(facts).available).toEqual(['me', 'lead']);
  });

  it('adds Group delivery for a visible project in the member group', () => {
    const facts = { ...personalFacts(), supervisedGroupCount: null, memberGroupProjectCount: 1 };
    expect(derivePerspectives(facts).available).toEqual(['me', 'lead']);
  });

  it('adds Department projects only when visible projects cross groups', () => {
    const oneGroup = { ...personalFacts(), supervisedGroupCount: 1 };
    expect(derivePerspectives(oneGroup).available).toEqual(['me', 'lead']);
    const twoGroups = { ...oneGroup, visibleProjectGroupCount: 2 };
    expect(derivePerspectives(twoGroups).available).toEqual(['me', 'lead', 'dept']);
  });

  it('adds Goals and delivery for multiple departments without requiring all-projects access', () => {
    const facts = { ...fullFacts(), canViewAllProjects: false };
    expect(derivePerspectives(facts).available).toEqual(['me', 'lead', 'dept', 'top']);
  });

  it('adds Goals and delivery for explicit all-projects access even within one department', () => {
    const facts = {
      ...personalFacts(),
      supervisedGroupCount: 1,
      visibleProjectGroupCount: 1,
      canViewAllProjects: true,
    };
    expect(derivePerspectives(facts).available).toEqual(['me', 'lead', 'top']);
  });

  it.each(['groupsRegistered', 'departmentsRegistered'] as const)(
    'degrades to at most the first two heights when %s is false',
    (key) => {
      const facts = { ...fullFacts(), directory: { ...fullFacts().directory, [key]: false } };
      const result = derivePerspectives(facts);
      expect(result.available).toEqual(['me', 'lead']);
      expect(result.limitations).toContain(key === 'groupsRegistered' ? 'groupsNotRegistered' : 'departmentsNotRegistered');
    },
  );

  it('keeps wider heights closed and explains unknown directory registration', () => {
    const facts = { ...fullFacts(), directory: { groupsRegistered: true, departmentsRegistered: null } };
    expect(derivePerspectives(facts)).toEqual({ available: ['me', 'lead'], limitations: ['directoryUnknown'] });
  });

  it('does not turn unknown group relationships into Group delivery', () => {
    const facts = { ...personalFacts(), supervisedGroupCount: null, memberGroupProjectCount: null };
    expect(derivePerspectives(facts)).toEqual({ available: ['me'], limitations: ['groupsUnknown'] });
  });

  it('does not turn unknown project group coverage into Department projects', () => {
    const facts = { ...fullFacts(), visibleProjectGroupCount: null, visibleProjectDepartmentCount: 1, canViewAllProjects: false };
    const result = derivePerspectives(facts);
    expect(result.available).toEqual(['me', 'lead']);
    expect(result.limitations).toEqual(['groupCoverageUnknown']);
  });

  it('does not turn unknown department coverage or all-projects access into Goals and delivery', () => {
    const unknownDepartments = { ...fullFacts(), visibleProjectDepartmentCount: null, canViewAllProjects: false };
    expect(derivePerspectives(unknownDepartments).available).not.toContain('top');
    expect(derivePerspectives(unknownDepartments).limitations).toContain('departmentCoverageUnknown');
    const unknownAllProjects = { ...fullFacts(), visibleProjectDepartmentCount: 1, canViewAllProjects: null };
    expect(derivePerspectives(unknownAllProjects).available).not.toContain('top');
    expect(derivePerspectives(unknownAllProjects).limitations).toContain('allProjectsUnknown');
  });

  it('treats invalid counts as unknown rather than widening access or pending work', () => {
    const facts = {
      ...personalFacts(),
      supervisedGroupCount: Number.POSITIVE_INFINITY,
      visibleProjectGroupCount: Number.NaN,
      visibleProjectDepartmentCount: -2,
      pending: { me: Number.NaN, lead: 1, dept: 1, top: 1 },
    };
    expect(derivePerspectives(facts).available).toEqual(['me']);
    expect(defaultPerspective(facts)).toBe('me');
  });
});

describe('default, remembered and requested selections', () => {
  it('chooses the narrowest height with known pending work', () => {
    const facts = { ...fullFacts(), pending: { me: 0, lead: 0, dept: 4, top: 8 } };
    expect(defaultPerspective(facts)).toBe('dept');
    expect(defaultPerspective({ ...facts, pending: { ...facts.pending, me: 1 } })).toBe('me');
    expect(defaultPerspective({ ...facts, pending: { ...facts.pending, me: 0, dept: 0 } })).toBe('top');
  });

  it('uses the narrowest height when no pending count is positive, including unknown counts', () => {
    expect(defaultPerspective(fullFacts())).toBe('me');
    const facts = { ...fullFacts(), pending: { me: null, lead: null, dept: null, top: null } };
    expect(defaultPerspective(facts)).toBe('me');
  });

  it('honors an authorized request and otherwise falls back instead of escalating', () => {
    expect(resolvePerspective(fullFacts(), 'lead', null)).toBe('lead');
    expect(resolvePerspective(personalFacts(), 'top', null)).toBe('me');
    expect(resolvePerspective(personalFacts(), 'administrator', null)).toBe('me');
  });

  it('accepts a remembered value only while authorized', () => {
    expect(resolvePerspective(fullFacts(), null, 'lead')).toBe('lead');
    expect(resolvePerspective(personalFacts(), null, 'top')).toBe('me');
  });

  it('an unauthorized request cannot reuse an authorized remembered height', () => {
    const facts = { ...personalFacts(), supervisedGroupCount: 1 };
    expect(resolvePerspective(facts, 'top', 'lead')).toBe('me');
  });
});

describe('the rule table in docs/dev/reference/perspective-rules.md', () => {
  const docPath = path.resolve(process.cwd(), '../../docs/dev/reference/perspective-rules.md');
  const text = readFileSync(docPath, 'utf8');
  const rows = text.split('\n')
    .filter((line) => /^\| [1-4] \|/.test(line))
    .map((line) => line.split('|').map((cell) => cell.trim().replace(/^`|`$/g, '')).slice(1, -1));

  it('has exactly four rows whose codes match the implementation order', () => {
    expect(rows).toHaveLength(4);
    expect(rows.map((row) => row[0])).toEqual(['1', '2', '3', '4']);
    expect(rows.map((row) => row[3])).toEqual(['me', 'lead', 'dept', 'top']);
  });

  it('keeps every documented condition aligned with a positive implementation example', () => {
    const examples: Record<PerspectiveId, PerspectiveFacts> = {
      me: personalFacts(),
      lead: { ...personalFacts(), supervisedGroupCount: 1 },
      dept: { ...fullFacts(), visibleProjectDepartmentCount: 1, canViewAllProjects: false },
      top: fullFacts(),
    };
    for (const row of rows) {
      const code = row[3] as PerspectiveId;
      expect(derivePerspectives(examples[code]).available, row.join(' | ')).toContain(code);
    }
    expect(rows[0][2]).toContain('Always present');
    expect(rows[1][2]).toContain('supervises at least one group');
    expect(rows[1][2]).toContain('participates in a group');
    expect(rows[2][2]).toContain('at least two groups');
    expect(rows[3][2]).toContain('at least two departments');
    expect(rows[3][2]).toContain('view all projects');
  });

  it('pins the missing-directory fallback and its exact Chinese explanation', () => {
    expect(text).toContain('尚未登记部门');
    const facts = { ...fullFacts(), directory: { groupsRegistered: true, departmentsRegistered: false } };
    expect(derivePerspectives(facts).available).toEqual(['me', 'lead']);
    expect(derivePerspectives(facts).limitations).toContain('departmentsNotRegistered');
  });
});
