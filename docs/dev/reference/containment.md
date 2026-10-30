# Controlled decomposition

Splitting a work item, widening its scope or changing its dependencies is a change of the parent's
set of children. For a source that opts in to [plan admission](admission.md), the children of a
parent are admitted only while a **containment decision** on exactly this set is current. Nothing
here creates a task type, a state or a second planning system: children are ordinary work items
that name their parent in their [work definition](work-definition.md), created and edited by the
existing creation, batch and planning paths. Ordinary steps, waits and retries never need a task.

What the program checks without judgment: every child stays within the paths of the parent, names
the parent, and the decision on this exact set is current and made by an authorized agent. Whether
the children are really contained in the parent's intent stays an attributed judgment.

## What a decision binds

`awr work contain --input containment.json` (or the MCP tool `awr_work_contain`) records one
`work.containment` event on the parent. It binds, read in the same transaction as the write:

| Part | Meaning |
| --- | --- |
| `parent_contract` | the parent's management contract, which includes its definition |
| `change_set` | every child with its title, definition, paths, acceptance, tags and specification references |
| `dependencies` | the dependency edges of the children, including those on work outside the set |
| `children` | the child keys, so an added or removed child can be named |

The request names the parent, who decides (an active `session`, or a declaring `agent`, as for
admission), the revision the caller read, a stable request key, the `change_set_fingerprint` the
decider reviewed (the `current.change_set` of the parent's `containment` block in `work show`), and
the decision: `proposed` or `approved` with a `judgment` (why the children are contained, what
existing work was reused instead of becoming a child, what was left out), or `denied` with a
`reason`. A request is not a decision. The same request key with the same content returns the
original receipt; different content, even a different decision alone, is refused.

## Diagnostics

A governed child whose definition names a parent reports the state of its parent's set in its
readiness, and claims, executions, handoffs and resumes of the child share it:

| Diagnostic | Meaning |
| --- | --- |
| `containment_missing` | no decision is recorded for the children of this parent |
| `containment_pending` | a proposal is recorded and no decision answers it |
| `containment_denied` | the latest decision rejects this set; the reason is in the detail |
| `containment_stale` | the approved set differs from the current one; the detail names `parent_contract`, `change_set`, `dependencies` and each `child_added:KEY` or `child_removed:KEY` |
| `containment_unchecked` | a child leaves the parent's paths (`child_outside_parent_scope`); such a set cannot be approved |
| `containment_not_authorized` | the policy lists `supervisors` and the approving agent is not one of them |

A parent without paths grants no scope to compare against; a policy without `supervisors` lets any
attributed agent approve. Expansion is therefore a visible event: a child outside the parent's
paths is unchecked until it moves back inside, or until the parent's own paths are widened, which
changes the parent's contract and needs a new decision.

## History and denominators

Events are append-only, so earlier proposals, rejections and approvals stay on record. Removing a
child from the source cannot shrink the set unseen: the next evaluation is stale and names the
removed key. A parent's outcome is never proved by its children: finished children leave the
parent's own acceptance to its own evidence, as a definition's `parent_outcome_unproven` note says.

## Side lines and outlines

A [side line](work-definition.md) is work spawned from another line: it names its parent, says when
its result returns and when it stops, and contributes to an outcome like any work. An outline keeps
distant work visible with its known constraints and a trigger for elaborating it; it is never
admitted until it is elaborated into a full definition, so near-term work is executable before it is
admitted. Stop splitting at one independently judgeable result with clear responsibility and entry
conditions; keep the local implementation and its checks together, and keep independent acceptance
of the integrated outcome.
