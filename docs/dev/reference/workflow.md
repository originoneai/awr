# Preparing work with fewer steps

Prepare work and completion from source-ledger contracts, then bind observed checks to current acceptance through revision-checked writes.

## Use it

See [management intensity](management.md) for classification and upgrade rules, and [engineering assessment](assessment.md) for the frozen first-batch envelope and reuse mapping.

Use AWR’s source ledger as the work contract for outcome, current state, criteria and next action. Link detailed designs and reports. Chinese text and paths are supported.

`awr work create --input draft.json` previews explicitly known facts. Accept the same input with `--accept --expected-preview <fingerprint> --expected-revision <revision>`. Inspect `work create-status --key …` after uncertainty; recover a pending outcome through its explicit recovery command.

```json
{
  "version": 1,
  "request_key": "review-invoice-2026-09",
  "title": "核对本月发票",
  "fields": {
    "summary": "核对金额与订单记录",
    "acceptance": ["差异清单可供复核"],
    "next_action": "读取已授权的发票和订单"
  }
}
```

Creation produces a draft with visible missing facts. Supported fields are summary, goal, acceptance, next_action, kind, paths, tags, priority, depends_on, owner and milestone. Markdown tables/checklists must support the requested fields; unsupported shapes fail before writing.

`awr work prepare WORK --session SESSION --source-sha FULL_SHA` returns readiness, required context, claims, waits and next action from one snapshot. `--goal` selects a goal. MCP `awr_work_prepare` also accepts a bound conversation. Read the context before checkpointing.

`awr work prepare-completion WORK --report report.json --evidence-key KEY --source-sha FULL_SHA --level locally_verified` reads report bytes and derives evidence arguments and acceptance mapping; MCP uses `awr_completion_prepare`. Command, scope and time come from the report; AWR computes the digest. Match the report SHA to the actual verification revision and relevant Git/code changes. The work-section fingerprint separately binds the contract; `checks[].criteria` supplies exact acceptance coverage.

The `warning` field is null for exactly one trailing LF. Otherwise it describes the line-ending deviation, the canonical digest and the actual-byte binding.

The CLI's `evidence` object is ready for `awr evidence add --input` without
renaming fields: it contains `work_item_key` and an explicit `branch_id` resolved
from the preflight snapshot. Omitting `--branch` selects that snapshot's current
branch; a name or ID selects that branch, and `--branch main` selects main.
The draft pins the selected branch (`null` for main), even if the project's
current branch changes before recording. Complete with an owned session on the
same branch. MCP keeps its existing `work` and `branch` argument names for
`awr_evidence_record`.

## How it behaves

| Operation | Result |
| --- | --- |
| `work create` | Preview of an explicit draft |
| `work prepare` | One coherent readiness/context snapshot |
| `work prepare-completion` | Report digest and evidence/completion inputs |
| Record and complete | Separate writes checked against current claims and reports |

### Complete prepared work

For an already-owned session, the CLI sequence is:

```sh
awr work prepare-completion WORK --report report.json --evidence-key KEY \
  --source-sha FULL_SHA --json > prepared.json
jq '.evidence' prepared.json > evidence.json
jq '.completion' prepared.json > completion.json
awr evidence add --input evidence.json \
  --expected-revision "$(jq -er '.project_revision' prepared.json)" \
  --json > recorded.json
awr work complete WORK --session "$SESSION" --reason "Deliver verified work" \
  --input completion.json \
  --expected-revision "$(jq -er '.project_revision' recorded.json)"
```

Each write requires the current project revision. If another operation advances
it, read the current state and recheck the report, branch and claim before retrying.

For this sequence, `source_sha` identifies the verification revision and evidence `sha256` identifies exact report bytes. Work-aware ledger-drift currency follows [section-stamp rules](../../series/concepts.md#delivery-and-acceptance). Verified evidence requires source SHA, report, branch, freshness and required-proof bindings.

The report uses this structure; replace every fixture value with actual evidence:

```json
{
  "version": 1,
  "work_item": "WORK",
  "source_sha": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
  "command": "the command or verification procedure actually executed",
  "scope": ["the scope actually verified"],
  "verified_at": 1,
  "checks": [{
    "name": "independent check name",
    "passed": true,
    "details": "the observed outcome, not an intended result",
    "criteria": ["an exact current source acceptance criterion"]
  }]
}
```

Preflight checks all current criteria. Recording and completion remain separate
writes with current revisions. Completion rechecks claims, dependencies, source
freshness and report bytes; changing a report after preflight invalidates its digest.
Query a shared MCP write by its stable request ID when its outcome is unknown.

Report parsing and field errors use `details.location`, `details.rule`, `details.repair`, matching [source diagnostics](diagnostics.md). `/checks/0/passed` identifies an absent/failing result; `/source_sha` identifies malformed/mismatched revision. Pointers retain original field spelling, including `result`, and supported positional representations. Unlocated errors retain parser coordinates when available.

Preflight and completion share report diagnostics; revalidation keeps them in existing gap details. Structure failures use `InvalidInput` in preflight and `EvidenceMissing` in completion; binding/check failures use `EvidenceMissing` in both. Correct the actual record and rerun preflight before registration. Preflight covers all current criteria in one report; completion may bind several reports.

When source-SHA status matches a successful completion on the same runtime branch, it revalidates the entire selected evidence set, including extra required proof. Matching ledger section stamps retain that selection. Rejected attempts remain in history. Changed task facts/revision or branch use the conservative source/evidence assessment.

Work preparation replaces work-get plus context-compile with one call carrying the same required context.

## Limits

- Short writing is a convenience; the parser accepts the supported full text and paths.
- A request key identifies the whole creation input; retries reuse that key.
- Draft creation grants neither execution permission nor completion.
- Preparation reports readiness rather than completion. CLI refreshes projections; MCP verifies sources with persistent state unchanged.
- Preflight reads reports; check execution, evidence registration and completion are separate operations. Evidence level is caller declared.
- Verification revision includes relevant uncommitted changes; a clean Git checkout is not assumed.
- Noncanonical reports remain accepted and unchanged; line-ending warnings are advisory, and digests bind exact stored bytes.
- Standalone evidence reads remain SHA-strict; omitted work-query SHA can use the section binding. `Current` describes currency rather than execution or completion.
- Diagnostics omit submitted values; field pointers and parser coordinates are separate locations, with no inferred coordinates.
- Registration alone leaves report-byte validation to preflight/completion. A failed check needs a real passing result rather than editing `passed`.
- A matching receipt retains its selected reports even when one fails. Another passing report cannot replace missing, damaged or invalid selected proof.
- Source-declared completion supplies no verified receipt.
- The measured improvement is call count. Model tokens, billing and task duration require whole-workflow measurements including maintenance and recovery.

## Code and tests

- [Completion CLI tests](../../../crates/awr-cli/tests/work_complete_cli.rs)
- [CLI/MCP parity](../../../crates/awr-mcp/tests/cli_parity.py)
