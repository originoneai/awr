import { act, renderHook } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { PerspectiveFacts } from './types';
import { PERSPECTIVE_STORAGE_KEY, usePerspectiveSelection } from './usePerspectiveSelection';

function narrowFacts(): PerspectiveFacts {
  return {
    supervisedGroupCount: 1,
    memberGroupProjectCount: 0,
    visibleProjectGroupCount: 1,
    visibleProjectDepartmentCount: 1,
    canViewAllProjects: false,
    directory: { groupsRegistered: true, departmentsRegistered: true },
    pending: { me: 0, lead: 0, dept: 0, top: 0 },
  };
}

afterEach(() => {
  window.localStorage.removeItem(PERSPECTIVE_STORAGE_KEY);
});

describe('usePerspectiveSelection programmatic authorization', () => {
  it('does not change the selection or write storage for an unavailable height', () => {
    const setItem = vi.spyOn(Storage.prototype, 'setItem');
    const { result } = renderHook(() => usePerspectiveSelection(narrowFacts(), null));
    const before = result.current.selected;
    let accepted: boolean | undefined;
    act(() => {
      accepted = result.current.select('top');
    });
    expect(accepted).toBe(false);
    expect(result.current.selected).toBe(before);
    expect(setItem).not.toHaveBeenCalled();
  });

  it('accepts and persists an available height as a control', () => {
    const setItem = vi.spyOn(Storage.prototype, 'setItem');
    const { result } = renderHook(() => usePerspectiveSelection(narrowFacts(), null));
    let accepted: boolean | undefined;
    act(() => {
      accepted = result.current.select('lead');
    });
    expect(accepted).toBe(true);
    expect(result.current.selected).toBe('lead');
    expect(setItem).toHaveBeenCalledWith(PERSPECTIVE_STORAGE_KEY, 'lead');
  });
});
