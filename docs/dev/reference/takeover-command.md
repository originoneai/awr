# Take over a project with `awr takeover`

`awr takeover` brings an existing project to an accepted AWR initialization in saved, resumable steps and asks no question on the way. It runs the CLI's own `init` and `intake inspect`, applies an optional reviewer's draft under fixed guards, and writes a receipt of what it mapped, created and left for the owner to confirm.

## Use it

```sh
awr --project /absolute/project takeover
awr --project /absolute/project takeover --reviewer /absolute/reviewer --review-timeout-secs 600
awr --project /absolute/project takeover status
```

Without a reviewer the draft AWR generated is applied as it is. A run that stops continues when the same command runs again; steps that finished are skipped. The project's own files stay byte-identical, `.awr/` and the `.gitignore` entries AWR lists itself are the only additions, and nothing is committed or pushed.

## How it behaves

| Step | What runs | Writes to the project |
| --- | --- | --- |
| `preflight` | The project exists, has no `.awr/` (or `--adopt` is given) and has no active session; Git `HEAD` is noted | Nothing |
| `preview` | `init` preview, `init --write-draft`, and a deterministic `facts.json` in the state directory | Nothing |
| `review` | The reviewer turns the draft into a reviewed draft | Nothing |
| `accept` | `init --from-draft` preview, a check of its writes, then `--accept --expected-preview` | `.awr/` and AWR's ignore entries |
| `organize` | `intake inspect` | Nothing |
| `receipt` | `.awr/takeover/receipt.json`, copied to the state directory | The receipt |

| Option | Meaning |
| --- | --- |
| `--reviewer <PROGRAM>` | Command that reviews the draft; `--reviewer-arg <ARG>` adds one argument per use |
| `--review-timeout-secs <N>` | Seconds the reviewer may run; default 900 |
| `--manifest <FILE>` | The sources the owner chose; the draft maps and reads exactly these and lists every other file by path and size |
| `--await-review` | Stop after the preview with the draft and the review request for the host (see Host-driven review) |
| `--reviewed <FILE>` | The host's answer to that request, in the reviewer's answer format |
| `--fact-commit-limit <N>` | Commit ceiling for the fact baseline, from 1 through 100,000; default 100,000. Changing it refreshes a preview before accept |
| `--adopt` | Organize an initialized project that has no active session; `preview`, `review` and `accept` are skipped |
| `--max-rounds <N>` | Preview, review and accept rounds when the sources change meanwhile; default 3 |
| `--state-dir <DIR>` | Run records and drafts, outside the project; default `$AWR_TAKEOVER_STATE`, else `$XDG_STATE_HOME/awr/takeovers`, else `~/.local/state/awr/takeovers` |
| `--rerun` | Start a new run although the last one completed |

### Deterministic fact baseline

Every preview writes `facts.json` beside `draft.json` in the external run directory. The preview step result carries `facts_path`, `fact_commit_limit`, and `fact_coverage`; the completed receipt carries the same coverage under `facts.coverage`. An adopted run collects this baseline while creating its receipt because its preview step is skipped.

The document has `schema_version: 1`, `document: "awr_takeover_fact_baseline"`, and `verification: "recorded_unverified"`. Its sections are `repository`, `commits`, `branches`, `tags`, `rule_files`, `coverage`, and `notes`. Programs perform collection, grouping, and counting. Historical entries describe recorded activity; acceptance evidence establishes completion separately.

```json
{
  "schema_version": 1,
  "document": "awr_takeover_fact_baseline",
  "verification": "recorded_unverified",
  "repository": {
    "id": "F-REPOSITORY",
    "state": "ready",
    "head_state": "branch",
    "shallow": false,
    "history_complete": true,
    "submodules": []
  },
  "commits": [
    {
      "id": "F-C-0123456789abcdef0123456789abcdef01234567",
      "oid": "0123456789abcdef0123456789abcdef01234567",
      "author": {"name": "Fixture Author", "email": "fixture@example.invalid"},
      "authored_at": "2026-01-02T03:04:05+00:00",
      "title": "Synthetic example",
      "paths": {
        "changed": 1,
        "added": 1,
        "modified": 0,
        "deleted": 0,
        "other": 0,
        "sample": [{"path": "README.md", "status": "A", "withheld": false}],
        "sample_truncated": false
      },
      "bulk_import": true
    }
  ],
  "branches": [],
  "tags": [],
  "rule_files": [],
  "coverage": {
    "id": "F-COVERAGE",
    "commits_read": 1,
    "commits_available": 1,
    "commit_limit": 100000,
    "commits_truncated": false,
    "current_tracked_files": 1,
    "bulk_import_file_count": 1,
    "bulk_import_file_share": {"numerator": 1, "denominator": 1, "percent": 100.0},
    "unmerged_branch_count": 0,
    "workstream_assignment_ratio": "not_computed"
  },
  "notes": []
}
```

