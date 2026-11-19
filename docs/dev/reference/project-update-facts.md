# Project period-update fact snapshots

`update.record` saves a deterministic task-fact snapshot with every new update
version. The caller supplies the judgment sentence and the service supplies the
facts. This table is the shared counting contract for the Team store and
Observer's derived model. Only explicit source records contribute to the counts.

## Counting table

| Snapshot field | Counted fact and task detail | Authoritative source | Deduplication and exclusions |
| --- | --- | --- | --- |
| `verified` | Distinct tasks that passed verified completion in the period; `work_ids` identifies them. | `work.completed` events | One entry per task, even with several completion events in the period. A progress report saying `delivered` is excluded. |
| `waits_opened` | Waits that appeared in the period, broken down by `decision`, `review`, `dependency`, and explicitly recorded `external` kinds. Each kind's `work_ids` has one entry per wait. | Ordinary `decision.request` with `deduplicated=false`; legacy `wait.opened`; `review.opened`; a checkpoint transition into `waiting_dependency`; legacy wait events with explicit `kind=external` | Repeated identical progress reports preserve the existing wait. A task can contribute more than one wait and can occur under more than one kind. External waits require an explicit kind; `blocked`, free text, and incomplete dependency graphs remain outside that category. |
| `waits_cleared` | Waits cleared in the period, with the same kind and task-detail shape as `waits_opened`. | Applied ordinary `decision.respond` / `decision.cancel`; legacy `wait.replied`; `review.decided`; a checkpoint transition out of `waiting_dependency`; explicit external `wait.replied` | A no-op response (`applied=false`) is excluded. A withdrawal clears a wait while remaining excluded from made decisions. Clearing accepts an opening event from any period. |
| `review_returned` | Distinct tasks returned by review in the period; `work_ids` identifies them. | `review.decided` with `decision=reject` | One entry per task. Approval is excluded. This row stays independent of another wait opened for the returned task. |
| `claimed_unverified` | Distinct tasks whose progress newly transitioned to `delivered` in the period and still await verified completion at or after that claim. | Checkpoint phase transitions plus the absence of a later-or-simultaneous `work.completed` event for the task | One entry per task. Repeated `delivered` checkpoints retain one claim. A completion recorded after the claim and before the update removes it; an older completion leaves the new claim unverified. |
| `decisions_made` | Ordinary decisions actually answered in the period; `work_ids` has one entry per answer, so a task can occur more than once. | Applied `decision.respond` and legacy `wait.replied` for a decision wait | Review decisions, cancelled questions, external wait replies, and no-op responses are excluded. |

Every non-wait metric is an object with `count` and `work_ids`. Wait metrics
have `count` and `kinds`; each kind has `kind`, `count`, and `work_ids`. Every
`count` is computed from the corresponding array: non-wait counts are array
lengths, and wait totals are the sums of their kind lengths. Arrays are sorted;
only the distinct-task rows deduplicate them. `fact_snapshot_json` contains only
task IDs, counts, and wait kinds; titles, sentences, questions, replies, report
bodies, actor/person IDs, display names, and per-person aggregates are excluded.

## Period and timezone

The window is half-open:

```text
[ starts_on 00:00 in project time zone,
  (ends_on + 1 day) 00:00 in project time zone )
```

`projects.settings_json.time_zone` is an IANA/PostgreSQL time-zone name. Missing
`time_zone` uses `UTC`; an unknown non-empty name triggers stored-data divergence.
Start boundaries are inclusive and end boundaries exclusive. The resolved
epoch-millisecond bounds and time zone have separate audit columns beside the
task-only JSON. Events on opposite sides of either boundary therefore belong to
different periods, including across daylight-saving transitions.

## Immutability and authorized reads

The facts, resolved window, reference, update sentence, and version are inserted
atomically. The existing history trigger rejects every update or delete, including
a facts-only rewrite. An idempotent replay returns the original reference and
skips recalculation. Changed task data affect facts only when a caller records a
new version of the period; retained versions keep their original facts.

`updates.list` exposes only the opaque reference. `update.inspect` additionally
returns `data.fact_snapshot`, projected through the reader's current visible
workstreams/tasks, and `data.fact_snapshot_coverage` with `complete` and
`hidden_work_count`. A narrowed reader receives only authorized task IDs and
counts. Unknown or foreign update IDs remain `Forbidden`, and task-only
delegations receive `Forbidden` for update visibility.
