# Evidence currency read paths

This page freezes one rule: **one surface, one entry point, one answer**. It
lists every public read path that reports evidence currency, the single function
that decides it, and the question that function may answer. It does not add a
second currency authority, and it does not make presentation grouping resolve
aliases.

## The rule

1. Evidence currency is decided only by `EvidenceRecord::assess`. A surface that
   reports currency calls it and nothing else; two entry points may never be
   consulted for one answer.
2. The entry point is chosen by the question being asked, never by whether a
   work id happens to be in hand. A surface with work context and a surface
   without it use the same function; they differ only in the source revision
   they request.
3. The entry point is strict by construction. `assess` takes the requested
   source SHA, the section fingerprint and the branch; it carries no
   ledger-fingerprint argument, so no ledger state can widen its answer. This is
   a signature property, not a runtime branch, and it stays true as long as no
   ledger parameter is added.
4. Same record, same surface, one answer. Each invocation returns exactly one
   currency value with its reasons and missing bindings; a surface never
   combines two rules or reports one record twice.
5. Presentation grouping is not currency. Groups are keyed by the exact locator
   string; aliases are never resolved and records are never merged, transferred
   or inferred across groups.

## Surfaces

| surface | site | question | requested source revision |
| --- | --- | --- | --- |
| `evidence_for_work` / `evidence_for_work_scoped` | `crates/awr-store/src/evidence.rs` | is this record current for this work item? | the caller's source SHA |
| `context_evidence_for_work` | `crates/awr-store/src/scoped_context.rs` | work-scoped listing used to compile context | the caller's source SHA |
| `validate_completion_binding` | `crates/awr-store/src/work_action.rs` | is every record in this binding still current? | the binding's source SHA |
| `verify_completed` | `crates/awr-runtime/src/organization.rs` | may this completed work item still be reported as completed? | the organization snapshot SHA |
| `evidence show` | `crates/awr-cli/src/records.rs` | what does this record say and how does it stand? | the requested SHA, or none |
| `record_evidence` | `crates/awr-store/src/evidence.rs` | can this draft be recorded at all? | none: it validates the draft |
| completion report self-check | `crates/awr-core/src/completion.rs` | is this report valid for the binding it names? | the report's own source SHA |

Consumers that only re-print one of these answers (CLI query output, MCP
operations, related-work context) inherit the row above them and add no rule of
their own.

## Completion bindings

A completion binding proves the whole set of records it names. It counts as
proof only when every named record is current for the request; if any named
record is not current, the binding is treated as absent and the established
re-verification decides instead. A partially current binding is never accepted,
so a stale record cannot ride along on a current neighbour.

## Changing this page

A new read path, or a second entry point for any reason, must be added to the
table above before it ships. The table is the contract: a surface that is not
listed here has no documented currency answer.