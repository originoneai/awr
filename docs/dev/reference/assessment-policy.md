# Assessment policy (DEC-040)

A project can keep a small, versioned file that chooses among rules the program already has: which optional advice an
[assessment explanation](assessment.md) shows, how much one envelope may hold, and the order of the soft reason codes. It
names rules from a closed set. It holds no script, no expression and no text that is ever interpreted. It can only narrow
what the program does by default; it cannot remove a hard protection and it cannot go beyond what the host allows.

The file is a source. The runtime reads it and never writes it. It changes only through a previewed and confirmed write, and
nothing that is recorded while working (observations, logs, receipts) reaches it.

## The file

`.awr/assessment-policy.toml`, at most 64 KiB, apart from `.awr/policy.toml` on purpose: a problem in one never stops the other.

```toml
format = 1                  # the only layout this build reads; a newer one is refused with "use a newer awr"
policy_id = "local-advice"  # 1 to 64 of a-z 0-9 . _ -, starting with a letter or digit
scope = "project"           # the only scope in format 1 (the default)

[[version]]
version = 1
effective_from = 0          # the first as_of this version is in force for; default 0
extends = "builtin"         # "builtin" (the default) or the number of an earlier version
note = "Quiet the nudges while the release settles"   # for people; never interpreted

[version.advice]            # optional advice, true or false
collect_user_reply = false

[version.limits]            # each from 1 up to the program's own cap; a limit can only be lowered
max_advisory = 8

[version.order]             # a full permutation of the soft reason codes
soft_reason_codes = ["persistent_wait", "deferred_wait", ...]
```

Any key this format does not have is refused. There is no place for a script or an expression, and a value is a boolean, a
whole number, a name from the lists below or a short note.

### What a version can set

| Table | Key | Values |
| --- | --- | --- |
| `advice` | `continue_prepare_current_work`, `collect_user_reply`, `refresh_sources`, `repair_required_materials`, `inspect_claim_conflict`, `record_management_observations` | `false` withholds the advice; `true` keeps it |
| `limits` | `max_assessments` (cap 64), `max_advisory` (32), `max_candidates` (256), `max_return_bytes` (262144) | a whole number from 1 to the cap |
| `order` | `soft_reason_codes` | all of the 11 soft reason codes once each, in the order wanted |

Withholding advice removes the advice and nothing else: the diagnostics, the claim, completion and freshness checks stay as
they are. Advice that a policy withheld is counted in the envelope's `limits.omitted_count` with the truncation reason
`advice_withheld_by_policy`; when nothing is left the envelope says `no_advisory` with `when: advice_withheld_by_policy`.
A withheld advice is not a resource overrun and does not change the hard gate.

### What no policy can touch

- The advice `query_original_operation_result`, `no_advisory` and `request_human_review`: naming them at all, with either value,
  is refused (`protected_rule`). They keep an unknown effect from being run again and say nothing when the authority is gone.
- The hard reason codes (`execution_result_requires_query`, `query_outcome_before_retry`, `work_contract_changed`,
  `unresolved_dependencies`, `plan_invalidated`, `claim_conflict`, `hard_guard_reject`, `resource_limit_exceeded`): the set
  and the slot each has in the order are fixed. `order.soft_reason_codes` fills only the other slots.
- Admission, claims, completion, source freshness and stop or revoke: not part of the policy at all.

## Priority and inheritance

From the lowest priority to the highest:

1. **The built-in policy**: everything on, the program's caps, its default order.
2. **The versions of the file**, from the oldest in the chain to the newest. A version starts from what it `extends` (the
   built-in policy, or an earlier version, which in turn starts from what it extends) and sets only what it names; the nearest
   version in the chain wins. A version without `extends` starts from the built-in policy.
3. **The ceiling**: what the host allows. A library caller passes a `PolicyCeiling` (lower caps, advice the host forces off);
   the CLI passes the program's own caps, since no host restriction exists yet. A project value above the ceiling, and an
   explicit `true` for advice the host forced off, are conflicts and are refused (`exceeds_ceiling`, `conflicts_with_ceiling`).
   Without a project policy the ceiling still narrows the built-in one, and the policy says so.

## Which version is in force

