# Verify completed work for a commit

Use `awr verify` before pushing or in CI to check completed work against its registered examples. Local verification reads recorded runs; CI verification executes the checked-out commands and can compare their results with an older commit.

## Use it

```sh
awr verify
awr verify --commit HEAD --work SAVE-CONTENT
awr verify --rerun --baseline origin/main --require-work --json
awr verify install-hook
awr verify install-hook --accept
```

The capability `verify.commit` identifies compatible builds. Supply the baseline
commit available in your CI checkout. A minimal project policy is:

```toml
version = 1

[verify]
require_work = true
```

## How it behaves

| Input | Result |
| --- | --- |
| Default local mode | Selects completion receipts bound to the selected commit's source SHA; evaluates the project's shared completion check gate. |
| `--commit REF` | Resolves a commit, defaulting to `HEAD`. Rerun uses the checked-out `HEAD`. |
| Repeated `--work KEY` | Selects those exact keys in either mode; repeated keys count once. Local mode can reuse eligible ledger-only ancestor observations. |
| `--rerun` | Parses configured YAML ledgers in memory and executes each declared example through the direct check runner. |
| `--rerun --baseline BASE` | Selects work completed since BASE; runs the current oracle files over BASE in a temporary worktree for `fail_before_pass_after`. |
| Successful baseline proof | A failed check at BASE followed by a passing check in the current checkout. |
| Baseline passes | Reports `passed_before_change` and exits 1. |
| Timeout or process error | Reports an inconclusive baseline or failed acceptance and exits 1. |
| Empty selection | Exits 0 with `no_associated_work`. |
| `--require-work` or `[verify] require_work = true` | Local empty selections can use eligible ledger-only ancestor bindings; an empty selection after that exits 1. |
| Every selected check meets its gate | Exits 0; otherwise exits 1 with each work key and unmet reason. |
| `--json` | Emits mode, commit, results, per-work `met`, and overall `met`. Rerun includes check outcomes and output hashes. |

Local mode follows the [completion gate policy](gates.md), including its scope,
current oracle binding, clean-tree requirement and recorded baseline. Observe mode
still produces a failing verification result when its gate has outstanding checks.

Rerun loads `.awr/project.toml` and its configured ledger paths. With a baseline,
`git show` reads the old ledger and identifies newly completed work. Current check
files replace the corresponding baseline files before executing that check. The
temporary worktree and direct-run logs are removed on success and failure.

### Ledger-only local observations

After completing work at C1, committing its ledger can create C2 with the same
implementation. Explicit `--work KEY` can reuse C1's observations when the latest
completion binding still matches the current work, C1 is an ancestor of C2, and
every changed repository path is a `domain = "ledger"` source `path` in
`.awr/project.toml`. Multiple mapped files and empty tree deltas qualify. Renames
check both paths; deletions and file-mode changes also count as path changes.

Ordinary default selection keeps its exact-commit behavior, including successful
empty results with `reason: "no_associated_work"`. With `--require-work` or
`verify.require_work = true`, local mode considers eligible ancestor bindings only
when the exact-commit selection is empty. An existing exact-commit selection stays
as selected. Each explicitly selected work is evaluated separately.

The shared completion gate still evaluates current checks, their frozen oracles,
clean-tree observations and the original completion session. Changed check
commands or files retain their existing rejection reasons, including
`oracle_changed`. Code, decisions, gate policy, rules and other unmapped paths
prevent reuse. Reopened work or changed completion facts require fresh assessment.

The top-level `commit` remains the requested commit. A reused per-work JSON result
adds these fields; other results retain their existing shape:

| Field | Value |
| --- | --- |
| `bound_commit` | Commit whose completion binding and observations were used. |
| `delta` | `"ledger_only"`. |
| `delta_paths` | Sorted repository-relative changed paths, at most 16; special characters are JSON-escaped. |
| `delta_paths_omitted` | Number of additional paths, present only when the list exceeds 16. |

## Install a pre-push hook

`install-hook` previews the script and target path. `--accept` creates an executable
hook at Git's configured hook path, including `core.hooksPath`. An existing hook
keeps its bytes; the output gives an `awr verify` line to add inside its outgoing
commit loop. The [hook example](../../../examples/git-hooks/pre-push) handles
outgoing commits and deleted refs. Set `AWR_VERIFY=off` for an explicit local opt-out.

With `[verify] require_work = true`, a ledger-only commit after a pushed completion
can use its eligible ancestor binding. A code delta with an empty exact-commit
selection is refused as having no associated work. The default hook keeps the
ordinary empty-selection success when this policy is unset.

The [GitHub Actions example](../../../examples/ci/github-actions/awr-verify.yml)
fetches full history, installs a compatible build, and verifies against the pull
request's base commit. Its commands run under the CI job's ordinary permissions.

## Limits

- Rerun requires a repository-local YAML ledger and manifest in the checkout; local mode uses the initialized project's observations.
- Rerun leaves the project state database unopened and absent in a fresh checkout. Check commands themselves run with ordinary subprocess permissions.
- Mutation and justified-alternative discrimination receive acceptance reruns only. Baseline discrimination applies to `fail_before_pass_after`.
- Local verification uses current work definitions and completion receipts; historical receipts superseded by later source edits require reassessment.
- Compile-based baseline checks build the older implementation and can take additional CI time.

## Code and tests

- [CLI implementation](../../../crates/awr-cli/src/verify.rs)
- [Automatic positive and negative checks](../../../crates/awr-cli/tests/verify_cli.rs)
- [Gate configuration](../../../crates/awr-source/src/gates.rs)
