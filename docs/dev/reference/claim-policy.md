# Claim policy

A claim is runtime ownership of a work item. Without a lifetime it is held until someone releases
it, so a session that stops without ending leaves the work held for good. A project can declare
how long a claim lives when nobody says, and that doing recorded work keeps a claim alive, in an
optional file of its own. Nothing changes for a project that declares nothing.

## The policy file

`.awr/policy.toml`, committed with the project so a team shares it:

```toml
version = 1

[claims]
default_ttl = "4h"          # a claim whose request names no lifetime lives this long; "none" turns it off
renew_on_activity = true    # recorded work extends the live claims of its session (default: true)
```

- `version` is required. This build reads version `1`.
- A duration is a whole number with a unit of `s`, `m`, `h` or `d` (`90s`, `30m`, `4h`, `2d`), or
  `none`. The bounds are one minute and seven days.
- A key or a version this build does not know is refused with a pointer to a newer `awr`; the file
  is never read partly. A file that is not UTF-8 text, is larger than 64 KiB, is a link, or is not
  a policy is refused without echoing its content.
- The file is separate from `.awr/project.toml` on purpose. That manifest rejects keys it does not
  know, so a published `awr` would stop opening a project that grew a `[policy]` table, while it
  never reads a file it has no name for. The 0.5.1 release opens a project that has this file and
  behaves as it did. Delete the file to return to the earlier behaviour; no runtime schema changes.

`AWR_DEFAULT_CLAIM_TTL` (same duration syntax, or `none`) overrides `claims.default_ttl` for one
process, and alone is a policy: a default lifetime with renewal on. The file still decides
`renew_on_activity` when both exist.

## Which requests get the default

A request that acquires a claim and names no lifetime gets `default_ttl` from now:

| Surface | Request |
| --- | --- |
| CLI | `session start --claim`, `work claim`, `session resume --claim` |
| MCP | `awr_session_start` with `claim`, `awr_session_claim` with `acquire`, `awr_session_resume` with `claim: "acquire"` |

- An explicit `--ttl-ms` / `ttl_ms` is unchanged, whatever the policy says.
- `--no-expiry` / `no_expiry` asks for a claim without an expiry. Naming both a lifetime and no
  expiry is a usage error, and so is asking for no expiry without asking for a claim.
- Explicit requests do not read the policy, so they work whatever state the file is in. A request
  that wants the default fails with the policy's error when the file cannot be read; check
  `awr doctor`.
- A handoff, and a resume that inherits a claim, keep the expiry rule they always had: the new
  holder gets the expiry the claim has, with the default neither applied nor renewed on the way.
- Without a declared default a claim without a lifetime has no expiry, as it always had.

## Renewal on recorded work

With `renew_on_activity` and a default, doing recorded work extends the claims the session holds to
`default_ttl` from now:

- a saved checkpoint (CLI `session checkpoint`, MCP `awr_session_checkpoint`), and
- a recorded `work progress`, `work block` or `work unblock` (MCP `awr_work_transition`).

The rules:

- A claim that already runs further is not shortened, and a claim without an expiry stays without
  one. Only claims that are `active` and have an expiry are touched.
- A claim that ran out is never revived. It is marked `expired` and the receipt names it; a claim
  the work needs (a progress action) is refused as it was before. Take the work again with a new
  claim.
- Only the claims of the session that did the work are extended.
- The extension, the work and its receipt commit in one transaction and one project revision, so
  the revision a reply carries is still the revision to send next.

The receipt is the `claim_renewal` field of the event of the work (`checkpoint.created`,
`work.progressed`, `work.blocked`, `work.unblocked`), present only when something changed:

```json
{"claim_renewal": {
  "ttl_ms": 14400000,
  "renewed": [{"claim_id": "…", "work_item_id": "…", "previous_expires_at": 1793422258000, "expires_at": 1793436658000}],
  "expired_claim_ids": []
}}
```

`awr event show <id> --full` reads it.

## What `doctor` and `capabilities` say

- `claim_without_expiry` (warning): an active claim has had no expiry for more than 24 hours. It
  advises and never fails `doctor`, and it comes with no repair; give claims a lifetime or declare
  a default.
- `claim_policy_unreadable` (error): `.awr/policy.toml` or `AWR_DEFAULT_CLAIM_TTL` cannot be read
  by this build. Claims that ask for the default are refused until it is fixed, and nothing is
  renewed.
- `awr capabilities --require operating_baseline` succeeds on a build that has all of this; its
  entry lists `policy_file_v1`, `default_claim_ttl`, `renew_on_activity` and `explicit_no_expiry`.

## Existing claims

Claims acquired before a policy existed keep their expiry, or their lack of one; nothing rewrites
them. A claim without an expiry is never given one by renewal. Release it, or take the work again
after the policy exists.
