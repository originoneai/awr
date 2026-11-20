import { describe, expect, it } from 'vitest';
import {
  normalizeProjectDetail,
  projectPath,
  projectsDetailPath,
  takeoverResultPath,
} from './project-path';

describe('project list paths', () => {
  it('uses one encoder for project and takeover destinations', () => {
    expect(projectPath('project /?&=')).toBe('/projects/project%20%2F%3F%26%3D');
    expect(takeoverResultPath('result /?&=')).toBe('/takeover/result%20%2F%3F%26%3D');
  });

  it('clamps every unknown detail key to the full list', () => {
    for (const value of [null, '', 'bogus', '99', '-1', 'abc', 'ALL']) {
      expect(normalizeProjectDetail(value)).toBe('all');
      expect(projectsDetailPath(normalizeProjectDetail(value))).toBe('/projects');
    }
    for (const value of ['all', 'risk', 'no-update', 'takeover'] as const) {
      expect(normalizeProjectDetail(value)).toBe(value);
    }
    expect(projectsDetailPath('risk')).toBe('/projects?detail=risk');
    expect(projectsDetailPath('no-update')).toBe('/projects?detail=no-update');
    expect(projectsDetailPath('takeover')).toBe('/projects?detail=takeover');
  });
});
