# Outcome history

A read-only count of what AWR already recorded about the outcome of work: completions, confirmations, cancellations,
reopenings, supervised executions and usage. It adds no authority and changes no receipt. It counts; it does not
rate. The same question asked again gives the same report.

```rust
let report = awr_runtime::outcome_history(
    &store,
    project,
    None, // the project's main line; pass a branch id to read that branch
    &HistoryRequest { as_of_ms, cohort_by: CohortBy::ProviderModel, work_keys: None },
)?;
```

`awr_core::aggregate_outcome_history` is the same count over records an adapter has already read (no store, no
file, no clock, no model, no network), and `Store::outcome_history_input` reads them. There is no CLI or MCP command
yet; the first consumer is the evidence-based reassessment hint (AWR-DEC-042).

## The question

| Field | Meaning |
| --- | --- |
| `as_of_ms` | The instant of the question. Records after it are left out and counted; an execution that ends after it has not ended. Nothing reads the clock. |
| `cohort_by` | `none` (one cohort), `provider_model` or `agent`: the label of the session that wrote the work's latest completion, confirmation or cancellation (withdrawn or not), else of its latest execution. |
| `work_keys` | Only these works. A key no work has is named in `scope.unknown_work_keys`, not ignored. |

## What is counted, each on its own

| Count | Source | What it does not say |
| --- | --- | --- |
| `completion_reported` | A `work.completed` receipt in force whose bound acceptance is the work's current acceptance. The evidence it bound was supplied by the caller. | That the work is right. |
| `completion_by_evidence_level` | The highest level of the evidence that completion bound (`locally_verified` … `released`), or `not_recorded`. | That a test passed: the level is what the evidence claims. |
| `completion_under_earlier_standard` | A completion in force that bound other acceptance than the work has now. | An old pass is not reused under a new standard. |
| `completion_standard_unknown` | A completion that cannot be compared with the current acceptance. | |
| `executions.succeeded` / `failed` | The exit status of a supervised command recorded by the program. | That the command was a test: AWR does not know what a command is for. |
| `executions.unfinished` | Registered, starting or running at `as_of_ms`. | A failure. |
| `accepted_by_human` / `accepted_by_ai` / `accepted_by_delegate` | A `work.ordinary_confirmed` receipt in force, by the origin of its actor. | Engineering verification. |
| `cancelled` | A `work.cancelled` receipt in force. | |
| `conflicting` | Two completions in force at once (for example at two versions of the work): neither is chosen. | |
| `open` | No completion, confirmation or cancellation in force. | A failure: it is not yet known. |
| `reopened_works`, `withdrawn_completions`, `withdrawn_acceptances` | A reopening withdraws what a completion or a confirmation said. The original receipts stay counted as read. | |

Completion, human confirmation and the exit status of a command are three different observers. None implies another,
and nothing merges them into one score.

## Cost, duration, coverage

- Cost comes from the usage receipts the program holds, with their corrections applied, attributed to a work by
  `attribution.work_id` equal to the work's key or its id. Actual and API-equivalent cost are separate columns, each
  currency is its own column, and a call of unknown cost is counted in `unknown_cost_calls`, never added. A work with
  no usage receipt is `works_without_usage`: its cost was not observed, which is not zero.
- Duration is `observed_execution_ms` over the finished executions that have both a start and an end;
  `without_duration` counts the ones that do not. It is execution time, not elapsed time.
- `coverage` is `[observed, expected]` as integers: works with an outcome, with an execution, with usage, and finished
  executions with a duration. There are no percentages.
- `sample` says what happened to the records read, by family: how many were used, repeated, after the instant, naming
  no work of the project, or outside the filter; `receipts.without_version` counts receipts that do not say which
  version of the work they were written against.

## What is never derived

`not_derived` lists, in the words the assessment corpus pre-registered (cases C16, C17, P02, P05):
`model_success_rate`, `dollar_savings`, `semantic_understanding`. There is no rate, probability, score or float
anywhere in the report. A cohort exists only for sessions that wrote a receipt or ran an execution: no cohort is made
for a model that did not, and no counterfactual result is produced. A work is counted once, in one cohort; handoffs
mean that other sessions contributed to it.

## Isolation, determinism, limits

- One project per report. A record of another project (a receipt, an execution, a session label or a usage receipt)
  makes the whole question fail with `CrossProject`; the store reads only the project's own rows.
- The same receipt delivered twice is counted once; one receipt identity with two contents is refused.
- `history_hash` covers the whole report: the same records, request and instant give the same hash, whatever order the
  records arrive in.
- More than 200 000 records of one family are refused, not summarised from a prefix.

Fixtures: `tests/fixtures/assessment/history/` (each case lists the exact values it expects).
