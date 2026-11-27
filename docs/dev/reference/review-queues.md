Requested reviewers and queues preserve the distinction between a request's recipient and the person who records a review decision. Agents submit requests; Observer reads pending work and points each person back to their Agent.

# Requested reviewers and queues

## Submit and inspect

`review.open` and `delivery.submit_and_request_review` accept optional
`args.requested_reviewer` in either form:

```json
{"kind":"member","person_id":"reviewer-member"}
```

```json
{"kind":"role","role":"reviewer"}
```

A member target resolves to an active project person with review duty. The service
checks the current membership ceiling and recorded person bindings. Ambiguous
bindings remain unresolved. A role target addresses members with review duty.
Omission uses the role form, including historical rounds recorded before schema 70.
The product label for this form is “面向有复核职责的成员” / “Members with review duty”.

The opening receipt, `review.inspect` and `review.rounds` expose
`requested_reviewer` before a decision exists. The decision's recorded reviewer
keeps its existing meaning. Requested routing grants zero authority; each review
decision retains its existing authorization, delegation, policy and independence
checks. Review history also exposes `opened_at_unix_ms` for readable rounds.
Restricted history placeholders mask both new fields.

## Read operations

Both operations use `awr_workstream_query`, protocol version 1. Each requires
live project membership and work-read authority. An Agent's read delegation also
bounds the visible work. Optional `work_id` and `workstream_id` narrow that scope;
an inaccessible selection receives the existing `Forbidden` error.

| Operation | Result |
| --- | --- |
| `review.queue` | Pending open rounds in oldest-first order, plus grouped queue counts and maximum elapsed milliseconds |
| `team.reviewers` | Current review-duty members, each containing exactly `name`, `review_duty`, `queue_count` |

`review.queue` accepts optional `recipient_person_id`: it includes that member's
explicit requests and the shared review-duty requests. An eligible recipient is
resolved within the bound project. Each item includes `round_id`, `work_id`,
`work_title`, `round_index`, `contract_matches_current`, `requested_reviewer`,
`opened_at_unix_ms` and `wait_ms`. Each group includes `requested_reviewer`,
`queue_count` and `oldest_wait_ms`. A shared role queue remains one group.

The member roster follows the same membership action ceiling used for review
admission, then resolves active, unambiguous project persons. It includes each
person once. Its count includes explicitly addressed and shared-role requests
visible to this reader. Shared requests can therefore contribute to several
members' advisory counts; use `review.queue.data.total` for the unique round total.
Current review eligibility remains an admission-time check.

Counts cover authorized matches only. Evidence bound to another work stays outside
the queue. Contract currentness is an explicit fact on each pending round. The
service calculates elapsed time using its transaction clock, clamped at zero.
Source bodies, evidence payloads, user reports, credentials and unrelated duties
remain in their existing controlled storage.

## Paging and Observer

Both reads accept `limit` (1–100, default 20), `cursor` and
`max_context_bytes` (up to 262144). Follow `next_cursor` to exhaustion. Cursors bind
reader authority, scope, source snapshot, review revision and roster. Changed
bindings produce `CursorExpired`; byte limits produce `ResponseTooLarge`.
Responses include `execution_authorized: false`, selector echoes and an `as_of`
measurement. Reads leave business records unchanged.

Observer exposes `GET /api/team/review-queue`, `GET /api/team/reviewers` and the
existing generic read route. All forward the current reader's session. Overview
composition consumes both reads for the shared review-chain and per-person facts.
A named pending request supplies its member identity. A shared-duty request resolves
to a person only when complete roster and directory reads establish exactly one
eligible member; ambiguous names preserve the shared queue. Each page continues
to direct work to that person's Agent.

## Schema and reproducible samples

Migration `20261123000070_review_requested_reviewer.sql` adds the nullable
`review_rounds.requested_reviewer` routing record. The read projection maps legacy
nulls to the shared role target. Schema version 70 is required.

`awr-server`'s `workstream_http` integration suite records exact HTTP response
bytes from its production router against isolated PostgreSQL when
`AWR_REVIEW_SAMPLE_DIR` names a local output directory. Public synthetic samples
live in `tools/observer/test/fixtures/review-queue/`; the bridge tests read those
files directly. The test fixture owns the database and synthetic identities.