#### Stable fact identity

Fact identity uses the source object rather than an array position. The full identifier remains stable when another commit arrives or traversal order changes.

| Fact | Identifier rule |
| --- | --- |
| Repository | Fixed `F-REPOSITORY` |
| Commit | `F-C-<full Git object ID>`; both SHA-1 and SHA-256 object IDs are supported |
| Branch | `F-B-<SHA-256 of the full refname>` |
| Tag | `F-T-<SHA-256 of the full refname>` |
| Rule file | `F-R-<SHA-256 of the normalized project-relative path>` |
| Submodule | `F-S-<SHA-256 of the normalized project-relative path>` |
| Coverage | Fixed `F-COVERAGE` |
| Note | `F-N-<UPPER_SNAKE_CASE note code>` |

Path samples and author fields belong to their containing commit fact. RCN-003 and RCN-004 cite these identifiers directly; sorting or adding unrelated facts preserves their meaning.

#### History, refs, and rules

Commit collection covers objects reachable from every local branch, remote branch, and tag. Each commit fact records author name and email, strict ISO authoring time, subject, and first-parent changed-path status counts. Merge path summaries use the first parent. The serializer selects the subject only, and `--name-status` supplies path metadata; message bodies and patch text stay outside the document. Each commit stores up to 20 path samples plus exact status totals and `sample_truncated`.

Branch facts cover local and remote refs. `merged_into_head` and `unmerged_branch_count` use the current HEAD as their basis. Tag facts include lightweight and annotated tags, with `target` holding the peeled target when annotation supplies one.

Rule discovery recognizes fixed names for contribution guides, CODEOWNERS review ownership, CI definitions, Agent instructions, and issue templates. Hidden directory traversal uses this whitelist: `.github`, `.gitlab`, `.circleci`, `.buildkite`, `.gitea`, and `.forgejo`. Root pipeline names such as `.gitlab-ci.yml` are part of the fixed-name set. Other hidden directories remain outside rule discovery. A readable public rule file contributes its path, category, byte count, SHA-256, and UTF-8 text. Suspicious content produces `read_status: "withheld_sensitive"`, a category, and null text and digest fields.

#### Coverage and limits

- `commits_available` counts commits reachable in the checkout. `commits_read` counts parsed records, and `commit_limit` records the selected ceiling. `commits_truncated` and the `commit_limit` note describe ceiling exhaustion with exact counts.
- `history_complete` combines ceiling coverage with the shallow-repository signal. A complete read of available shallow commits retains `history_complete: false` and a `shallow_history` note.
- A bulk import commit adds at least half of the current regular tracked paths. Its `bulk_import` field is true, and its fact ID appears in `bulk_import_commit_ids`.
- `bulk_import_file_count` counts distinct current regular tracked paths added by those commits. `bulk_import_file_share` gives the exact numerator and denominator plus a percentage rounded down to two decimals. `bulk_import_added_path_occurrences` separately retains the historical addition total.
- `workstream_assignment_ratio` is `not_computed` in this version. RCN-003 supplies that calculation.
- Rule coverage records discovered, recorded, safely read, withheld, oversized, and unreadable totals. Discovery records at most 256 rule files and reads at most 256 KiB per file. `truncated` signals additional candidates.
- Submodule coverage records each gitlink and its initialization state. Commit totals describe the superproject; a `submodule_history` note states that boundary.

| Checkout | Repository and receipt meaning |
| --- | --- |
| Directory without Git | `not_a_git_repository`, zero Git coverage, `no_git_history` note; fixed-name rule discovery still runs |
| Git repository with zero commits | `empty_repository`, `unborn` HEAD state, zero reachable commits, `empty_repository` note |
| Shallow clone | `shallow: true`, available-count coverage, incomplete full history, `shallow_history` note |
| Detached HEAD | `detached` HEAD state with the exact commit; unmerged branches still use that commit |
| Superproject with submodules | Structured gitlinks and initialization states; `submodule_history` states the history boundary |

With `--await-review`, fact collection leaves both project files and Git state byte-identical. Writes belong to the external state directory. The same repository state produces byte-identical `facts.json` output.

### Reviewer contract

The reviewer is any command; AWR starts it without a shell and sends one JSON object on stdin.

| Direction | Fields |
| --- | --- |
| stdin | `project_root`, `draft`, `draft_path`, `round`, `sources`, `ambiguous_domains` |
| stdout | `generated_files`: object of path to text under `.awr/intake/`; `notes`: short text; optional `authority_mapping` and `ambiguous_domains` to settle an ambiguity |

AWR reads the answer from plain JSON, from the last fenced `json` block, or from the outermost braces. The receipt keeps the notes and the files the reviewer changed.

### Host-driven review

A host that cannot be started as a reviewer command, such as an agent running inside an app, reviews the draft itself in two runs.

