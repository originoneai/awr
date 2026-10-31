import { act, screen } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { DEMO_OVERVIEW, DEMO_PROJECTS, renderApp, stubFetch } from './test-utils';

function overview(overrides: Record<string, unknown> = {}) {
  return {
    ...DEMO_OVERVIEW,
    ...overrides,
    coverage: { ...DEMO_OVERVIEW.coverage, ...(overrides.coverage as object | undefined) },
    freshness: { ...DEMO_OVERVIEW.freshness, ...(overrides.freshness as object | undefined) },
  };
}

function setup(data: unknown) {
  const fetchMock = stubFetch({ '/api/team/projects': DEMO_PROJECTS, '/api/team/overview': data });
  return { fetchMock, ...renderApp('/workbench?scope=demo') };
}

afterEach(() => {
  vi.useRealTimers();
});

describe('Workbench observation states', () => {
  it('shows a pending list presentation and real overview counts when works exist', async () => {
    setup(overview({
      works: [{ id: 'w1', title: 'A real work item' }, { id: 'w2', title: 'Another' }],
      coverage: { works_enumerated: 2, works_total: 2, succeeded: 1, failed: 0 },
    }));

    const pending = await screen.findByTestId('state-list-pending');
    expect(pending.textContent).toContain('后续切片');
    expect(pending.textContent).toContain('2/2');
    expect(pending.textContent).toContain('1');
    expect(screen.queryByTestId('state-empty')).toBeNull();
  });

  it('uses empty only for a succeeded scope with zero enumerated works and no failed stream', async () => {
    setup(overview({
      coverage: {
        scope: { ...DEMO_OVERVIEW.coverage.scope, status: 'succeeded' },
        works_enumerated: 0,
        works_total: 0,
        failed: 0,
        streams: [],
      },
      freshness: {
        ...DEMO_OVERVIEW.freshness,
        report_received: { status: 'measured', at_unix_ms: 1761840000000 },
      },
    }));

    expect(await screen.findByTestId('state-empty')).toBeTruthy();
    expect(screen.queryByTestId('state-list-pending')).toBeNull();
  });

  it('keeps the failed stream and its confirmation time visible', async () => {
    setup(overview({
      coverage: {
        failed: 1,
        streams: [{ workstream_id: 'stream-1', status: 'failed', error_code: 'ReadTimeout' }],
      },
    }));

    const partial = await screen.findByTestId('state-partial');
    expect(partial.textContent).toContain('ReadTimeout');
    expect(partial.textContent).toContain(String(new Date(1761840000000).getFullYear()));
    expect(screen.queryByTestId('state-empty')).toBeNull();
  });

  it('distinguishes not reported from an empty successful enumeration', async () => {
    setup(overview());
    expect(await screen.findByTestId('state-not-reported')).toBeTruthy();
    expect(screen.getByTestId('state-empty')).toBeTruthy();
  });

  it('localizes the succeeded coverage status', async () => {
    setup(overview());
    const summary = await screen.findByTestId('coverage-summary');
    expect(summary.textContent).toContain('读取成功');
    expect(summary.textContent).not.toContain('succeeded');
  });
});

describe('Workbench polling lifecycle', () => {
  it('polls the real overview route every five seconds', async () => {
    vi.useFakeTimers();
    const { fetchMock } = setup(DEMO_OVERVIEW);
    await act(async () => {
      await vi.advanceTimersByTimeAsync(0);
    });
    expect(screen.getByTestId('freshness-badge')).toBeTruthy();
    const overviewCalls = () => fetchMock.mock.calls.filter(([input]) => String(input).includes('/api/team/overview')).length;
    const initial = overviewCalls();

    await act(async () => {
      await vi.advanceTimersByTimeAsync(4_999);
    });
    expect(overviewCalls()).toBe(initial);
    await act(async () => {
      await vi.advanceTimersByTimeAsync(1);
    });
    expect(overviewCalls()).toBe(initial + 1);
  }, 15_000);
});
