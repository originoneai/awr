# Team governance

A source may declare **governance** for a Team: which work needs a recorded, approved admission
decision before it is claimed or run, and what each governed work is defined to be. The Team reuses
the rules of [plan admission](admission.md) (`evaluate_admission`, with the same binding of contract,
definition, examples and policy), supplies the facts from its own store, and attributes every
decision to the **verified** caller. A source describes governance; it cannot approve anything.

## What a snapshot declares

A publish package may carry `governance.json` (`awr_source::PublishPrepOptions::governance` writes
it and `governance_digest` binds it). It holds the effective admission policy of the source and, for
each governed work, its tags, its declared definition and the summary of its definition report
(whether a definition is declared, whether its intent is adopted, the codes of its gaps). The summary
is computed where the whole source is at hand and is the publisher's statement, like the rest of the
package; the Team does not recompute it. The file is canonical (works in the order of their keys,
once each, only works of the snapshot) and installs with the snapshot into `snapshot_governance`
and `work_governance`.

* A work the declaration does not list owes a definition, so work added to a governed snapshot can
  never escape governance by being absent from it.
* Once a snapshot has declared governance, a package without `governance.json` is refused: publishing
  cannot drop admission for every work at once.
* Contract codecs and contract hashes do not change. A definition or policy change makes an earlier
  decision stale through its own binding, not through the contract hash.

## Reading and deciding

`work.admission` (a read) says whether the work is governed, its state (`missing`, `pending`,
`denied`, `approved`, `stale`), the diagnostics with the reason of each, the `binding_fingerprint`
of the scope a decision will be checked against and the `admission_version`, the number of decisions
recorded so far.

`admission.decide` records one decision in the append-only `work_admissions` table. It needs the
planning approval permission (`planning.approve`), so a developer cannot admit work, and it takes
`admission_decision`, the `binding_fingerprint` and the `expected_admission_version` that were read.
A decision on a scope that moved since it was read is refused as a changed precondition and records
nothing. So is a decision made after another one was recorded: two deciders who read the same
history get one decision, the other reads again and decides on what the first one decided.

| `admission_decision` | needs | meaning |
| --- | --- | --- |
| `proposed` | `admission_judgment` | a request for a decision; it admits nothing |
| `approved` | `admission_judgment`, or `admission_proposal_id` | admits the work for the scope it binds |
| `denied` | `admission_reason` | refuses admission; a later decision replaces it |

The decider is the verified actor of the request, never a label in the body, and the person it is
linked to is recorded with it. When the policy requires independent review for the work's tags, an
approval answers a proposal made for the same scope by **another person**; the program shows only
that the persons differ, never that the reasoning is independent. The same `request_id` returns the
original receipt.

## What is held

With an enforced policy (`governed_enforce`), `claim.acquire`, `task.assign`,
`task.accept_assignment`, `task.claim_available`, `handoff.accept`, `execution.prepare` and
`execution.start` answer
`AdmissionRequired` with the reason (`admission_missing`, `admission_pending`, `admission_denied`,
`admission_stale`, `admission_obligations_unmet`) until the decision is current. Claim and dispatch
share one check, so the same decision serves both. Reading, checkpoints, ending a session, releasing a
claim, reporting or cancelling an execution and every other recovery path are never held. Under
`legacy_observe` the state is shown and nothing is held.

Source ingest, import and restore never write a decision: an imported snapshot that declares
governance starts with none, and logical backups carry no admission rows (see the backup limits).

Callers can race, and the decision still decides: the project writer lock serializes commands, two
deciders who read the same history get one decision, rival claims of one decision give one claim, and
a denial that follows a claim holds `execution.prepare` and `execution.start` without taking anything
from the session, which can still checkpoint and release the claim. A transfer needs the decision to be
current when `handoff.accept` runs, not only when the claim was taken. Admission belongs to the work
and its stream: reading or deciding work of another stream needs that stream's own grant and is
refused otherwise, a decision of one work never stands for another work that requires it or is
required by it, and an admitted work still waits for the work it requires.

## Reviews cover the scope they opened under

A review round for governed work records the fingerprint of the governed scope (contract, declared
definition, examples and policy) when it opens (`governance_binding`, also in the `review.open`
receipt). If the definition, the examples or the policy of **that work** change in a later snapshot,
the round no longer covers it: `review.accept`, `review.return` and `review.decide` on the old round
and `work.complete` on an approval given before the change answer `ReviewRequired`, and a new
`review.open` starts a round under the new scope. Other work is untouched, because a round binds only
its own work's scope, and nothing is deleted: sessions, checkpoints, evidence and earlier rounds
stay readable, so a session can still recover and continue.

## Not in this change

Structured review findings with their severity and criterion, re-review limited to unresolved
blocking findings and the delta, and the deferral of a new non-blocking finding in a second round are
the last part of the shared-governance work. Containment, reassessment and gaps remain Core
features.
