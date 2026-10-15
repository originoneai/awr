# Preparing work with fewer steps

For measurable task classification and upgrade rules, see [management intensity](management.md).
The frozen first-batch assessment envelope and reuse mapping are in [engineering assessment](assessment.md).

Use AWR's source ledger as a small work contract. Keep the outcome, current state,
completion criteria and next action there. Link design documents and detailed
reports instead of duplicating their contents. Chinese text and paths are supported;
short writing is a convenience, not a parser requirement.

`awr work create --input draft.json` previews one draft containing explicitly known
facts. Apply that same input with `--accept --expected-preview <fingerprint>
--expected-revision <revision>`. The stable request key identifies the whole input.
Query `work create-status --key …` after an uncertain response; use the existing
explicit recovery command for a pending outcome. Do not generate another key to retry.

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

Creation always produces a draft. Missing goals or required facts remain visible;
creation does not grant permission to execute or complete. Supported fields are
summary, goal, acceptance, next_action, kind, paths, tags, priority, depends_on,
owner and milestone. Markdown sources must support the requested fields in their
existing table/checklist shape; unsupported shapes fail before writing.

`awr work prepare WORK --session SESSION --source-sha FULL_SHA` returns readiness,
the required context, claims, persistent waits and a next action from one snapshot.
Use `--goal` for an explicit goal selection. The MCP equivalent is
`awr_work_prepare` (also accepts a bound conversation). The caller must actually
consume the context before acknowledging it in a checkpoint. Readiness is not a
completion claim. CLI refreshes the projection; MCP verifies sources without writing.

`awr work prepare-completion WORK --report report.json --evidence-key KEY
--source-sha FULL_SHA --level locally_verified` reads actual report bytes and returns
the evidence arguments and the acceptance mapping for the existing completion
operation. `awr_completion_prepare` provides the same preflight through MCP.
No command runs, no evidence is registered and no task is completed by preflight.
The level is the caller's declaration. The command, scope and time come from the
report; the file digest is computed by AWR. The supplied source SHA must match the
report and must include any changes relevant to verification in its accompanying
source evidence. AWR does not assume a clean Git checkout.

The CLI's `evidence` object is ready for `awr evidence add --input` without
renaming fields: it contains `work_item_key` and an explicit `branch_id` resolved
from the preflight snapshot. Omitting `--branch` selects that snapshot's current
branch; a name or ID selects that branch, and `--branch main` selects main.
The draft pins the selected branch (`null` for main), even if the project's
current branch changes before recording. Complete with an owned session on the
same branch. MCP keeps its existing `work` and `branch` argument names for
`awr_evidence_record`.

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

When status is checked with a source SHA, an unchanged task completed through AWR
is verified against the evidence selected by that successful completion on the
same runtime branch. Every selected report, including extra required evidence,
is read and checked again; another passing report cannot replace a missing or
damaged selected report. Rejected attempts remain available in evidence history.
If the task facts/revision, branch or requested source SHA no longer match the
completion receipt, status uses the existing conservative source/evidence
assessment. Source-declared completion alone never supplies a verified receipt.

For one preparation, the old work-get plus context-compile pair becomes one call
with the same required context. This is a call-count reduction only, not a claim
about net model tokens, billing or total task duration. Measure those across the
whole workflow, including maintenance and recovery.
