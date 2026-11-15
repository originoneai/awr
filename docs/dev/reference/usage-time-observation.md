# Usage and time observation handoff (WS-041 → WS-043)

WS-041 records **historical** usage and wall-clock observations. WS-043 may
consume them as covered history for forecasting, but must not treat any
cumulative-duration field as estimated remaining time.

## Surfaces

- Core: `awr_core::workstream_usage` — dedup, cost totals, counter deltas,
  allocations, coverage reports, corrections, `usage_observation_handoff`,
  `refuse_eta_from_cumulative_duration`.
- Store (schema 13 / `usage_time`): persisted receipts, corrections, allocation
  records, counter snapshots, execution intervals, idempotent ingest receipts.
- Runtime: attested ingest/query APIs and `usage_observation_for_ws043`.
- CLI: `awr usage record`, `list`, `summary` and `query` in source builds.
- MCP: `awr_usage_record` and `awr_usage_query`, discovered through `awr_evidence`.

## CLI and MCP access

These entry points use the project's existing personal usage ledger. Owner stdio
and unisolated project-level shared grants provide access. Shared workstream
grants and the Team PostgreSQL service retain their own authenticated contracts.

Start with the current project identity and revision.

```sh
awr --json usage summary
```

Prepare a JSON record using that response's `project_id` and `project_revision`.
The following receipt is synthetic. Replace its identifiers and measurements
with the importing host's actual observations.

```json
{
  "expected_revision": 12,
  "request_key": "billing-account/provider-call",
  "record": {
    "kind": "receipt",
    "data": {
      "receipt_id": "host-observation",
      "project_id": "PROJECT_ID",
      "provider_namespace": "billing-account",
      "provider": "example-provider",
      "call_id": "provider-call",
      "model": "example-model",
      "session_id": "coding-session",
      "occurred_at_ms": 1790000000000,
      "channel": "model",
      "attribution": {
        "work_id": "WORK_ID",
        "execution_id": "EXECUTION_ID",
        "workstream_id": null
      },
      "tokens": { "input": 1000, "output": 200, "cached_input": 100 },
      "cost": { "actual": { "currency": "USD", "micros": 1900 } }
    }
  }
}
```

```sh
awr --json usage record --input usage.json
awr --json usage list --limit 20
awr --json usage summary
```

`awr_usage_record` accepts the same JSON object. The shared HTTP adapter also
requires its normal stable `request_id`. The project comes from the configured
endpoint or registered project selector. A record's project identity must agree
with that binding. Record data, its intake receipt and the runtime event commit
in one transaction. The response retains `caller_supplied` provenance.

The `record.kind` values use the existing ledger types.

| Kind | Data |
| --- | --- |
| `receipt` | Provider/account call identity, model, occurrence time, historical work attribution, optional tokens and cost basis. |
| `correction` | Correction identity, project, target receipt, time, reason, actor, prior cost and new cost. |
| `allocation` | Allocation identity, project, receipt, time, explicit rule and conserved workstream shares. |
| `counter` | Project/account/provider/model/session/epoch scope, observation time and cumulative token counters. |
| `interval` | Execution identity and an observed `start_ms` / `end_ms` interval. |

Reuse the original request identity when a response is uncertain. Stable
account/provider/call identities deduplicate repeated observations, including
model and compaction reports of the same call. Conflicting measurements are
refused. Corrections retain the original record and their adjustment history.
Allocations retain the supplied rule and shares alongside occurrence-time
ownership.

For a missing MCP response, inspect `awr_operation_get` with its original
`request_id`. `awr_operation_recover` can resolve a committed usage event bound
to that request. It preserves uncertainty when the required event is absent.

### Detail queries and coverage

`awr_usage_query` takes a `kind` of `receipts`, `corrections`, `allocations`,
`counters`, `intervals`, `bindings`, `summary` or `observation`.
Detail pages accept a zero-based `offset`, a `limit` from 1 to 100, and return `next_offset`.
The CLI provides the corresponding page selection.

```sh
awr --json usage list --kind corrections --limit 20
awr --json usage list --kind allocations --limit 20
awr --json usage list --kind counters --limit 20
awr --json usage list --kind intervals --limit 20
awr --json usage list --kind bindings --limit 20
awr --json usage summary --uncorrected
```

Receipts and summaries apply corrections by default. A query with
`"apply_corrections": false` reads the original cost basis. Use
`awr usage query --input query.json` for the same query JSON as MCP.
Reads use a coherent SQLite snapshot and preserve registered source files.
Summaries cite up to 20 applied correction identities and provide a continuation
query for any remaining adjustment records.

Each currency and accounting basis has its own subtotal. Actual costs,
API-equivalent estimates and unknown calls remain distinct. An unknown cost is
represented as `"unknown"`. An estimate uses
`{"api_equivalent_estimate":{"amount":{"currency":"USD","micros":1900},"pricing_version":"price-v1"}}`.
Amounts use millionths of their stated currency.

Missing execution intervals leave observed time unknown. A recorded
zero-duration interval establishes an observed zero. Parallel intervals retain
both their wall-clock union and summed execution duration.

Summary coverage leaves an unreported expected set unknown. An observation query
may supply `expected_calls` and `expected_time_ms`. Observed counts and time come
from the stored ledger.

```json
{
  "kind": "observation",
  "expected_calls": 10,
  "expected_time_ms": 60000
}
```

## Invariants for WS-043

1. `UsageTimeObservationHandoff.is_historical_observation` is always true.
2. `is_not_estimated_remaining_time` is always true; refuse ETA labels on
   cumulative duration via `refuse_eta_from_cumulative_duration`.
3. Coverage (`call_coverage` / `time_coverage`) is optional only when the
   expected denominator is known; unknown expected never invents 100% coverage.
4. Actual, API-equivalent, and unknown costs remain separate from coverage.
5. Parallel `observed_wall_clock_ms` is not equal to `observed_execution_ms`
   when intervals overlap.

This note is a handoff contract, not a forecasting implementation.

See also `eta-checkpoints.md` for the WS-043 forecasting surface that
consumes this handoff.
