# Agent activity in `work.list` summaries

The overview read model uses an explicit session-activity fact to determine whether an Agent is currently running. Report times and audit activity retain their separate meanings.

## Summary field

A `work.list` item with summary version 1 carries this field; consumers must
still tolerate its absence from older services:

```json
{
  "execution": {
    "value": {
      "session_state": "active",
      "last_checkpoint_at_unix_ms": 1795088880000
    },
    "as_of_unix_ms": 1795089600000,
    "reason": null
  }
}
```

`session_state` is either `active` or `ended`. `last_checkpoint_at_unix_ms` is a timestamp or `null`; it describes the selected session and is not itself evidence that the session is active.

When no session exists, the field is:

```json
{
  "execution": {
    "value": null,
    "as_of_unix_ms": null,
    "reason": "no_session"
  }
}
```

## Field reference

| Aspect | Definition |
| --- | --- |
| Value shape | `session_state` (`active` or `ended`) plus `last_checkpoint_at_unix_ms` (integer or `null`), or an explicit `no_session` absence. |
| Selected session | The most recently created session bound to the work's current ownership version — sessions joined with `workstream_snapshot_ownership` on scope, work, workstream and an equal `ownership_version`. Session ids are ULIDs, so descending id order is creation order. |
| Session state | The selected session's `sessions.state`; only `active` reads as `active`. `ended`, `interrupted` and `incomplete` all read as `ended`. |
| Last checkpoint time | `created_at` of the selected session's latest checkpoint, from the session's `latest_checkpoint_id` pointer. Checkpoint contents stay unread. |
| As-of time | The read transaction's page timestamp, like the other populated summary fields. |
| Absence | A work with no session under the current ownership version — including works whose sessions all belong to older ownership versions — reads as `no_session`, with no guessed state. |

## Why the field stays within the read boundary

The summary is already filtered to works the reader may read; this field adds
only two facts inside that boundary: whether a session is currently live, and
when it last checkpointed. It carries no session, client, conversation or
actor identifier, no checkpoint content, and no claim or lease state. Reading
it grants no execution permission; agent readers' delegation scope is
unchanged. A hidden work stays absent, with or without this field.

## Observer adaptation

The bridge maps `active` to `agentActive: true`. It maps `ended` and the explicit `no_session` absence to `agentActive: false`.

If `execution` is absent, the bridge leaves `agentActive` unset. The derivation then keeps Agent activity unknown. The bridge must not substitute report timestamps, checkpoint timestamps, claims, or audit events for this fact.

The synthetic full demo emits the field from each task's recorded `agent_active` value and uses the report time as its checkpoint time.

## Service status

The production Team service provides this field in every `work.list` and
`work.search` summary item (AWR-PPD-056). One extra batched query per page
fetches the liveness fact for the whole page; no per-work queries are added.
See [Team workstream service](team-workstream-service.md) for the summary
envelope.
