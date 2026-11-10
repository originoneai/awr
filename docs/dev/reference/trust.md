# Trust fields

Read `trust` beside an observation to identify its provenance. Negotiate support
before depending on the field:

```sh
awr capabilities --require output.trust_field --json
```

The values are `source`, `reported`, `recorded`, `verified` and `reviewed`.
Their canonical explanations are defined once by
[`Trust::legend()`](../../../crates/awr-core/src/trust.rs). Clients can use that
method when they need a legend.

## JSON locations

| Output | Location | Value |
| --- | --- | --- |
| `status`, `work show` | `progress.source_next_action.trust` | `source` |
| `status`, `work show` | `progress.latest_checkpoint_next_action.trust`, when a checkpoint exists | `reported` |
| `session show` | `session_label.trust` | `reported` |
| Evidence records | `trust` beside the evidence metadata | `reported` or `verified` |
| Admission status | `trust` beside the decision state | `reviewed` or `reported` |

In a status queue, each work item's progress is inside its queue entry. The
existing session object, checkpoint verification flags and progress boundary
remain available. The additional `session_label` object carries the declared
agent, provider and model, with `is_caller_proof: false`.

## Evidence and review

Manual evidence registration starts with `reported`, including when the caller
supplies a verification level, digest and timestamp. Caller-supplied types such as
`check_run`, `test_run` and `review` do not grant `recorded`. That value is reserved
for evidence backed by program-generated events; this version does not assign it
to evidence. Evidence becomes `verified` when its exact stored snapshot
matches a report binding in a successful completion receipt. Completion checks
the report bytes against the registered digest; registering a report or preparing
completion does not perform that promotion. Another record with the same locator
does not inherit it.

Evidence trust describes the recorded verification event. It does not reread the
report on each metadata query. Check `currency`, source bindings and report
content separately when deciding whether evidence still supports current work.

An approved admission decision uses `reviewed` when its proposer and decider are
both recorded sessions with different IDs. Inline approvals, self approvals,
proposals, denials and missing decisions use `reported`. Review provenance does
not establish that a decision is current or that the work is complete.

These fields are additive. Human-readable entry formatting is unchanged.
