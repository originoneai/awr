# Project period-update fact snapshots

`update.record` saves a deterministic task-fact snapshot with every new update
version. The caller supplies the judgment sentence and the service supplies the
facts. Only explicit source records contribute to the counts. The words shared
with Observer's derived model (AWR-PPO-010) are mapped in
[Shared words with Observer](#shared-words-with-observer).

## Counting table

| Snapshot field | Counted fact and task detail | Authoritative source | Deduplication and exclusions |
| --- | --- | --- | --- |
| `verified` | Distinct tasks that passed independently accepted completion in the period; `work_ids` identifies them. | `work.completed` events whose payload states `team_independent_acceptance=true`, or the older receipt whose `policy` names the agent or simulated-member independent review completion policy | One entry per task. `team_independent_acceptance=false` stays unverified whatever the policy says. A progress report saying `delivered` is excluded. |
| `waits_opened` | Waits that appeared in the period, broken down by `decision`, `review`, `dependency`, explicitly recorded `external`, and `unknown` kinds. Each kind's `work_ids` has one entry per wait. | Ordinary `decision.request` with `deduplicated=false`; legacy `wait.opened` with explicit `kind=decision` or `kind=external`, without a kind under `unknown`; `review.opened`; a checkpoint transition into `waiting_dependency` | Repeated identical progress reports preserve the existing wait. A task can contribute more than one wait and can occur under more than one kind. An untyped legacy wait stays out of the decision kind; `blocked`, free text, and incomplete dependency graphs remain outside `external`. |
| `waits_cleared` | Waits cleared in the period, with the same kind and task-detail shape as `waits_opened`. | Applied ordinary `decision.respond` / `decision.cancel`; legacy `wait.replied` carrying the same explicit kind as its opening; `review.decided`; a checkpoint transition out of `waiting_dependency` | A no-op response (`applied=false`) is excluded. A withdrawal clears a wait while remaining excluded from made decisions. Clearing accepts an opening event from any period. An untyped legacy reply clears an `unknown` wait only. |
| `review_returned` | Distinct tasks returned by review in the period; `work_ids` identifies them. | `review.decided` with `decision=reject` | One entry per task. Approval is excluded. This row stays independent of another wait opened for the returned task. |
| `completed_without_independent_review` | Distinct tasks completed in the period without independent acceptance; `work_ids` identifies them. | Every remaining `work.completed` event: `team_independent_acceptance=false`, an old-payload `policy` without an independent review name, or a payload with no basis at all | One entry per task. Recorded as completion only: an unknown basis stays out of `verified`. A task can appear here and in `verified` only through separate completions of different basis. |
| `delivered_unverified` | Distinct tasks whose progress newly transitioned to `delivered` in the period and still await completion of any basis at or after that delivery. | Checkpoint phase transitions plus the absence of a later-or-simultaneous `work.completed` event for the task | One entry per task. Repeated `delivered` checkpoints retain one delivery. A completion of any basis recorded after the delivery and before the update removes it; an older completion leaves the new delivery unverified. |
| `decisions_made` | Ordinary decisions actually answered in the period; `work_ids` has one entry per answer, so a task can occur more than once. | Applied `decision.respond` and legacy `wait.replied` carrying an explicit `kind=decision` | Review decisions, cancelled questions, untyped or external legacy replies, and no-op responses are excluded. |

Every non-wait metric is an object with `count` and `work_ids`. Wait metrics
have `count` and `kinds`; each kind has `kind`, `count`, and `work_ids`. Every
`count` is computed from the corresponding array: non-wait counts are array
lengths, and wait totals are the sums of their kind lengths. Arrays are sorted;
only the distinct-task rows deduplicate them. `fact_snapshot_json` contains only
task IDs, counts, and wait kinds; titles, sentences, questions, replies, report
bodies, actor/person IDs, display names, and per-person aggregates are excluded.

## Shared words with Observer

The mapping below is machine-checked on this side against the counting table,
and [overview-fact-semantics.md](overview-fact-semantics.md) machine-checks the
same words on the Observer side. Rows marked as open differences are recorded
for the supervisor to resolve; neither table silently adopts the other's rule.
Observer's bridge does not route `updates.list` or `update.inspect` yet
(AWR-PPD-012 will add the routes), so today the two tables agree at the word
level without one feeding the other.

| Shared word | Service snapshot fields | Relationship to the Observer rule |
| --- | --- | --- |
| 核验过 / Verified | `verified` | Both sides require independent acceptance: the service counts only completions whose receipt states team-independent acceptance, and Observer requires an explicitly independent reviewer. Observer additionally requires every criterion supported and integration; the service receipt vouches for its stored evidence binding, and completions without independent acceptance fall to `completed_without_independent_review`. |
| 等待 / Waiting | `waits_opened`, `waits_cleared` | Same four named kinds: `decision`, `dependency`, `review`, `external` correspond to Observer's decisionWait, dependencyWait, reviewWait and externalWait. The service adds `unknown` for stored wait events that carry no kind; an untyped legacy wait stays outside the named kinds. Evidence binding differs by design: the service counts explicit stored events and checkpoint transitions, while Observer's adapters bind the shared rules to their own current evidence. |
| 压着 / Holding up | `none` | Not a service fact. The snapshot stores wait facts only; the behind set (the waiting task plus every direct and transitive dependent, deduplicated, unknown when graph facts are incomplete) stays an Observer derivation instead of a partial service count. |

标了完成 / reported complete stays Observer's word alone: a completion is
declared and nothing was submitted. The service has no such fact; its
`delivered_unverified` row (已交付，还没核验) is broader because a `delivered`
progress phase can already carry a submitted result under review.

Count and coverage conventions match the shared `countDetails`, `coverage` and
`unknown` rows: every count is the length of the detail array beside it, a
narrowed read keeps exact counts over visible work and discloses only the
number of hidden tasks, and a schema-64 row without a snapshot reads back as
unknown (`fact_snapshot` and `fact_snapshot_coverage` are null) instead of
zero counts. The service's half-open `[start, end)` window is the same set of
instants as Observer's `vcount` rule with inclusive bounds one millisecond
apart: `[start, end)` equals `[start, end−1ms]` inclusive.

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
`fact_snapshot` and `fact_snapshot_coverage` as null — unknown rather than
zero — and it stays unbackfilled. Unknown or foreign update IDs remain `Forbidden`, and
task-only delegations receive `Forbidden` for update visibility.
