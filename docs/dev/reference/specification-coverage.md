# Explicit specification references and coverage

`awr spec check` and MCP `awr_spec_check` inspect declared requirements and task
references. They report changed versions, superseded items, missing task coverage
and unresolved references. They do not read arbitrary PRD paths, scan prose for
requirements, create tasks, change acceptance, or establish implementation or
completion. This capability is unreleased on `main`; it is not included in 0.6.0.

A work item may also carry a [definition](work-definition.md) that reuses these specification
references as its versioned inputs and derives delivery tiers from their dispositions.

## One authoritative specification

Register the **actual authoritative document**, using the existing
`decisions` domain and `markdown-directory-v1` adapter. Do not copy its requirements
into a second editable specification registry. Explicit metadata annotates the
document's existing identifiers; its body remains the requirement text. The
existing decision status/adoption lifecycle governs its approval.

```toml
[[sources]]
domain = "decisions"
role = "supporting"
path = "specifications"
adapter = "markdown-directory-v1"
```

An illustrative document in that registered directory:

```markdown
---
id: configuration-spec
status: accepted
affected_keys: [BUILD-CONFIG]
specification:
  version: "2"
  items:
    - id: OLD
      disposition: superseded
      superseded_by: NEW
    - id: NEW
      disposition: required
    - id: EXPORT
      disposition: future
    - id: REMOVED
      disposition: cancelled
    - id: EXTERNAL
      disposition: out_of_scope
---
# Configuration requirements

## Decision
NEW replaces OLD. EXPORT is deferred work; REMOVED is cancelled and EXTERNAL
does not belong to the product. Their detailed requirements live in this document.
```

`required` and `future` items need an explicit task association. `cancelled` and
`out_of_scope` items do not. A `superseded` item requires a resolvable replacement
within the same document; missing targets and cycles remain `unknown`. Document
supersession continues to use the existing `superseded_by: DocumentVersion`
metadata, returned with the specification's source reference. No replacement is
inferred from dates, names or similarity.

Cross-document replacement traversal is deferred. A superseded or otherwise not
currently accepted document remains `unknown` with a diagnostic. Its declared
items remain visible as unknown requirements; no `historical` document or reference
state is produced. Selection limits reported findings to that document; source
freshness is still verified project-wide.
Adoption content and identity checks retain the existing source-adapter behavior;
the checker uses that approval status internally without adding a validity field
to the public decision model or deriving approval from prose extraction warnings.

## Bind an explicit task or acceptance criterion

In the existing YAML ledger, add `spec_refs` to the actual work item. Copy the
registered document's exact fingerprint from `awr source list` after indexing:

```yaml
work_items:
  - id: BUILD-CONFIG
    title: Deliver reviewed configuration saving
    status: planned
    acceptance: [Save the reviewed configuration]
    spec_refs:
      - specification: configuration-spec
        item: NEW
        source_fingerprint: "sha256:<64 hexadecimal characters from source list>"
        criterion: Save the reviewed configuration
```

`specification` is a stable registered decision key, not a filesystem path.
`source_fingerprint` is the required version pin. It resolves to the existing
document-version provenance; no database-assigned `source_id` belongs in this
source-owned reference. The fingerprint binds the whole document, including
approval metadata: changing bytes without bumping its human version still requires
review. `specification.version` and the report's `observed_version` retain that
human label for display, without a separate approval/version comparison. Adoption
is an explicit source assertion, not independent proof that a person reviewed it.
Do not automatically refresh a task's fingerprint to make a warning disappear.

`criterion`, when present, must exactly match one current acceptance string.
Omitting it declares a work-wide association, not per-criterion coverage.
Duplicate or malformed references fail source validation. Existing ledgers and
decisions without these optional fields keep their existing behavior.

Use existing reviewed source editing/batch operations to update the binding.
`spec_refs` is supported by YAML work creation and field changes; changing it is
part of the planning contract and cannot bypass an active execution claim.
Markdown task ledgers reject `spec_refs` during source parsing, creation and field
changes, including HTML metadata. Remove that field explicitly from a Markdown
ledger before reindexing; this contribution does not add Markdown reference support.
After upgrading, explicitly reindex: the ledger and decision adapter versions
change so previously ignored metadata is projected. No database schema migration
or additional task authority is introduced.

### Applicability cross-check

`spec_refs` alone determine specification alignment and requirement coverage.
`affected_keys` retain their existing decision applicability meaning for related
work, evidence relevance and workstream scope. The checker does not rewrite them.
It reports contradictions separately in `association_diagnostics`:

