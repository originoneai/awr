# Daily work navigation and small edits

These capabilities describe the current source tree; installing released 0.4.0
does not enable them.

## Find the next action

`awr status` and MCP `awr_project_status` default to `view="action"`. Their shared
projection separates four queues, with at most five entries per queue and exact
total/omitted counts:

| Queue | Meaning | Next step |
| --- | --- | --- |
| `current` | Structured active/claimed work without a known wait or blocker | Prepare context; continue with the owning session or explicitly resume it |
| `ready` | Source structure and claim readiness both pass | Prepare context and acquire ownership |
| `waiting` | A persistent user wait, unresolved execution record, or unfinished dependency | Obtain the reply or inspect the original outcome/prerequisite before retrying |
| `blocked` | Invalid structure, unavailable dependencies, source problems or explicit blockers | Inspect the cited work and fix the cause |

Use `--work KEY` (repeatable), `--goal KEY`, and `--milestone KEY` to narrow the
selection. Selectors intersect. MCP uses `work`, `goal`, and `milestone`.
The queue is navigation, not an execution authorization. Claims, source freshness,
required context and completion checks remain enforced by their existing actions.
Execution classification uses the registry and latest host reports, not a live
process probe. A host-reported success or failure does not verify task completion.
Unknown/interrupted results require inspection. Pending runtime findings remain
project-wide; filesystem-only and host-private receipts require their own queries.

`awr ready` / `awr_work_ready` retain their scheduling meaning: **eligible for a
new claim**. In-progress work is deliberately absent. In that legacy report,
`blocked_total` means not selectable, including work already claimed.

**Compatibility:** scripts needing the previous full status shape must explicitly
select `--view full` / `view:"full"`. The previous `summary` view also remains
available; its `blocked_count` is the legacy not-selectable count. The action view
has its own `view` and `schema_version`; its `blocked_count` counts actual current
blockers, and its `ready_count` additionally requires the declared work structure.
Do not compare differently defined counts as progress changes.

## Source plans and recorded progress

`status --view action`, `status --view summary`, and `work show KEY` expose a
`progress` object. MCP `awr_project_status` and `awr_work_get` return the same
fields. A single-work status selection also exposes `progress` at the top level,
including blocked or completed work that is absent from the current queue.

- `source_next_action` contains the authoritative text, source locator, pointer,
  source revision and freshness. `projected_at` is the time AWR imported that
  source revision, **not** the time someone edited the file. Missing historical
  projection receipts leave that timestamp null.
- `latest_checkpoint_next_action` contains the most recent checkpoint for this
  exact work, branch and current workstream ownership, including active sessions.
  It identifies the checkpoint, session, recording time, declared caller, session
  labels and the unverified context-hash boundary. No checkpoint means null.
- `differs_from_source` compares the full next-action strings. A difference is an
  observation: saving progress does not modify the original contract or establish
  completion. Legacy `next_action` fields keep their existing meanings.

Progress text is capped at 240 characters and includes a `truncated` flag. Use
`work show KEY` for the full source action and `session show SESSION` for the full
checkpoint. A long historical preamble in a source next action should be corrected
in that authoritative source; AWR does not guess a replacement from code edits.

For example, if the source says “Draft the guide” and a checkpoint says “Investigate
the failed example”, both are visible. Reindexing unchanged source files still
reports zero indexed sources. Tests that have failed may be recorded in a checkpoint
without marking the source work complete.

## Read history without a repair flood

The action view focuses diagnostic samples on selected open work, associated goals
and required dependencies. Historical completions are aggregated under `history`:

- `not_checked`: no explicit source SHA was supplied on this query.
- `check_blocked`: verification was requested, but structural gaps prevent it.
- `verification_failed`: a check ran and did not validate the completion.
- `verified_completed`: reports passed for the explicitly requested source SHA.

No query fills missing historical SHAs or rewrites source completion status. Use
`work show KEY` to inspect original evidence and its recorded version. Use
`intake inspect --source-sha SHA` for the complete project audit against a known
version; running it against current HEAD does not migrate old proof. Ordinary
user confirmations and business checks retain their separate policy counts.
Historical warnings alone do not block unrelated current work or certify delivery.
Full diagnostic consumers should handle `completion_not_checked`,
`completion_check_blocked`, and `completion_verification_failed`, which replace
the ambiguous `completion_unverified` code.

## Edit common fields without hand-editing YAML

Preview by task identity:

```sh
awr --json work edit GUIDE-1 --request-key guide-next-1 --actor writer \
  --reason 'Clarify the review step' --next-action 'Review the conclusion'
```

The compact response shows the source location, old/new field values,
`change.source_fingerprint`, `preview_fingerprint`, and `project_revision`.
Review them, then repeat the same command with:

```sh
--accept --source-fingerprint SOURCE_FINGERPRINT \
--expected-preview PREVIEW_FINGERPRINT --expected-revision REVISION
```

Supported options are `--title`, `--summary`, `--priority`, and `--next-action`.
The existing host-save writer preserves unrelated source facts and comments,
checks source fingerprints and revisions, and keeps its durable recovery journal.
Use `host status --key guide-next-1` after an uncertain response, and `host recover`
only for an inspected pending operation. Identical applied requests return their
original outcome. A new edit requires a new request key and a fresh review.

MCP uses the existing change tools, without another tool in the catalog:

```json
{
  "request_id": "guide-next-1",
  "reason": "Clarify the review step",
  "change": {
    "kind": "work_edit",
    "work": "GUIDE-1",
    "fields": {"next_action": "Review the conclusion"}
  }
}
```

Send this to `awr_change_preview`. For `awr_change_apply`, use the returned
`change` (which includes the resolved source fingerprint), the same request ID and
reason, and the returned revision/preview fingerprint. Query or recover with
`kind:"work_edit"`. MCP client provenance and project/write permissions are
unchanged. A small field edit never changes ownership, lifecycle or verification;
use dedicated work actions for those, or existing reviewed batches for wider edits.

## Text typed into YAML by hand

Prefer `work edit`, the work actions and `evidence add`: AWR's writer quotes any text
that would not read back unchanged and parses the result before saving it. When a
ledger entry has to be typed directly, quote every entry that contains a colon followed
by a space or a space followed by `#`:

```yaml
evidence:
  - "Report.java: refresh() parses both columns"   # quoted: one string
  - Report.java: refresh() parses both columns      # unquoted: a mapping, rejected
  - src/Report.java #12                             # unquoted: " #12" is a comment, dropped
```

Unquoted, `Report.java: refresh() ...` is read as a mapping with the key `Report.java`,
so the entry loses its locator, and everything after ` #` silently disappears. The
diagnostics for `evidence` and `depends_on` entries explain the colon case
([source diagnostics](diagnostics.md)); nothing can report the dropped comment. A
full-width colon is not a substitute for quotes: it hides the problem by changing the
text.

## Diagnose intake rejection

Sensitive-source errors retain `RuleViolation` and add safe `location`, `rule`,
and `repair` details. CLI text and JSON/MCP use the same diagnostic. Line/column
coordinates refer to original UTF-8 source text; unavailable coordinates are null,
never guessed. Matched values and surrounding text are not returned.

Complete value-free TypeScript interface/type/class declarations can contain
primitive property annotations. Bare `password: string` remains ambiguous as a
YAML value. Actual assignments, initializer values and credentials are still
checked. See [secret boundaries](secret-boundaries.md) for the precise supported
forms and structured schema alternative.
