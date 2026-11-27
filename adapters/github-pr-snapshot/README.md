# GitHub pull request snapshot adapter

`github-pr-snapshot-v1` indexes a locally materialized GitHub pull request snapshot as work items. The read path is [github_pr.rs](../../crates/awr-source/src/github_pr.rs).

Fetching stays outside AWR. A host or script writes the API response bytes to a registered file and the adapter parses those bytes; it never contacts GitHub, never sees credentials and never polls. Every row stays byte-bound to the snapshot fingerprint, so the retained snapshot files are the audit trail of what GitHub returned. The adapter is read-only: work actions cannot rewrite the snapshot, and adapter options (`field_map`, `status_map`) are rejected because the mapping is built in.

## Producing a snapshot

```sh
gh pr list --repo OWNER/REPO --state all --limit 200 --json \
  number,title,state,url,isDraft,reviewDecision,author,labels,milestone,files,createdAt,updatedAt,closedAt,mergedAt \
  > .awr/intake/upstream-prs.json
```

Register the file as a primary ledger source:

```toml
[[sources]]
domain = "ledger"
role = "primary"
path = ".awr/intake/upstream-prs.json"
adapter = "github-pr-snapshot-v1"
```

The root must be the top-level JSON array that `gh pr list` emits; a search response wrapped in an `items` object is rejected. An empty array is valid and projects no work. Several repositories can share one snapshot, for example by combining per-repo outputs with `jq -s 'add'`. A fetch cadence is a host concern; re-running the command above and `awr source reindex` refreshes the projection.

## Mapping

- Identity is `{owner}/{repo}#{number}` parsed from the pull request html URL, so two repositories can share a snapshot, and a number, without colliding. A URL whose trailing `/pull/N` disagrees with `number` is rejected.
- `number`, `state` and `url` are required. `title` falls back to the identity key. Unknown extra fields are ignored by the projection and retained in the snapshot bytes.
- `author.login` projects to owner, `labels[].name` to tags, `milestone.title` to milestone, `files[].path` to paths. List entries may also be plain strings.
- Each entry retains its exact JSON pointer (`/0`, `/1`, ...) and a section fingerprint. The fingerprint excludes `updatedAt`, which moves on any GitHub event without changing a mapped field, and normalizes an empty `reviewDecision` to null; any other source edit advances it.

## Status mapping

`raw_status` keeps the GitHub `state` verbatim. Canonical status never guesses:

| GitHub facts | status | blocker / next_action |
| --- | --- | --- |
| `MERGED` | `completed` | — |
| `CLOSED` | `cancelled` | — |
| `OPEN`, `isDraft` | `draft` | next: mark ready for review |
| `OPEN`, no review decision or `REVIEW_REQUIRED` | `blocked` | waiting for review |
| `OPEN`, `CHANGES_REQUESTED` | `in_progress` | next: address the review feedback |
| `OPEN`, `APPROVED` | `blocked` | approved; waiting for a maintainer to merge |
| any unknown spelling | `unknown` | diagnostic warning, no transition inferred |

Review states describe waiting, not work: an approved or unreviewed pull request is `blocked` because the next step belongs to someone else. Snapshot reads cap at 4 MiB.
