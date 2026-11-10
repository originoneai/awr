# Trust fields

Read `trust` beside an observation to identify its provenance, and negotiate field
support before consuming it.

## Use it

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

| Evidence operation | Result |
| --- | --- |
| Manual registration, including types `check_run`, `test_run` and `review` | `reported`, including caller-supplied verification levels, digests and timestamps. |
| Successful completion | `verified` when the exact stored evidence snapshot matches a report binding in the completion receipt, after report bytes match the registered digest. |
| Report registration or completion preparation | `reported` until successful byte-checked completion records the matching snapshot. |
| Another evidence record at the same locator | `reported` until that record receives its own matching, successful byte-checked completion receipt. |

An approved admission decision uses `reviewed` when its proposer and decider are
both recorded sessions with different IDs. Inline approvals, self approvals,
proposals, denials and missing decisions use `reported`.

## Limits

- This version assigns evidence records only `reported` or `verified`; `recorded` remains reserved for program-generated evidence events.
- Trust describes historical verification; assess `currency`, source bindings and current report content separately when deciding whether evidence supports current work.
- Metadata queries derive trust from stored completion receipts; current report bytes require a separate read.
- Review provenance describes the recorded approval; decision currency and work completion require separate checks.
- Trust fields extend existing JSON; human-readable entry formatting stays unchanged.

## Code and tests

- [Canonical trust values and legend tests](../../../crates/awr-core/src/trust.rs)
- [Evidence trust derivation](../../../crates/awr-core/src/query.rs)
- [CLI evidence regression tests](../../../crates/awr-cli/tests/evidence_currency_cli.rs)
- [CLI and MCP parity tests](../../../crates/awr-mcp/tests/cli_parity.py)
