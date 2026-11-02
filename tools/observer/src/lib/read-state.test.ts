import { describe, expect, it } from 'vitest';
import { readStateOf } from './read-state';

describe('readStateOf', () => {
  it('says loading until a read has been made and is not pending', () => {
    expect(readStateOf(undefined)).toBe('loading');
    expect(readStateOf({ isError: false, isPending: true })).toBe('loading');
  });

  it('says ready for a finished read', () => {
    expect(readStateOf({ isError: false, isPending: false })).toBe('ready');
  });

  it('says failed whenever the query reports an error, even with an earlier answer kept', () => {
    expect(readStateOf({ isError: true, isPending: false })).toBe('failed');
    expect(readStateOf({ isError: true, isPending: true })).toBe('failed');
  });
});
