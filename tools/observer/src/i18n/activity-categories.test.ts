import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { describe, expect, it } from 'vitest';
import {
  ACTIVITY_CATEGORIES,
  ACTIVITY_CATEGORY_ROWS,
  activityCategoryOf,
} from './activity-categories';
import { OPERATION_VOCABULARY } from './human-language';

describe('activity category registry', () => {
  it('keeps the seven prototype categories in their exact order', () => {
    expect(ACTIVITY_CATEGORIES).toEqual(['delivery', 'review', 'decision', 'assignment', 'report', 'breakdown', 'update']);
  });

  it('contains every operation exactly once, so none can fall into two categories', () => {
    const actions = ACTIVITY_CATEGORY_ROWS.map((row) => row.action);
    expect(new Set(actions).size).toBe(actions.length);
    for (const action of Object.keys(OPERATION_VOCABULARY)) {
      expect(actions.filter((candidate) => candidate === action)).toHaveLength(1);
    }
  });

  it('uses the same category as the runtime filter for every registry row', () => {
    for (const row of ACTIVITY_CATEGORY_ROWS) {
      expect(activityCategoryOf(row.action), row.action).toBe(row.category);
    }
  });

  it('pins representative business operations and leaves non-business reads in All only', () => {
    expect(activityCategoryOf('delivery.submit_and_request_review')).toBe('delivery');
    expect(activityCategoryOf('review.decide')).toBe('review');
    expect(activityCategoryOf('decision.respond')).toBe('decision');
    expect(activityCategoryOf('task.assign')).toBe('assignment');
    expect(activityCategoryOf('execution.report')).toBe('report');
    expect(activityCategoryOf('planning.propose')).toBe('breakdown');
    expect(activityCategoryOf('update.recorded')).toBe('update');
    expect(activityCategoryOf('work.snapshot')).toBeNull();
    expect(activityCategoryOf('access.changed')).toBeNull();
    expect(activityCategoryOf('not.registered.anywhere')).toBeNull();
  });

  it('gives every registered operation a product-language label in both languages', () => {
    for (const row of ACTIVITY_CATEGORY_ROWS) {
      const word = OPERATION_VOCABULARY[row.action];
      expect(word, row.action).toBeDefined();
      expect(word.zh.trim().length, row.action).toBeGreaterThan(0);
      expect(word.en.trim().length, row.action).toBeGreaterThan(0);
      expect(word.zh).not.toBe(row.action);
      expect(word.en).not.toBe(row.action);
    }
  });

  it('keeps the developer reference in lockstep with the executable registry', () => {
    const reference = readFileSync(resolve('../../docs/dev/reference/activity-categories.md'), 'utf8');
    for (const row of ACTIVITY_CATEGORY_ROWS) {
      expect(reference.split(`\`${row.action}\``).length - 1, row.action).toBe(1);
    }
  });
});
