# Agent activity in `work.list` summaries

The overview read model uses an explicit session-activity fact to determine whether an Agent is currently running. Report times and audit activity retain their separate meanings.

## Optional summary field

A `work.list` item with summary version 1 may carry this optional field:

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

## Observer adaptation

The bridge maps `active` to `agentActive: true`. It maps `ended` and the explicit `no_session` absence to `agentActive: false`.

If `execution` is absent, the bridge leaves `agentActive` unset. The derivation then keeps Agent activity unknown. The bridge must not substitute report timestamps, checkpoint timestamps, claims, or audit events for this fact.

The synthetic full demo emits the field from each task's recorded `agent_active` value and uses the report time as its checkpoint time.

## Service status

The production Team service does not provide this field yet. AWR-PPD-056 owns the server-side implementation. Until then, production overview results retain their existing unknown-Agent-activity behavior.
