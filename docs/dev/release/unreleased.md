# Unreleased

Changes shipped in 0.6.0 are listed in [0.6.0](0.6.0.md).

## Added

- Explicit requirement items and YAML task `spec_refs`, with bounded, read-only
  `awr spec check` and MCP `awr_spec_check` views of one evaluator. Checks identify
  changed or superseded item bindings, missing task associations and contradictions
  with decision `affected_keys`; applicability never infers requirement coverage.
  References reuse the existing task-contract fingerprint and evidence rules.
  Run `awr source reindex` after upgrading to refresh the changed projections.
  Markdown task bindings and cross-document replacement traversal are deferred.
  See [specification references and coverage](../reference/specification-coverage.md).
- Optional work definitions in YAML ledgers: a work item can record its purpose, adopted intent,
  the goal outcome it advances, responsibility, examples and how its tests will show they detect a
  wrong implementation. `awr work show` and `awr work prepare` (and the MCP work tools) report each
  obligation of the declared purpose and every gap by code, only for work that declares one. The
  check is deterministic and read-only, grants no permission and changes no completion rule. See
  [work definitions](../reference/work-definition.md).
- Opt-in governed plan admission per ledger source. Under `governed_enforce`, claiming work, a
  handoff's claim transfer, a resume's claim inheritance and registering or starting an execution
  need a recorded, approved decision that still describes the current contract, definition,
  reviewed examples and policy; `awr work admit` and the MCP tool `awr_work_admit` record
  proposals and decisions as events (no schema migration). Projects without a policy are
  unaffected, `legacy_observe` reports without blocking, and recovery paths and completion policy
  are unchanged. See [plan admission](../reference/admission.md).
- Controlled decomposition for governed sources: the children of a parent are admitted only while a
  containment decision on exactly this set is current (`awr work contain`, MCP `awr_work_contain`).
  The decision binds the parent's contract, each child's definition, paths and acceptance, and the
  dependencies between them; added or removed children, widened paths and changed dependencies make
  an approval stale by name, and a child outside the parent's paths cannot be approved. Work
  definitions gain optional `side_line` and `outline` declarations. See
  [controlled decomposition](../reference/containment.md).
