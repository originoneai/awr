# Project period-update fact snapshots

`update.record` saves a deterministic task-fact snapshot with every new update
version. The caller supplies the judgment sentence and the service supplies the
facts. The counting table below is the Team store's side of the shared fact
contract; the words it shares with Observer's derived model (AWR-PPO-010) are
mapped in [Shared words with Observer](#shared-words-with-observer) and defined
on the Observer side in [overview-fact-semantics.md](overview-fact-semantics.md).
Only explicit source records contribute to the counts.

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

## Shared words with Observer

The mapping below is machine-checked on this side against the counting table,
and `overview-fact-semantics.md` machine-checks the same words on the Observer
side. Rows marked as open differences are recorded for the supervisor to
resolve; neither table silently adopts the other's rule. Observer's bridge does
not route `updates.list` or `update.inspect` yet (AWR-PPD-012 will add the
routes), so today the two tables agree at the word level without one feeding
the other.

| Shared word | Service snapshot fields | Relationship to the Observer rule |
| --- | --- | --- |
| 核验过 / Verified | `verified` | Open difference. The service counts `work.completed` receipts, which bind evidence digests to a settled execution under the project's completion policy. Observer's `verified` additionally requires every criterion supported, an explicitly independent reviewer, and integration. `overview-fact-semantics.md` keeps this as its open alignment point for this task. |
| 标了完成 / Reported complete | `claimed_unverified` | Same idea, different conditions. The service counts a reported `delivered` phase transition with no later-or-simultaneous verified completion. Observer's `claimed` requires a completion claim with no submitted result. |
| 等待 / Waiting | `waits_opened`, `waits_cleared` | Same four kinds: `decision`, `dependency`, `review`, `external` correspond to Observer's decisionWait, dependencyWait, reviewWait and externalWait. Evidence binding differs by design: the service counts explicit stored events and checkpoint transitions, while Observer's adapters bind the shared rules to their own current evidence. |
| 压着 / Holding up | `none` | Not a service fact. The snapshot stores wait facts only; the behind set (the waiting task plus every direct and transitive dependent, deduplicated, unknown when graph facts are incomplete) stays an Observer derivation and never becomes a partial service count. |

Count and coverage conventions match the shared `countDetails`, `coverage` and
`unknown` rows: every count is the length of the detail array beside it, a
narrowed read keeps exact counts over visible work and discloses only the
number of hidden tasks, and a schema-64 row without a snapshot reads back as
unknown (`fact_snapshot` and `fact_snapshot_coverage` are null), never as zero
counts.

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
counts. A row recorded under schema 64 has no snapshot; its read returns
`fact_snapshot` and `fact_snapshot_coverage` as null — unknown, not zero — and
it is never backfilled. Unknown or foreign update IDs remain `Forbidden`, and
task-only delegations receive `Forbidden` for update visibility.
