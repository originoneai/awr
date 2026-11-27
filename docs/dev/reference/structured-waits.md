# Structured waits on progress checkpoints

`session.checkpoint` accepts an optional `progress.waits` list so agents can record who a work item waits on, what it waits for, when the wait began, and what clears it. `work.snapshot`, `work.observe`, and `work.graph` return that structured record under `structured_waits` while reporting `not_provided` for legacy reports that omit the field.

## Recording structured waits

Include `progress.waits` in `session.checkpoint` whenever work is waiting on a person, a group, or a party outside the project, and send an empty list (`[]`) once those waits clear. Omit the field or pass `null` only when wait state is unreported.

```json
{
  "phase": "blocked",
  "summary": "Export implementation is paused awaiting partner field confirmation.",
  "waits": [
    {
      "waiting_on": {
        "kind": "external_party",
        "name": "Partner Operations"
      },
      "what": "Confirm required export columns for settlement files.",
      "since_unix_ms": 1731542400000,
      "release_condition": "Signed column list received from Partner Operations."
    }
  ]
}
```

### Field bounds and party kinds

| Field | Rule |
| --- | --- |
| `progress.waits` | Optional list of at most 8 entries; stored inside `awr_team.checkpoints.progress_json`. |
| `waiting_on.kind` | Exactly one of `person`, `group`, or `external_party`. |
| `waiting_on.name` | Required single-line text, 1 through 80 Unicode characters, non-blank (excluding whitespace, `U+FEFF`, `U+200B`), free of control characters and line separators (`U+2028`, `U+2029`). |
| `what` | Required single-line text, 1 through 200 Unicode characters, non-blank, free of control characters and line separators. |
| `since_unix_ms` | Required positive integer timestamp in Unix milliseconds (`1` through `4102444800000`, and within 5 minutes of server clock on write). |
| `release_condition` | Required single-line text, 1 through 200 Unicode characters, non-blank, free of control characters and line separators. |

All wait text passes the same public-data screening as other progress fields so credentials and private tokens stay out of recorded summaries.

## External parties and authority boundaries

An `external_party` entry requires zero project membership records. Its `name` is bounded free text for observation clarity, and wait records remain strictly caller-declared observations (`provenance: "caller_declared"`, `grants_authority: false`). Every wait entry leaves work status, leases, membership, and authorization unchanged.

## Reading structured waits

`work.snapshot` (`data.observation.structured_waits`), `work.observe` (`data.structured_waits`), and `work.graph` (`data.items[].structured_waits`) return one deterministic object derived from the work item's latest progress report under its current ownership version:

| Condition | `status` | `items` | `checkpoint_id` / `reported_at_unix_ms` |
| --- | --- | --- | --- |
| Latest progress report includes `waits` (1 to 8 entries) | `"provided"` | Array of waits | Recorded checkpoint metadata |
| Latest progress report explicitly sends `waits: []` | `"provided"` | `[]` | Recorded checkpoint metadata |
| Legacy progress report omits `waits` (or sends `null`) | `"not_provided"` | `null` | Recorded checkpoint metadata |
| Work item has no progress report under current ownership | `"not_provided"` | `null` | `null` |

Each new progress report replaces the previous wait list: sending `waits: []` clears waits (`status: "provided"`, `items: []`), and sending a newer progress report that omits `waits` reports `status: "not_provided"` (`items: null`). An ordinary follow-up `session.checkpoint` that omits `progress` preserves the previous progress report and its `structured_waits`. `work.prepare` excludes observations from `context_hash`, so adding or updating `progress.waits` leaves `context_hash` intact.

Projected `progress` observations omit the raw `waits` key so `structured_waits` remains the single read surface.

## Discovery and bridge validation

`capabilities.session_feedback.structured_waits` publishes the field path (`progress.waits`), allowed `party_kinds`, character and item bounds, supported reads (`work.snapshot`, `work.observe`, `work.graph`), `legacy_status: "not_provided"`, `external_party_requires_membership: false`, and `grants_authority: false`.

The Observer bridge (`tools/observer/server/team-progress.js` and `tools/observer/server/team-bridge.js`) validates `structured_waits` on `work.snapshot`, `work.observe`, and `work.graph` using the same blank-text and timestamp bounds as the server, forwarding bounded fields while rejecting malformed shapes with `BadGateway` and mapping an omitted upstream field to `status: "unknown"`.