By the `as_of` of the evaluation, never by a clock: the highest version whose `effective_from` is not after it. An explanation
uses the project revision as its `as_of`. Before every version, or without a file, the built-in policy is in force. The same
file, ceiling and `as_of` give the same policy and the same `policy_hash`.

A policy in force gives the envelope its identity: `policy_id` is the file's, `policy_version` the version's and `policy_hash`
a digest of the effective policy. The built-in policy keeps the identity and the hash it always had (none), so a project
without a file sees nothing change. An explanation made under a project policy also carries `policy`: its source, id, version,
hash, the fingerprint of the file and any diagnostics.

## Documents that are refused

A document is checked as a whole; none of it is used if one part is refused. Codes are stable and never carry text from the
document (an unknown name is not echoed).

| Code | Meaning |
| --- | --- |
| `unsupported_format`, `unsupported_scope`, `invalid_policy_id` | the header |
| `too_many_versions` | more than 64 versions |
| `version_sequence` | versions are not numbered 1, 2, 3 … in order |
| `effective_from_order` | an `effective_from` below zero or below the one before |
| `invalid_extends` | not `builtin` and not an earlier version |
| `invalid_note` | longer than 512 bytes, a control character, or content the public-data check refuses |
| `protected_rule`, `unknown_rule`, `deprecated_rule` | an advice name that is protected, not known, or deprecated from this version on |
| `limit_out_of_range`, `exceeds_ceiling` | a limit of zero or above the cap; above the host's ceiling |
| `conflicts_with_ceiling` | `true` for advice the host forced off |
| `invalid_order` | not a full permutation of the soft reason codes |
| `history_changed` | a change that edits, drops or reorders a version already in the file |

A deprecated rule can no longer be set by a version numbered from its deprecation on; older versions keep their meaning. None is
deprecated in this release.

When the file in the project cannot be used (a newer format, malformed, a link, too large, refused by the ceiling) an evaluation
does not read half of it: the built-in policy applies and the explanation's `policy.diagnostics` says why
(`project_policy_newer_format`, `project_policy_invalid`, `project_policy_unreadable`, `project_policy_refused:<code>`).

## Changing the file

```
awr assessment policy show [--as-of N]
awr assessment policy preview --input candidate.toml [--as-of N]
awr assessment policy apply --input candidate.toml --expected-current FP|absent --expected-candidate FP [--as-of N]
```

A script can check that the installed `awr` has these commands with `awr capabilities --require assessment.policy`; it exits
with a failing status when the capability is missing.

- `show` prints the policy in force, the versions the file holds and the fingerprint of the file. `--as-of` defaults to the
  project revision, read from the last recorded snapshot without refreshing anything.
- `preview` writes nothing. The candidate is the whole file as it should be. It prints the fingerprint of the file now
  (`current_fingerprint`), the fingerprint of the candidate, the versions it appends, every refusal, and what would change
  field by field in the policy in force at the same `as_of`. It exits with status 1 when the candidate would be refused.
- `apply` writes the candidate if, and only if, the file still holds the bytes whose fingerprint was read
  (`absent` when there was no file) and the candidate is the one previewed. The check and the write happen under the
  cooperative writer lock of the file, with an exact-byte compare and an atomic replacement; a file that moved on is a
  `SourceConflict` and nothing is written. Confirming the bytes already there writes nothing.
- A change may only **append** versions: those already in the file must stay exactly as they are, so an old `as_of` resolves
  to what it resolved to. A rollback is a new version that restates an older one (`extends = 1` with nothing else, for
  example).
- The first write creates `.awr/.awr-source-<hash>.lock` beside the file; leave it out of version control.

No event is recorded in the project database: the file is the authority, its history is the history of the file in version
control, and the receipt of `apply` carries both fingerprints.

## Old explanations and replays

A replay snapshot embeds the policy it was made under and the hash of it, so a later change of the file never changes how an
old snapshot is read. Policies that existed before this one serialize exactly as they did (the new `disabled_advice` field is
absent when empty).

## Not in this release

An MCP tool for the policy (the file and the three commands are the whole surface), a user-level layer apart from the project,
rules for single works (the scope is the project), and any rule beyond the advice, limits and order above. A prior explanation
made under another policy is not invalidated by a change of policy; it names the policy in its identity and the reader compares.
