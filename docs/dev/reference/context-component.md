# Context packing kernel: the contract (EVO-020)

`awr-context-kernel` fixes, as plain data and pure checks, what a host hands to context packing and what it gets back. It is
the seam an external host can use without our store: fixed text segments with stable names in, a checked selection out. The
full AWR work compilation (`awr-context`) stays what builds the request from the project's sources, rules, dependencies and
claims; the kernel never decides what is authorized, ready, complete or accepted.

This release fixes the shapes and the checks. The packing itself (moved out of `awr-context` with the old output and hash
unchanged), the pure parsing entry and the consumer-level budgets are later steps.

```rust
let issues = awr_context_kernel::validate_request(&request, &Limits::default());   // [] = well formed, nothing more
let issues = awr_context_kernel::check_result(&request, &result, &Limits::default());   // [] = a consistent answer
let digest = awr_context_kernel::request_digest(&request);   // "sha256:…", independent of the order of JSON keys
```

## What the kernel takes

| Field | Meaning |
| --- | --- |
| `protocol_version` | `1`. Another version is `unsupported_protocol`, never guessed at. |
| `policy` | `awr.whole_chunks.v1`: required chunks are indivisible, optional ones are tried in stable rank order, one that does not fit is skipped so that a later, smaller one may fit, and the whole text is measured. No other policy is put in its place (`unsupported_policy`). |
| `identity` | The host's own identity binding, opaque, at most 1 KiB. |
| `pinned_entities` | Entities the identity pins (`kind`, `id`, `revision`). A chunk that names one at another revision conflicts (`entity_revision_conflict`). |
| `request_binding` | The host's binding, opaque JSON, at most 16 KiB. |
| `budget` | `unit` (`model_tokens` or `bounded_bytes`), `limit`, `exactness` (`exact` or `estimate`), `scope` in words, and a `tokenizer` for tokens. Tokens and bytes are never mixed and an estimate says it is one. |
| `required` | Chunks that may not be dropped. |
| `optional` | Ranked chunks: `priority` (lower first), `recency` (newer first), then section and key. |
| `groups` | Atomic groups: chunks selected together or not at all. |

A **chunk** has a `key` and a `section` (unique together), a `section_rank` (sections render in that order, chunks by key),
`text`, the `entities` it is about, `encoding` (`utf-8`) and `media` (`text/plain` or `text/markdown`), and optionally the
`cost` the host measured in the budget's unit. Names, ids and kinds are the host's own strings: nothing requires a database
key, and a field the format does not have is refused wherever it stands (a store handle, a writable flag, a claim, a
verdict).

## What a request must be

`validate_request` lists every reason a request cannot be packed, in the order they are found, at most 64. The codes:

| Group | Codes |
| --- | --- |
| Version and bounds | `unsupported_protocol`, `unsupported_policy`, `invalid_identity`, `binding_too_large`, `budget_out_of_range`, `invalid_budget_scope`, `tokenizer_missing`, `too_many_chunks`, `too_many_groups`, `too_many_pinned_entities`, `too_many_entities`, `chunk_too_large`, `input_too_large` |
| Identity | `invalid_name`, `invalid_entity`, `duplicate_chunk`, `entity_revision_conflict` |
| Unsupported input | `unsupported_encoding`, `unsupported_media` |
| Atomic groups | `duplicate_group`, `group_empty`, `group_too_large`, `group_member_missing`, `group_member_repeated`, `group_overlap`, `group_mixes_required_and_optional` |
| Mandatory content | `mandatory_overflow` |

- The bounds (`Limits`) are 4,096 chunks, 1,024 groups of at most 256 members, 64 entities per chunk, 256 KiB per chunk, 8 MiB
  of text, names of 256 bytes, 1,000,000 tokens or 16 MiB; a host may tighten them, not read a request above them.
- **Mandatory content is never dropped to fit.** When the required chunks alone are over the budget, by the host's measure
  or, for a byte budget, by the length of their text, the request fails with `mandatory_overflow`. A group that holds a
  required chunk and an optional one is refused: the optional one could be dropped from it.
- An optional chunk larger than the whole budget is not an error of the request; it is omitted.
- An unsupported encoding or medium (an image, say) fails by name. Nothing is replaced or approximated.
- No code carries the text of a chunk; the identifiers it names are cut at 80 bytes.

## What a result must be

`check_result` is the oracle any packing code is held to; it is written from this contract, not from a packing algorithm.
A result is `selected` and `omitted` lists, a `measurement` and the `rendered_hash` of the text, and is consistent when:

- every chunk of the request is in `selected` or in `omitted`, once (`result_chunk_missing`, `result_chunk_unknown`,
  `result_chunk_twice`);
- no required chunk is omitted, and each `required` flag is the request's (`result_required_omitted`,
  `result_required_flag_mismatch`);
- the members of an atomic group are all selected or all omitted, and each names its group
  (`result_group_split`, `result_group_mismatch`); an optional chunk is omitted for `budget` or, with its group, for
  `group_budget`;
- the measurement is in the unit, exactness, scope, tokenizer and limit of the budget, `required_used` is not above
  `total_used`, and `total_used` is within the limit (`result_measurement_mismatch`, `result_over_budget`);
- it names the request it answers (`result_digest_mismatch`, `result_protocol_mismatch`), carries a `sha256:` hash
  (`result_hash_malformed`) and the fixed note (`result_scope_note`): *a structural packing result; it grants no
  authority, says nothing of readiness, completeness or acceptance, and is not an execution admission.*

A request that passes, or a result that passes, is well formed and consistent. It is not authorized, ready, complete or
accepted: no field of either says so, and a test holds the serialized result to that.

## Boundaries, checked

- The dependency closure of the crate is `serde`, `serde_json` and `sha2`. It needs no `awr-core`, `awr-store`, `awr-source`,
  `rusqlite`, no tokenizer, no runtime and no network client (a test reads the closure with `cargo metadata`).
- The source uses no file, network, process, environment, thread or clock (a test scans it); calling it initializes no
  project and creates no file.
- The same request gives the same digest whatever order its JSON keys came in; any change of a chunk, a group, the budget
  or the binding gives another.

## Corpus

`tests/fixtures/context-kernel/` holds 69 synthetic cases, one file each, with the codes it must give: 12 accepted requests
(external ids, groups of required and of optional chunks, a token budget with its tokenizer, an estimate, Markdown, a
required chunk exactly the budget), 36 refused requests (duplicate ids, entity revision conflicts with chunks and with the
identity, mandatory overflow by bytes, by declared cost and by sum, unsupported encoding, medium, policy and protocol, every
bound, every group fault) and 21 results (three consistent, one of them with an atomic group omitted whole and one using exactly
the budget, and eighteen that break one rule each). A test requires every code to be given by some case.

Limits that would need large inputs are handed in smaller by the case (`limits`), so the files stay small.
