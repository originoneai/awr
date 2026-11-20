import { OVERVIEW_PARTS, type OverviewInput } from '../core/overview';
import { myWorkInput } from './workbench';

/** A valid composition response for legacy shell tests whose old /overview stub used to match /overview-model. */
export function demoOverviewModelResponse(input: OverviewInput = myWorkInput(), extra: Record<string, unknown> = {}) {
  const value = structuredClone(input);
  value.tasks = {
    ...value.tasks,
    items: (value.tasks.items ?? []).map((task) => ({ ...task, projectId: 'demo' })),
  };
  value.approvals = {
    ...value.approvals,
    items: (value.approvals.items ?? []).map((approval) => ({ ...approval, projectId: 'demo' })),
  };
  value.options = {
    ...value.options,
    views: {
      ...value.options.views,
      me: { personId: value.options.views?.me?.personId ?? null, projectId: 'demo' },
      lead: { personId: value.options.views?.lead?.personId ?? null, projectId: 'demo' },
    },
  };
  return {
    ok: true,
    project: 'demo',
    read_at_unix_ms: value.options.nowUnixMs,
    input: value,
    coverage: Object.fromEntries(OVERVIEW_PARTS.map((part) => [part, {
      status: value[part].coverage,
      ...(value[part].coverage === 'complete' ? {} : { reason: value[part].reason }),
      missing: [],
    }])),
    freshness: {
      report_received: { status: 'not_measured' },
      authoritative: { status: 'measured', at_unix_ms: value.options.nowUnixMs },
      page_read: { status: 'measured', at_unix_ms: value.options.nowUnixMs },
    },
    ...extra,
  };
}

export function emptyOverviewModelResponse() {
  return demoOverviewModelResponse(myWorkInput({
    tasks: { coverage: 'complete', items: [] },
    dependencies: { coverage: 'complete', items: [] },
    decisions: { coverage: 'complete', items: [] },
    reviewRounds: { coverage: 'complete', items: [] },
    handoffs: { coverage: 'complete', items: [] },
    approvals: { coverage: 'complete', items: [] },
    activity: { coverage: 'complete', items: [] },
  }));
}
