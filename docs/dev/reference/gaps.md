# Gaps

A gap is something missing or changed in the **intent**, the **acceptance**, the **design**, the
**plan** or the **work definition** of a work. For a source that opts in to
[plan admission](admission.md), a gap is recorded against the layer it affects and says how it can
be closed. It puts in question only the judgments that read that layer, on that work; it keeps
every failure on record, opens no task and holds no unrelated work. Repairing a plan is not an
answer to a question that needs a run.

## Route and resolution

`awr work gap --input gap.json` (or the MCP tool `awr_work_gap`) records one `work.gap` event.
`action` is `open`, `resolve` or `withdraw`; a `gap_key` names the gap within the work and branch
for good, so a closed gap is never reopened under the same key.

Opening needs a `route`, a `resolution` and a `subject` (what is missing or changed), plus the
anchor of its resolution and no other:

| Resolution | Opening names | Closing needs |
| --- | --- | --- |
| `fact_lookup` | nothing more | the `sources` that were consulted |
| `feasibility_experiment` | the `experiment`: the operation key the run will be registered under | the run itself, see below |
| `authorized_choice` | optional `authority`: the agents who may choose | an agent among them, else among the policy's `supervisors` |
| `environment_wait` | what it is `waiting_for` | what was `observed` that ended the wait |

Every close states an `outcome`. A choice nobody is authorized to make cannot even be opened: it
needs named `authority` or `supervisors` in the policy (`gap_authority_unknown`).

## What an open gap puts in question

| Route | Puts in question, for this work only |
| --- | --- |
| `intent` | its admission and the containment of its children |
| `acceptance`, `work_definition` | its admission |
| `plan` | the containment of its children |
| `design` | nothing: AWR keeps no judgment on design, so the gap is only recorded |

The judgment is reported as `admission_suspended` or `containment_suspended` in the work's
readiness (and in that of its children), naming the gaps. Claims, `ready`, `work prepare`
(`resolve_open_gaps_before_claiming`), execution registration and start, handoff and resume are
held by the same gate as for [admission](admission.md), for the work concerned. Nothing else is
touched: another work, a sibling, a parent's own admission or a decision about other facts keeps
going, and no task is created. Closing the last gap lifts the suspension. Nothing is decided again
unless the work itself changed, in which case the decision is stale by its own binding.

Reading, checkpointing, ending a session, reporting an execution and the experiment of an open gap
are never held.

## An experiment is closed by the run

A `feasibility_experiment` gap is closed only when an execution of **this work** under the declared
operation key was registered **after the gap was opened** and has **ended**. A failed run is a run:
it counts, and it stays on record. Editing the plan, the acceptance or the definition around the
question closes nothing; the gap stays open and the suspension stays in place until the run
exists (`gap_experiment_not_run`). A run of another work, a run that predates the gap and a run
that has not ended do not count.

The experiment is how the gap gets closed, so an open gap never holds back the execution registered
under its operation key; every other new execution of the work is held until the gap is closed.

## History

Events are append-only. `work show`, `work prepare` and `awr_work_get` list every gap of the work
with its route, resolution, state (`open`, `resolved` or `withdrawn`), who opened and who closed it,
and the open ones with what they put in question. Withdrawing needs a reason and keeps the gap
listed.

The program keeps the record, checks that a run really ran and that the right agent chose, and
suspends only what reads the gap's layer. Whether the gap is real, whether an outcome answers it
and whether a choice is wise stay attributed judgments.