```sh
awr --project /absolute/project --json takeover --await-review
# outcome "awaiting_review": record.stop.detail has draft_path and request_path
awr --project /absolute/project --json takeover --reviewed /absolute/answer.json
```

The request file holds what a reviewer command gets on stdin. The answer has the reviewer's format, and the same guards apply to it, so a reviewer command and a host cannot differ. `awaiting_review` exits 0 because nothing failed. The run keeps waiting until a review is applied: a later command without `--reviewed`, or with an answer the guards refuse, leaves the draft unapplied. `--await-review`, `--reviewer` and `--reviewed` exclude each other.

A host checks for the command before it calls it: `awr capabilities` lists `intake.takeover` with the commands and flags of this build, and `awr capabilities --require intake.takeover` fails on a build that lacks them.

### Guards on a reviewed draft

| Guard | Rule |
| --- | --- |
| Paths | Generated files sit under `.awr/intake/`, are text, and stay within 1 MiB |
| Goals | A goal status of `active` or `confirmed` is lowered to `candidate`; only the owner confirms a goal |
| Work | A ledger returned with no work items gets the draft's `work_items:` section back, so a conversation can bind to a work item |
| Authority | A draft that still lists ambiguous domains stops the run |
| Writes | An `init` preview that lists a path outside `.awr/` other than `.gitignore` stops the run before accept |
| Secrets | A stop names the category, the path and the line of the source; the value appears nowhere. A reviewed draft that maps a document the secret boundary refuses stops at `accept`; with `--manifest`, a document outside the manifest that the reviewed draft maps is refused as a source when `accept` applies it |

### Stops

A stopped run records `step`, `reason` and a message in `takeover.json`, prints the outcome, and exits 1. The error report carries `details.takeover` with the step, the reason and the run directory.

| Reason | Meaning | Next action |
| --- | --- | --- |
| `secret_boundary` | A source the run maps holds content that looks like a credential; such a document the run does not map is listed in `refused_documents` instead and does not stop it | At `preview`: redact the line, or run again with `--manifest <file>` listing the sources AWR may read. At `accept`: redact the line, or leave the file out of the reviewed mapping |
| `source_unreadable` | AWR cannot read a source | Fix the file named in `detail`, then run again |
| `facts_unreadable` | A Git metadata command or parser failed before accept | Check local Git read access, then run again; project files remain unchanged |
| `ambiguous_authority` | Several candidates own one domain | Name the authoritative source in the reviewed draft; the next run reviews again |
| `inventory_changed` | The sources changed during review for more than `--max-rounds` rounds | Run again once the project is quiet |
| `active_session` / `already_initialized` | The project already runs under AWR | Close the session, or use `--adopt` |
| `review_failed` / `review_rejected` | The reviewer failed, timed out, or broke a guard | Fix the reviewer and run again |
| `unexpected_write` | The preview lists a path outside `.awr/` besides `.gitignore`; nothing was written | Change the reviewed draft; the next run reviews again |
| `accept_failed_partial` | `accept` failed after it began writing | Read `detail` and the `.awr/` left behind before running again |

### Output

`--json` prints `{outcome, record, receipt, run_dir}`; plain output prints a summary. `takeover status` shows the record and receipt of the last run. Records use snake_case keys. The receipt lists `mapped` sources, `created` writes, `reviewed` notes, `facts.path` and `facts.coverage`, the `organization` state, goals in `awaiting_confirmation`, and `git` with `committed` and `pushed` set to `false`. Fact coverage includes commit totals, bulk-import file counts and share, unmerged branches, rule-file coverage, and the deferred workstream ratio. When the run leaves documents the secret boundary refuses unmapped, the receipt adds `refused_documents`: their `total`, up to ten `{path, line, category}` entries, a `meaning` and the `inventory` file that lists them, never their content. The `preview` and `accept` step results carry the same summary, and plain output prints the count.

## Limits

- AWR ships no reviewer; any command that follows the contract works.
- Organization is one `intake inspect`; goals stay candidates until the owner confirms them.
- A lock from a stopped process is replaced, and any lock expires after two hours.
- An answer given with `--reviewed` is read only while the review step is unfinished; after it finished, the answer is ignored.
- Resuming reuses the saved draft; a source that changed since the preview starts a new round, up to `--max-rounds`. Another `--manifest` or `--fact-commit-limit` starts again from the preview before accept.

## Code and tests

- [Implementation](../../../crates/awr-cli/src/takeover.rs)
- [Takeover CLI tests](../../../tests/intake/takeover_cli.rs)
- [Synthetic fact-baseline tests](../../../crates/awr-cli/tests/takeover_facts.rs)
- [Synthetic benchmark stream generator](../../../crates/awr-cli/tests/takeover_fact_benchmark.py)
- [Intake overview](../TAKEOVER.md) and [secret boundaries](secret-boundaries.md)
