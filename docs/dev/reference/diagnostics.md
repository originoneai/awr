# Source diagnostics and evidence presentation

YAML syntax and typed ledger-field failures return the existing `InvalidInput`
code with structured `details`:

```json
{
  "location": {
    "locator": "file:///project/work.yaml",
    "pointer": "/work_items/0/title",
    "line": 3,
    "column": 10
  },
  "rule": "ledger.string",
  "repair": "Use a quoted string or a YAML block scalar (|) for multiline text."
}
```

Line and column are one-based. Field pointers use the original source vocabulary,
including configured Chinese names, and JSON Pointer escaping. Syntax failures
may only have parser coordinates. Unsupported location lookup, such as YAML
aliases, retains the field pointer with null coordinates; it does not reject an
otherwise valid source. Diagnostics do not include the rejected value or a raw
source excerpt. Repair text describes the expected structure and does not edit it.

For example, `title: [Draft, Review]` is a list where a string is required. Use
`title: "Draft, Review"` if that is the intended title. Long descriptions can use
YAML block scalars; valid Chinese text and paths with spaces remain supported.
Keep the work record focused on intent, acceptance and current progress; reference
design documents and reports when their bodies are not needed in every query.

A colon followed by a space inside an unquoted list item has the same effect:
`- Report.java: section 2` under `evidence:` is a mapping whose key is `Report.java`,
so the entry has no locator and fails with `ledger.evidence_locator`; the repair text
says so. Quote the whole entry (`- "Report.java: section 2"`) or write
`locator: Report.java` with an optional `summary:`. Text after ` #` in an unquoted
entry is a YAML comment and is dropped without any diagnostic, so quote entries that
contain it as well.

The same mistake under `depends_on:` or `dependencies:` (`- WORK-1: waits for the API`)
fails with `ledger.dependency_identity`, and the repair names the cause: write the work
ID alone (`- WORK-1`), or `id: WORK-1` with an optional `required: false`, and keep notes
in the work's `summary` or `next_action`. Neither diagnostic echoes the rejected text.
AWR's own writer quotes any text that would not read back unchanged and parses the
result before saving it, so only hand-typed entries need this care. Other string
fields report `ledger.string`, whose repair already asks for a quoted string.

`awr source reindex` prints operation success, projection completeness, source
location, rule and repair from the same index report serialized by `--json` and
returned by MCP `awr_source_reindex`. A successful `source scan` can still have an
incomplete projection: scanning observes sources without importing their changed
facts. Check both fields. Compare alternate output formats from equivalent
starting snapshots because reindexing itself updates source state.

`work show` and MCP `awr_work_get` include `evidence_groups` alongside the existing
flat `evidence` list. Each group has one exact locator and separate records with
their own source SHA, digest, scope, branch, evidence level, currency and missing
bindings. Human output prints these groups. A source reference and a verified
report at the same path remain distinct; grouping never transfers verification,
resolves path aliases or changes completion checks.

## Readiness diagnostics: which layer answers

`work show`, `ready`, `status` and `work prepare` of the CLI, and `awr_work_get`, `awr_work_ready`,
`awr_project_status` and `awr_work_prepare` of the MCP server, report whether a work can be selected as `ready` and,
when it cannot, a list of diagnostics `{code, work_item_key, detail}`. Both entries read the same evaluation and shape
a work with the same functions (`awr_core::work_brief`, `awr_core::ready_brief`), so the same facts give the same code,
key and detail whichever entry asks. Claiming, transitions and completion evaluate the same readiness again when they
are submitted (a refusal carries the same diagnostics in `DependencyBlocked`).

The codes answer different questions and are never folded into one another:

| Code | Question it answers |
| --- | --- |
| `unknown_status`, `status_not_selectable`, `work_archived`, `active_blocker` | Work readiness: does the work itself allow selection |
| `missing_dependency`, `dependency_cycle`, `dependency_not_completed`, `source_not_fresh` | Work readiness: do its required dependencies and sources allow it |
| `active_claim` | Execution admission: another session holds an unexpired claim |
| `admission_*`, `containment_*`, `reassessment_*` | Execution admission of a governed work: see [admission](admission.md), [containment](containment.md) and [reassessment](reassessment.md) |

An unknown value is never read as success: `unknown_status` makes a work not ready, and an unknown dependency status
does too.

Context completeness is a separate answer. `context.completeness.complete` and its `issues` describe whether the facts
the context needs are present, not whether a dependency is done. A dependency that is not done but has a fresh, known
status and a next action leaves the context complete and the work not ready; a dependency that is absent, in a cycle or
without a known status makes the context incomplete, and `work prepare` then answers `ContextIncomplete` with the same
body and diagnostics from both entries.

A `ready` answer authorizes nothing. Reading does not claim, and it does not make the MCP server write; a claim, a
transition or a completion is judged on the facts of the moment it is submitted, so a dependency that became unfinished
behind an earlier answer is refused with `DependencyBlocked`, and a claim whose lease ended with `ClaimConflict`.

Two differences are about the envelope and not about the verdict. A failed `work prepare` carries its `error` inside
the result over MCP and on standard error over the CLI, and `response_view=action` names the call for the full result as
a command line (CLI) or a tool call (MCP).
