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
report and must be the actual revision used for verification, including any
Git/code changes relevant to the checks. AWR does not assume a clean Git
checkout. The task's section fingerprint is a separate contract binding; exact
acceptance coverage still comes from the report's `checks[].criteria` mapping.

The result also carries a `warning` field: `null` when the report is in canonical
form (exactly one trailing LF line ending), otherwise a descriptive message naming
the deviation (no trailing newline, more than one newline, or a CRLF line ending),
the digest the canonical bytes would have, and the fact that the binding stays on
the exact stored bytes. The warning is advisory only — the preflight accepts
non-canonical reports, never modifies the caller's file, and never refuses on
line-ending grounds.

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

For this sequence, `source_sha` is the actual verification revision and the
evidence `sha256` is the exact report digest. On development `main`, work-aware
currency may reuse a record across ledger-only drift only under the section-stamp
rules in [Core concepts](../../series/concepts.md#delivery-and-acceptance); generic
evidence reads without work context remain SHA-strict. An omitted query SHA can
use the work section binding, but verified evidence still requires `source_sha`
and the report, branch, freshness and required-proof gates. `Current` is a
currency label, not proof of execution or completion.

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

Report parsing and field-validation failures return one diagnostic with
`details.location`, `details.rule` and `details.repair`, using the same format as
[source diagnostics](diagnostics.md).
For example, `/checks/0/passed` identifies a missing or failing check result, and
`/source_sha` identifies a malformed or mismatched verification revision. The rule
and repair distinguish these causes without including the submitted value.
Pointers follow the original report's field spelling (including the `result` alias)
and supported positional struct representations. Unlocated errors omit the pointer
and retain parser coordinates when available. Coordinates are not inferred for a
field pointer.

Preflight and completion share report diagnostics. Completed-work revalidation
renders these diagnostics in its existing gap detail. A report structure failure
retains `InvalidInput` during preflight and `EvidenceMissing` during completion;
binding and check failures use `EvidenceMissing` in both. Correct the indicated
problem against the actual verification record, then rerun preflight before
registering evidence. Evidence registration alone does not validate report bytes.
Preflight requires one report covering all current acceptance criteria; completion
can still bind different reports to different criteria. A failed check needs an
actual passing result, not merely an edit to its `passed` flag.

When status is checked with a source SHA, an unchanged task completed through AWR
is verified against the evidence selected by that successful completion on the
same runtime branch. Every selected report, including extra required evidence,
is read and checked again; another passing report cannot replace a missing or
damaged selected report. Ledger-only drift recognized through matching stored
section stamps retains that receipt's exact evidence selection. Rejected attempts
remain available in evidence history. A matching receipt is not discarded when
its evidence fails validation for the requested source revision: that completion
fails verification, even if another report passes. If the task facts/revision or
runtime branch no longer matches the receipt, no recorded selection applies and
status uses the existing conservative source/evidence assessment. Source-declared
completion alone never supplies a verified receipt.

For one preparation, the old work-get plus context-compile pair becomes one call
with the same required context. This is a call-count reduction only, not a claim
about net model tokens, billing or total task duration. Measure those across the
whole workflow, including maintenance and recovery.