| Reason | Meaning |
| --- | --- |
| `referencing_work_not_in_affected_keys` | A task references this specification but is absent from its applicability list. |
| `affected_work_without_spec_ref` | A listed task has no explicit reference to this specification. |
| `affected_key_not_resolved` | A listed key cannot resolve to registered work or a registered goal scope. |

The existing `*` applicability wildcard matches registered work; it does not
create references or prove coverage. Goal keys retain their existing scope
meaning and do not become task associations. Cancelled/archived work still has
its declarations checked, but cannot supply current requirement coverage.
A valid explicit reference can remain `current` and its item `covered` while an
applicability mismatch makes the overall result `needs_review`. An unresolved or
ambiguous source key makes the overall result `unknown`. A listed task
without a reference never fills a requirement gap. These diagnostics include
source provenance and obey the same selection and row/byte limits as other rows.

### Composition with task contracts and evidence

Declared `spec_refs` participate in the existing YAML `section_fingerprint`,
including pin, item or criterion changes that leave acceptance text unchanged.
Work-aware evidence reads and completion use the [existing currency rules](../../series/concepts.md#delivery-and-acceptance):
when the request is for the current ledger hash, eligible ledger-stamped proof
can survive unrelated task edits, while a changed owning task contract makes
that stamped path historical. Other requested revisions and generic
`evidence show` remain source-SHA strict. Unstamped or Git-bound proof is not
automatically stamped;
when its requested SHA still matches, an unstamped record can remain current.
The checker adds no evidence-version field, completion gate or receipt evaluator.

An external specification edit can therefore report `needs_review` while proof
remains current for the unchanged old task contract. That proof does not verify
the revised specification. Explicit rebinding and re-verification follow the
existing source-editing and evidence workflows. Completion retains its exact
selected reports, including extra required proof; another passing report cannot
replace damaged selected proof. No automatic completion revocation is performed.

## Inspect without changing state

```sh
awr source reindex
awr spec check --spec configuration-spec --json
```

MCP: discover `awr_work` then call its `awr_spec_check` child with
`{"spec":"configuration-spec","limit":1000}`. Flat clients can call the child
directly. Shared clients also pass the authorized `project` key. Existing project
read permissions apply; the checker grants no additional access.

Local CLI and owner stdio can inspect the project. Shared HTTP supports this tool
only for project-level grants on projects without workstream isolation. Projects
with workstreams enabled, or callers with scoped workstream grants, receive
`Unsupported` from the existing isolation guard. `awr_workstream` has no scoped
specification-check action; choosing that tool is not a workaround. Team/Postgres
and authenticated cross-workstream inspection are outside this capability.

Read complete task bindings explicitly with `awr work show WORK`, MCP
`awr_work_get`, or the bounded `spec check` report. Ordinary status and ready
queues retain their existing brief shape and do not perform specification checks.

The check itself uses a coherent private snapshot, verifies current indexed
sources before and after, and persists neither source edits nor runtime events.
Changed files/mappings or an upgrade requiring reindex return `SourceStale`;
explicitly refresh and inspect again. Source `fresh` only describes the indexed
bytes. It does not establish that task requirements agree with the specification.

Results contain source references, exact work bindings, declared versions,
replacement identifiers, requirement coverage and reason codes:

| Result | Meaning |
| --- | --- |
| Reference `current` | Its pinned fingerprint, item and optional criterion resolve; the work association is admitted and active. This is specification alignment, distinct from evidence currency. |
| Reference `needs_review` | Approved source bytes changed, an item was superseded or withdrawn, or the mapped work is cancelled/archived. |
| `unknown` | Missing/ambiguous/unapproved specification, unresolved item/criterion/replacement, non-current document approval, draft/unknown work scope, or missing explicit specification declarations. |
| Requirement `covered` | At least one current declared task association exists. This is coverage, not a test result. |
| Requirement `uncovered` | An explicit required/future item has no declared task reference. |
| Requirement `not_required` | Explicitly cancelled/out of scope, or superseded with a resolved replacement. |

Overall `matched` means only that the selected explicit declarations have no
reported gaps. Unknowns take precedence over `needs_review`. References to
unregistered keys remain unknown even when every configured source is fresh;
unregistered files are never opened or silently imported.

`limit` bounds the complete combined specification/requirement/reference/association-diagnostic row
count (default 1000, maximum 10000). Oversized selections return `BudgetExceeded`
without silently truncating findings; select a specification or raise the limit
within the ceiling. Reports also have a 4 MiB serialized ceiling. There is no
project completion percentage, automatic reconciliation or completion-policy
change.
