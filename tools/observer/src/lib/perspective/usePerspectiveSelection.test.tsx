import { act, render, renderHook, screen } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { PerspectiveProvider, usePerspective } from './PerspectiveProvider';
import { unknownPerspectiveFacts, type PerspectiveFacts } from './types';
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

function Probe() {
  const { selected } = usePerspective();
  return <div data-testid="selected-perspective">{selected}</div>;
}

function deptFacts(): PerspectiveFacts {
  return {
    supervisedGroupCount: 1,
    memberGroupProjectCount: 1,
    visibleProjectGroupCount: 3,
    visibleProjectDepartmentCount: 1,
    canViewAllProjects: false,
    directory: { groupsRegistered: true, departmentsRegistered: true },
    pending: { me: null, lead: null, dept: null, top: null },
  };
}

describe('PerspectiveProvider deferred URL cleanup while shell facts load', () => {
  it('stays fail-closed while loading without erasing a requested perspective, then opens it once authorized', () => {
    const onChange = vi.fn();
    const { rerender } = render(
      <PerspectiveProvider facts={unknownPerspectiveFacts()} loading requestedPerspective="dept" onChange={onChange}>
        <Probe />
      </PerspectiveProvider>,
    );
    expect(screen.getByTestId('selected-perspective').textContent).toBe('me');
    expect(onChange).not.toHaveBeenCalled();

    rerender(
      <PerspectiveProvider facts={deptFacts()} loading={false} requestedPerspective="dept" onChange={onChange}>
        <Probe />
      </PerspectiveProvider>,
    );
    expect(screen.getByTestId('selected-perspective').textContent).toBe('dept');
    expect(onChange).not.toHaveBeenCalled();
  });

  it('cleans an unauthorized requested perspective once loading finishes', () => {
    const onChange = vi.fn();
    const { rerender } = render(
      <PerspectiveProvider facts={unknownPerspectiveFacts()} loading requestedPerspective="dept" onChange={onChange}>
        <Probe />
      </PerspectiveProvider>,
    );
    expect(onChange).not.toHaveBeenCalled();

    rerender(
      <PerspectiveProvider facts={narrowFacts()} loading={false} requestedPerspective="dept" onChange={onChange}>
        <Probe />
      </PerspectiveProvider>,
    );
    expect(screen.getByTestId('selected-perspective').textContent).toBe('me');
    expect(onChange).toHaveBeenCalledWith('me');
  });
});
