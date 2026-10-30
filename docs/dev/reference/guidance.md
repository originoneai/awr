# Guidance and overhead for governed work

Governed work reports many things: its definition, its admission, the containment of its children,
its gaps, reassessment, budgets and findings. An agent should not have to read all of them to know
what to do next. In its action view (`awr work prepare --response-view action`, and the same view of
`awr_work_prepare`) the response carries **one bounded instruction** for the earliest material gap,
and every other block stays where it is and is loaded on demand.

## The instruction

| Field | Says |
| --- | --- |
| `when` | the condition the instruction applies to |
| `basis` | the diagnostic and its detail it rests on, so it can be checked |
| `next_action` | what to do, concretely |
| `recheck` | what makes it worth looking again |

It is at most 1 KiB, whatever the detail it quotes; for a clear small task it measures a few hundred
bytes. Required context that is incomplete, a session that has to be recovered and a persistent
wait come before any governance gap, as before.

## The earliest material gap

When several gaps hold a work, the one that has to be dealt with first is told:

1. a policy that cannot be read, or a state that could not be established
2. the definition: a work that has none yet, or the obligations its purpose leaves unmet
3. the admission decision: missing, pending, denied or stale
4. an open gap that puts the admission in question
5. the containment of the parent's set of children
6. a gap that puts that containment in question
7. reassessment of failures and unknown outcomes, then the rework and effort budgets
8. blocking findings, which hold completion rather than readiness

Work that governance asks nothing of gets no governance instruction, and a source without a policy
is untouched.

## Unknown, denied and stale are different things

- **Unknown** (`admission_unknown`, `reassessment_unknown`): the state could not be established.
  Refresh the sources and prepare again. It is neither an approval nor a denial, so nothing is
  claimed or run on it.
- **Denied**: the reason is the basis. Address it in the definition or the scope and propose again
  with a new request key; the denial stays on record.
- **Stale**: an approval no longer matches the work, and the detail names what moved. Decide again
  on the current contract. A supervisor replanning within the scope that was granted needs no new
  permission from the user; widening the scope or the intent does.

## Forming and resuming

Initial formation and resumption start from the earliest gap, not from the beginning. A work with no
definition is told to declare one in a single source edit and admit it with a single `work admit`
call; a work whose definition has gaps is told which ones. Judgments are events of the work, not of
a session, so a decision made for one session is current for the next and is not asked for again
while the work is what it was decided to be. Clear small work needs no stage document, no round of
confirmations and no step-level task, and a source that opts in to governance adds none.

## Overhead

Governance is measured, not assumed. `crates/awr-cli/tests/governance_overhead_cli.rs` takes a
clear small task of each purpose through the shipped binary from its context to the end of its
session, once without a policy (the baseline) and once governed, and counts what can be counted:
the calls of the CLI and the bytes they return. Model tokens, cost and wall-clock time are not
measured and nothing is inferred about them.

| Target | Value |
| --- | --- |
| extra calls of a clear small task | at most 1 |
| guidance of a clear small task | at most 2048 bytes |
| median extra-call ratio over the flows | at most 0.2 |

Today the only step governance adds is the `work admit` call. Run
`cargo test -p awr-cli --test governance_overhead_cli -- --nocapture` to read the report. A path that
needs two extra calls for definition, review and admission fails the one-call target, and exceeding
a target blocks adopting governance for a project until the path is simplified or the user accepts
the overhead.
