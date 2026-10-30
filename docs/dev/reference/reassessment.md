# Bounded reassessment

A work that deviates from its plan is not stopped by the program. For a source that opts in to
[plan admission](admission.md), the program classifies what the work's own records show, notices
the few conditions that deserve a second look and, while one holds, asks for a **current decision**
on that work and on no other. Nothing here creates a task, changes a plan or freezes unrelated
work; a decision is an event, not a task, and ordinary steps, waits and retries stay ordinary.

What is mechanical stays mechanical. The same command in the same directory is one method, and a
success of that method ends a streak of failures. Whether several failures share one root cause is
a judgment, so it is only ever **attributed** by a decider and never inferred from titles or
messages.

## What the program derives

Every fact keeps the record that carries it, so the actual evidence stays reachable. `work show`,
`work prepare` and `awr_work_get` list them under `reassessment.deviations` for a work in a source
that declares a policy, once there is something to show; other work gains no key.

| Kind | Source of the fact | Basis |
| --- | --- | --- |
| `failed_method` | an execution of the work that ended in failure (exit code, signal or error) | `recorded` |
| `unknown_outcome` | an execution that started and whose session is no longer active; or a current host observation with `outcome_known: false` | `recorded`, `host_reported` |
| `external_wait` | a wait for the user that nobody answered | `recorded` |
| `objective_blocker` | the blocker the work's source declares | `source_declared` |

A host observation (`awr work manage`) counts only while it is **current**: it must describe the
work's present contract and no execution may have been recorded after it. An older one is listed
under `stale_observations` and triggers nothing; a newer observation, such as `outcome_known: true`,
replaces it.

## Triggers

| Trigger | When it holds |
| --- | --- |
| `repeated_failed_method` | three failed attempts of one method in a row, with no success of that method in between |
| `unknown_outcome_unresolved` | an outcome nobody can state, so no one should retry blindly |

Two failures, failures interleaved with successes, failures of different methods, an external wait
and a declared blocker are classified but trigger nothing. A trigger that stops holding, because the
method succeeded, the execution was reported or the host learned the outcome, asks for nothing, and
the earlier decision stays on record.

## Decisions

`awr work reassess --input reassessment.json` (or the MCP tool `awr_work_reassess`) records one
`work.reassessment` event on the work. The request names the work, who decides (an active `session`,
or a declaring `agent`, as for admission), the revision the caller read, a stable request key, the
`facts_fingerprint` the decider reviewed (`reassessment.current.facts_fingerprint` of the work) and
the choice: `continue`, `wait`, `reuse`, `alter_method`, `split` or `replan`. The judgment states
why the choice answers the facts, what happens next and which evidence was actually read. An
optional `root_cause` groups records of this work under an attributed label.

The choice is a record, not an action. `split` and `replan` open no task and edit no plan: the split
is decided as [controlled decomposition](containment.md) is decided, and a plan is replaced where
plans are made. The same request key with the same content returns the original receipt; different
content under the key is refused. Facts that moved since the decider read them are a conflict, a
request while nothing calls for a decision is refused, and a root cause may only name records of the
work.

A decision is **current** while it binds exactly the facts that now trigger. A further failure or a
new unknown outcome makes it stale, and the diagnostic names each fact that appeared or went away.

## Diagnostics

| Diagnostic | Meaning |
| --- | --- |
| `reassessment_required` | a trigger holds and no decision answers these facts |
| `reassessment_stale` | the latest decision overlaps the facts but no longer matches them |
| `reassessment_unknown` | the records of the work could not be read; refresh the sources |

They join the admission diagnostics, so claims, `ready`, `work prepare`, execution registration and
start, handoff and resume are held by the same gate for the work they concern, and `work prepare`
answers `record_reassessment_before_claiming` when nothing else holds it. Finishing or reporting an
execution, checkpoints, ending a session and every read are never held, so an unknown outcome can
always be resolved. Under `legacy_observe` the state is shown and nothing is held; work
outside the policy's scope is shown the same way, never held, and no decision can be recorded for
it; a project without a policy is untouched.

## What the program does not decide

It checks that the facts are the work's own, that a decision exists, is attributed and describes
the facts as they are now. It does not judge whether continuing, waiting or changing the method is
right, and it never groups failures by what they say. Thresholds are written down here, open no
task and hold no other work.
