# Team MCP browser authorization

The authorization-code policy in `awr-server::oauth` is a foundation for an
optional Team MCP browser entry. This policy module alone does **not** enable
OAuth endpoints or authenticate a native client; HTTP integration and native
client verification are separate delivery steps.

The adapter retains the existing Team identity and permissions. It does not
create an account, project membership, grant, claim or task delegation. The HTTP
entry must verify current project access before approving consent, then recheck
the underlying credential and authorization in each MCP operation.

## Policy

- Public clients register exact HTTPS or HTTP loopback redirect URIs. Wildcards,
  userinfo, fragments, ambiguous URL normalization and preexisting OAuth result
  parameters are rejected. Registration performs no remote fetch.
- Authorization requires S256 PKCE, a registered client/callback and one exact
  operator-configured HTTPS resource. Client state is returned unchanged when
  supplied; browser consent has its own separate binding.
- Consent transactions expire after 10 minutes. Approval requires the original
  browser binding and consumes the transaction atomically.
- Codes expire after 2 minutes. A well-formed exchange consumes the code even
  when client, callback, resource or verifier does not match. Concurrent exchange
  can issue at most one access token.
- Opaque access tokens last at most 1 hour and resolve for one resource only.
  They neither contain nor return the original personal credential. Credential
  expiry, revocation and permission changes must still be enforced by Team.

## Storage and bounds

All registrations, consent, codes and tokens are process-local. Restarting the
server invalidates them. No refresh token or durable sign-in is provided. The
adapter hashes browser bindings, codes and access tokens before storing their
lookup keys; underlying personal credentials stay in memory only.

Registrations expire after 24 hours. There are at most 1,024 registrations,
1,024 consent transactions, 1,024 codes and 4,096 access tokens. Expired entries
are pruned before mutations; reaching a live capacity returns a generic
temporary-unavailability error. Request bodies and HTTP admission also need
their own limits in the transport integration.

These unit-tested policy guarantees do not establish browser security, native
OAuth compatibility or business acceptance. The browser integration must add
same-origin consent, CSRF protection, escaped client labels, no-store/no-referrer
responses, frame protection and operator-bound discovery metadata.
