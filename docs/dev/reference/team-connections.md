# Team connections

Agents use `awr_team_connections_list` and `awr_team_connections_revoke` to
observe and end access. Observer exposes the same operations in its separate
account area. Project work pages remain observational.

## Identity and scope

Omitting `subject` selects the authenticated actor and client. Current
membership, readable workstream access and a live Agent read delegation are
checked before inspecting or changing connections. Task-scoped Agents can end
their own connections without acquiring project-wide administration.

An explicit `subject` contains `actor_id` and `client_id`. Inspecting another
client requires current project administration covering the project's active
workstreams. This path only considers credentials explicitly bound to the
project. Account-wide credentials remain outside project administration.

Authorization locks remain held through the local connection-store operation.
Connection references are non-secret selectors. They never authenticate a
request, reveal a cookie or expose a token family key.

An Agent connection's `client_name` is the name its client registered for itself.
Approval stores that name with the member's credential for the activity ledger
(schema 68); the list keeps reading each connection's own name, so two
connections through one credential keep their own.

## Supported revocation units

| Kind | Effect |
| --- | --- |
| Browser | End one browser sign-in |
| Agent | End one authorization family, including its access and refresh tokens |
| Shared project access | Revoke one credential and every connection using it for the project |
| Shared account access | Revoke one own credential and every connection using it across projects |

Independent devices using the same direct credential cannot be separated by
the service. The list shows shared access at that actual granularity. Project
administrators revoke another member's shared credential through the existing
preview/apply member-management entry. They can separately revoke authorized
project-bound Agent families and browser connections through the connection
entry.

Revocation blocks subsequent connection validation. Requests already admitted
may complete. It does not stop external host processes or undo repository
effects. Credential expiry, membership changes and delegation changes continue
to be checked by PostgreSQL on subsequent operations.

## Results and recovery

List requests accept `protocol_version`, optional `subject`, `cursor` and a
`limit` between 1 and 100. Continue with `next_cursor`, and refresh from the first
page after a meaningful change. Results are current reads, without a historical
snapshot guarantee across pages.

Revoke requests accept `protocol_version`, optional `subject` and the listed
`connection_id`. A known revocation can be repeated without changing its effect.
Unknown, unauthorized and expired references are refused. The response never
claims successful revocation for a missing connection.

If a response is lost, inspect the connection before retrying. A storage failure
or uncertain result must remain unconfirmed. Ending the current browser or its
underlying credential also ends the caller's access. Another valid sign-in is
required to inspect the result if that final response is lost.

## Persistence

When the service configures an encrypted OAuth state directory, Agent families,
browser admission and browser revocation share that single-writer snapshot.
Admission and revocation are persisted before publication. Last-used browser
metadata may be newer in memory than the latest saved snapshot.

Existing snapshots load with absent connection timestamps and an empty browser
list. New snapshot fields require a matching binary when restoring a backup.
Keep the previous binary and its corresponding state snapshot together for
rollback. Without the configured state directory, derived connections last for
the service process lifetime. Shared credential revocation remains durable in
PostgreSQL. Multi-instance connection persistence is outside this implementation.

The encrypted store retains bounded live connection metadata. Expired entries
may disappear. Long-term request outcomes remain in the existing bounded audit
records, subject to their retention and access policies.
